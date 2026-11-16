//! Turns a streaming M4A into samples for rodio.
//!
//! Two rodio 0.20 problems make us do this ourselves.
//!
//! First, rodio's own symphonia decoder tells symphonia the stream length
//! is unknown. symphonia's MP4 reader needs the length to walk YouTube's
//! fragmented files, fails with a seek error, and rodio panics on it.
//!
//! Second, rodio converts sample rates in short spans and starts a fresh
//! converter for each one. Every restart nudges the timing, dozens of times
//! a second, and that is audible as crackle. So we resample here, in one
//! continuous pass, to the output device's own rate. rodio then sees
//! matching rates and passes the samples straight through.

use rodio::source::SeekError;
use rodio::Source;
use std::time::Duration;
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Bad packets in a row before we give up on a track.
const MAX_BAD_PACKETS: usize = 8;

pub struct Track {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    spec: SignalSpec,
    channels: usize,
    duration: Option<Duration>,
    /// Decoded input, interleaved. Starts one frame before `position` so
    /// the interpolator always has a frame behind it.
    input: Vec<f32>,
    /// Where the next output frame falls, in input frames from the start
    /// of `input`.
    position: f64,
    /// Input frames per output frame.
    step: f64,
    output_rate: u32,
    /// The output frame being handed out, and how far through it we are.
    frame: Vec<f32>,
    channel: usize,
    ended: bool,
}

impl Track {
    /// `output_rate` is the device rate. Zero keeps the track's own rate.
    /// A source that says it cannot seek opens from the head of the file
    /// alone. A seekable one has every fragment header read first.
    pub fn open(source: Box<dyn MediaSource>, output_rate: u32) -> Result<Track, String> {
        let stream = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        hint.with_extension("m4a").mime_type("audio/mp4");
        let options = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let probed = symphonia::default::get_probe()
            .format(&hint, stream, &options, &MetadataOptions::default())
            .map_err(describe)?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or("The stream has no audio track")?;
        let track_id = track.id;
        let duration = track
            .codec_params
            .time_base
            .zip(track.codec_params.n_frames)
            .map(|(base, frames)| {
                let time = base.calc_time(frames);
                Duration::from_secs_f64(time.seconds as f64 + time.frac)
            });
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(describe)?;
        let mut track = Track {
            format,
            decoder,
            track_id,
            spec: SignalSpec::new(44_100, Default::default()),
            channels: 0,
            duration,
            input: Vec::new(),
            position: 1.0,
            step: 1.0,
            output_rate,
            frame: Vec::new(),
            channel: 0,
            ended: false,
        };
        if !track.restart(0)? {
            return Err("The stream has no audio".into());
        }
        Ok(track)
    }

    /// Decodes the next packet that ends after `skip_before` (in track
    /// time base units) and appends it to `input`. False at the end.
    fn decode_more(&mut self, skip_before: u64) -> Result<bool, String> {
        let mut bad = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                // symphonia reports the end of a stream as an EOF IO error.
                Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(false)
                }
                Err(Error::ResetRequired) => return Ok(false),
                Err(e) => return Err(describe(e)),
            };
            if packet.track_id() != self.track_id || packet.ts() + packet.dur() <= skip_before {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    if decoded.frames() == 0 {
                        continue;
                    }
                    let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    samples.copy_interleaved_ref(decoded);
                    if self.channels == 0 {
                        self.spec = spec;
                        self.channels = spec.channels.count().max(1);
                        let rate = if self.output_rate == 0 {
                            spec.rate
                        } else {
                            self.output_rate
                        };
                        self.output_rate = rate;
                        self.step = spec.rate as f64 / rate as f64;
                    }
                    let channels = spec.channels.count().max(1);
                    if channels == self.channels {
                        self.input.extend_from_slice(samples.samples());
                    } else {
                        // A track that changes layout midway. Not seen on
                        // YouTube, but keep the frame size steady anyway.
                        for frame in samples.samples().chunks(channels) {
                            for c in 0..self.channels {
                                self.input.push(frame[c.min(channels - 1)]);
                            }
                        }
                    }
                    return Ok(true);
                }
                Err(Error::DecodeError(_)) if bad < MAX_BAD_PACKETS => bad += 1,
                Err(e) => return Err(describe(e)),
            }
        }
    }

    /// Clears the resampler and fills it from the next packet on. The
    /// first frame is repeated once so there is a frame behind the start.
    fn restart(&mut self, skip_before: u64) -> Result<bool, String> {
        self.input.clear();
        self.frame.clear();
        self.channel = 0;
        self.ended = false;
        if !self.decode_more(skip_before)? {
            self.ended = true;
            return Ok(false);
        }
        let first = self.input[..self.channels].to_vec();
        self.input.splice(0..0, first);
        self.position = 1.0;
        Ok(true)
    }

    fn frames(&self) -> usize {
        self.input.len() / self.channels.max(1)
    }

    /// Computes the next output frame into `self.frame`. False at the end.
    fn next_frame(&mut self) -> bool {
        // The interpolator reads frames i - 1 to i + 2.
        while !self.ended && self.position as usize + 2 >= self.frames() {
            match self.decode_more(0) {
                Ok(true) => {}
                _ => self.ended = true,
            }
        }
        let i = self.position as usize;
        let frames = self.frames();
        if i + 1 >= frames {
            return false;
        }
        let t = (self.position - i as f64) as f32;
        let n = self.channels;
        self.frame.clear();
        for c in 0..n {
            let at = |k: usize| self.input[k.min(frames - 1) * n + c];
            let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
            // 4 point, 3rd order Hermite.
            let c1 = 0.5 * (y2 - y0);
            let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
            self.frame.push(((c3 * t + c2) * t + c1) * t + y1);
        }
        self.position += self.step;
        // Drop frames we no longer need, keeping one behind.
        let used = self.position as usize;
        if used > 4096 {
            let drop = used - 1;
            self.input.drain(..drop * n);
            self.position -= drop as f64;
        }
        true
    }
}

fn describe(error: Error) -> String {
    match error {
        Error::Unsupported(what) => format!("Cannot decode this track: unsupported {what}"),
        // Download errors already read well. Others get the same prefix.
        Error::IoError(e) => {
            let text = e.to_string();
            if text.starts_with(crate::audio::STREAM_FAILED) {
                text
            } else {
                format!("{}: {e}", crate::audio::STREAM_FAILED)
            }
        }
        e => format!("Cannot decode this track: {e}"),
    }
}

impl Iterator for Track {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.channel >= self.frame.len() {
            if !self.next_frame() {
                return None;
            }
            self.channel = 0;
        }
        let sample = self.frame[self.channel];
        self.channel += 1;
        Some(sample)
    }

    /// A lower bound from what is already decoded. rodio's channel
    /// converter underflows if this drops to zero midway through a frame.
    fn size_hint(&self) -> (usize, Option<usize>) {
        let ahead = (self.frames() as f64 - self.position - 2.0) / self.step;
        let frames = if ahead > 0.0 { ahead as usize } else { 0 };
        (
            self.frame.len() - self.channel + frames * self.channels,
            None,
        )
    }
}

impl Source for Track {
    fn current_frame_len(&self) -> Option<usize> {
        // The format never changes midway, so there are no spans to report.
        None
    }

    fn channels(&self) -> u16 {
        self.channels.max(1) as u16
    }

    fn sample_rate(&self) -> u32 {
        self.output_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        let mut position = position;
        if let Some(total) = self.duration {
            position = position.min(total.saturating_sub(Duration::from_millis(50)));
        }
        let time = Time::new(position.as_secs(), position.subsec_nanos() as f64 / 1e9);
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| SeekError::Other(Box::new(e)))?;
        self.decoder.reset();
        // Seeks land on a packet boundary at or before the target. Skip the
        // packets that end before it. AAC packets are about 23 ms long, so
        // this is accurate enough to hear.
        self.restart(seeked.required_ts)
            .map_err(|e| SeekError::Other(Box::new(std::io::Error::other(e))))?;
        Ok(())
    }
}
