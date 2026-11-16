//! Audio engine. One thread owns the output device and the decoder.
//!
//! Tracks stream over HTTP into memory while they play. A four minute AAC
//! track is about 4 MB, and we free it as soon as the next track loads. A
//! track that finishes downloading is kept in the offline cache, and cached
//! or downloaded tracks play straight from their file.

use rodio::{OutputStream, OutputStreamHandle, Sink};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const CHUNK: u64 = 512 * 1024;

/// How every download error starts, so the player can tell them from
/// broken files and try a fresh link.
pub const STREAM_FAILED: &str = "Stream failed";

/// A download error as people can read it. Never the link itself: it is
/// long, says nothing useful, and is tied to this computer.
fn stream_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(403, _) => format!("{STREAM_FAILED}: YouTube refused the link (403)"),
        ureq::Error::Status(code, _) => format!("{STREAM_FAILED}: YouTube answered {code}"),
        ureq::Error::Transport(t) => format!("{STREAM_FAILED}: {}", t.kind()),
    }
}

pub const IDLE: u32 = 0;
pub const LOADING: u32 = 1;
pub const PLAYING: u32 = 2;
pub const PAUSED: u32 = 3;
pub const ENDED: u32 = 4;
pub const FAILED: u32 = 5;

/// Where a track's bytes come from.
pub enum Input {
    /// Streamed over HTTP. With a video id, a download that finishes is
    /// kept in the offline cache.
    Http {
        url: String,
        headers: Vec<(String, String)>,
        cache_as: Option<String>,
    },
    /// A cached or downloaded file.
    File(std::path::PathBuf),
}

enum Command {
    Load { input: Input, generation: u64 },
    /// Opens the next song ahead, ready to fade in.
    Prepare { input: Input, token: u64 },
    /// Fades from the playing song into the prepared one.
    Crossfade { token: u64, seconds: f32 },
    /// Starts the prepared song the moment the playing one ends, with no
    /// gap. Token zero calls it off.
    Follow { token: u64, gain: f32 },
    /// The levelling gain for the song under `generation`.
    Gain { generation: u64, gain: f32 },
    Play,
    Pause,
    Seek(f32),
    Volume(f32),
    Stop,
}

#[derive(Default)]
pub struct Status {
    pub state: AtomicU32,
    pub position_ms: AtomicU64,
    /// Length of the loaded track as the file states it. Zero when unknown.
    pub duration_ms: AtomicU64,
    /// Which `load` call the state belongs to.
    pub generation: AtomicU64,
    /// Which `prepare` call has its song open and ready to fade in. Zero
    /// when none is.
    pub prepared: AtomicU64,
    /// The `follow` token of a song that just started gaplessly. The app
    /// takes it, moves the queue on, and reports the song's generation.
    pub followed: AtomicU64,
    pub error: Mutex<String>,
}

pub struct Audio {
    tx: Sender<Command>,
    pub status: Arc<Status>,
}

impl Audio {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Status::default());
        let shared = status.clone();
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(rx, shared))
            .expect("cannot start the audio thread");
        Audio { tx, status }
    }

    pub fn load(&self, input: Input, generation: u64) {
        self.status.generation.store(generation, Ordering::SeqCst);
        self.status.state.store(LOADING, Ordering::SeqCst);
        self.status.position_ms.store(0, Ordering::SeqCst);
        self.status.duration_ms.store(0, Ordering::SeqCst);
        self.status.prepared.store(0, Ordering::SeqCst);
        let _ = self.tx.send(Command::Load { input, generation });
    }
    /// Opens the song after this one in the background, so a crossfade can
    /// start it without a wait. `token` names this call in `prepared`.
    pub fn prepare(&self, input: Input, token: u64) {
        self.status.prepared.store(0, Ordering::SeqCst);
        let _ = self.tx.send(Command::Prepare { input, token });
    }
    /// Fades the playing song out and the prepared one in over `seconds`.
    /// The new song is reported under `generation` from now on. Call only
    /// once `prepared` shows `token`.
    pub fn crossfade(&self, token: u64, generation: u64, seconds: f32) {
        self.status.generation.store(generation, Ordering::SeqCst);
        self.status.position_ms.store(0, Ordering::SeqCst);
        self.status.duration_ms.store(0, Ordering::SeqCst);
        let _ = self.tx.send(Command::Crossfade { token, seconds });
    }
    /// Plays the prepared song straight after this one ends. `gain` is its
    /// levelling gain. Call only once `prepared` shows `token`.
    pub fn follow(&self, token: u64, gain: f32) {
        let _ = self.tx.send(Command::Follow { token, gain });
    }
    /// Calls off a `follow`, as when the queue changes.
    pub fn unfollow(&self) {
        let _ = self.tx.send(Command::Follow { token: 0, gain: 1.0 });
    }
    /// A song that started through `follow` now belongs to `generation`.
    pub fn adopt(&self, generation: u64) {
        self.status.generation.store(generation, Ordering::SeqCst);
    }
    /// Sets the levelling gain of the song under `generation`, on top of
    /// the volume. Ignored once another song has started.
    pub fn gain(&self, generation: u64, gain: f32) {
        let _ = self.tx.send(Command::Gain { generation, gain });
    }
    pub fn play(&self) {
        let _ = self.tx.send(Command::Play);
    }
    pub fn pause(&self) {
        let _ = self.tx.send(Command::Pause);
    }
    pub fn seek(&self, seconds: f32) {
        let _ = self.tx.send(Command::Seek(seconds));
    }
    pub fn volume(&self, volume: f32) {
        let _ = self.tx.send(Command::Volume(volume));
    }
    pub fn stop(&self) {
        self.status.prepared.store(0, Ordering::SeqCst);
        let _ = self.tx.send(Command::Stop);
    }
}

fn run(rx: mpsc::Receiver<Command>, status: Arc<Status>) {
    // The device opens on first play, so an idle app holds no audio handle.
    // The stream, its handle, and the device's sample rate.
    let mut output: Option<(OutputStream, OutputStreamHandle, u32)> = None;
    let mut sink: Option<Sink> = None;
    // Tells the playing song to fade out, once it holds a length.
    let mut fade_out = Arc::new(AtomicU32::new(0));
    // The song going out during a crossfade, fading under the new one.
    let mut fading: Option<Sink> = None;
    // The next song, opened ahead for a crossfade, and its `prepare` token.
    let mut next: Option<(u64, crate::decode::Track)> = None;
    // The latest `prepare` token. Songs opened for older ones are dropped.
    let mut wanted = 0u64;
    // Songs open on their own threads, as a stream can take seconds to
    // start, and hand themselves back here.
    let (opened_tx, opened_rx) = mpsc::channel::<(u64, Result<crate::decode::Track, String>)>();
    let mut volume = 1.0f32;
    // Levelling gains of the playing and the fading song.
    let (mut gain, mut fading_gain) = (1.0f32, 1.0f32);
    // The prepared song to start once the playing one ends, and its gain.
    let mut follow: Option<(u64, f32)> = None;
    let fail = |status: &Status, message: String| {
        *status.error.lock().unwrap() = message;
        status.state.store(FAILED, Ordering::SeqCst);
    };
    loop {
        // Waiting to follow, the loop looks in often, so the gap stays tiny.
        let wait = if follow.is_some() { 4 } else { 100 };
        match rx.recv_timeout(Duration::from_millis(wait)) {
            Ok(Command::Load { input, generation }) => {
                sink = None;
                fading = None;
                next = None;
                wanted = 0;
                follow = None;
                gain = 1.0;
                if generation != status.generation.load(Ordering::SeqCst) {
                    continue; // A newer track was requested while we waited.
                }
                if output.is_none() {
                    match open_output() {
                        Ok(opened) => output = Some(opened),
                        Err(e) => {
                            fail(&status, format!("No audio output: {e}"));
                            continue;
                        }
                    }
                }
                let (_, handle, rate) = output.as_ref().unwrap();
                let opened = open_track(input, *rate).and_then(|track| {
                    if let Some(length) = rodio::Source::total_duration(&track) {
                        status
                            .duration_ms
                            .store(length.as_millis() as u64, Ordering::SeqCst);
                    }
                    start(handle, track, volume, 0.0)
                });
                match opened {
                    Ok((new, control)) => {
                        sink = Some(new);
                        fade_out = control;
                        status.state.store(PLAYING, Ordering::SeqCst);
                    }
                    Err(message) => fail(&status, message),
                }
            }
            Ok(Command::Prepare { input, token }) => {
                wanted = token;
                next = None;
                follow = None;
                // Only asked for while a song plays, so the device is open.
                if let Some((_, _, rate)) = &output {
                    let (rate, opened) = (*rate, opened_tx.clone());
                    std::thread::Builder::new()
                        .name("prepare".into())
                        .spawn(move || {
                            let _ = opened.send((token, open_track(input, rate)));
                        })
                        .expect("cannot start the prepare thread");
                }
            }
            Ok(Command::Crossfade { token, seconds }) => {
                status.prepared.store(0, Ordering::SeqCst);
                follow = None;
                let ready = next.take().filter(|(t, _)| *t == token);
                let started = match (ready, &output) {
                    (Some((_, track)), Some((_, handle, _))) => {
                        if let Some(length) = rodio::Source::total_duration(&track) {
                            status
                                .duration_ms
                                .store(length.as_millis() as u64, Ordering::SeqCst);
                        }
                        start(handle, track, volume, seconds)
                    }
                    // The app checks `prepared` first, so this is a race lost
                    // to another command. The player retries the song afresh.
                    _ => Err(format!("{STREAM_FAILED}: the next song was not ready")),
                };
                match started {
                    Ok((new, control)) => {
                        // A pause sent just before stays a pause.
                        let paused = sink.as_ref().is_some_and(|s| s.is_paused());
                        if paused {
                            new.pause();
                        }
                        // The old song fades out over the same time, under
                        // the new one, and then ends.
                        fade_out.store((seconds * 1000.0) as u32, Ordering::SeqCst);
                        fading = sink.replace(new);
                        fading_gain = gain;
                        // The app sends the new song's gain straight after.
                        gain = 1.0;
                        fade_out = control;
                        status
                            .state
                            .store(if paused { PAUSED } else { PLAYING }, Ordering::SeqCst);
                    }
                    Err(message) => {
                        sink = None;
                        fading = None;
                        fail(&status, message);
                    }
                }
            }
            Ok(Command::Play) => {
                if let Some(s) = &sink {
                    s.play();
                    status.state.store(PLAYING, Ordering::SeqCst);
                }
                if let Some(s) = &fading {
                    s.play();
                }
            }
            Ok(Command::Pause) => {
                if let Some(s) = &sink {
                    s.pause();
                    status.state.store(PAUSED, Ordering::SeqCst);
                }
                if let Some(s) = &fading {
                    s.pause();
                }
            }
            Ok(Command::Seek(seconds)) => {
                // A seek during a crossfade cuts the old song off. The new
                // one comes in at full volume, as the source ends its fade.
                fading = None;
                if let Some(s) = &sink {
                    let _ = s.try_seek(Duration::from_secs_f32(seconds.max(0.0)));
                }
            }
            Ok(Command::Follow { token, gain: next_gain }) => {
                follow = (token != 0).then_some((token, next_gain));
            }
            Ok(Command::Gain { generation, gain: g }) => {
                if generation == status.generation.load(Ordering::SeqCst) {
                    gain = g;
                    if let Some(s) = &sink {
                        s.set_volume(volume * gain);
                    }
                }
            }
            Ok(Command::Volume(v)) => {
                volume = v.clamp(0.0, 1.0);
                // Fades scale the samples, so both songs follow the slider.
                if let Some(s) = &sink {
                    s.set_volume(volume * gain);
                }
                if let Some(s) = &fading {
                    s.set_volume(volume * fading_gain);
                }
            }
            Ok(Command::Stop) => {
                sink = None;
                fading = None;
                next = None;
                wanted = 0;
                follow = None;
                status.state.store(IDLE, Ordering::SeqCst);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Ok((token, opened)) = opened_rx.try_recv() {
            // Dropping an older song stops its download.
            if token != wanted {
                continue;
            }
            match opened {
                Ok(track) => {
                    next = Some((token, track));
                    status.prepared.store(token, Ordering::SeqCst);
                }
                // The song ends and the next starts as it did without
                // crossfade, which shows any error then.
                Err(message) => eprintln!("crossfade: {message}"),
            }
        }
        if fading.as_ref().is_some_and(|s| s.empty()) {
            fading = None;
        }
        // The playing song ran out with the next one open: start it now.
        let ended = sink.as_ref().is_some_and(|s| s.empty())
            && status.state.load(Ordering::SeqCst) == PLAYING;
        if let (true, Some((token, next_gain))) = (ended, follow) {
            follow = None;
            let ready = next.take().filter(|(t, _)| *t == token);
            if let (Some((_, track)), Some((_, handle, _))) = (ready, &output) {
                let length = rodio::Source::total_duration(&track);
                if let Ok((new, control)) = start(handle, track, volume * next_gain, 0.0) {
                    status.prepared.store(0, Ordering::SeqCst);
                    status.position_ms.store(0, Ordering::SeqCst);
                    status.duration_ms.store(
                        length.map_or(0, |l| l.as_millis() as u64),
                        Ordering::SeqCst,
                    );
                    sink = Some(new);
                    fade_out = control;
                    gain = next_gain;
                    status.followed.store(token, Ordering::SeqCst);
                    continue;
                }
            }
        }
        if let Some(s) = &sink {
            status
                .position_ms
                .store(s.get_pos().as_millis() as u64, Ordering::SeqCst);
            if s.empty() && status.state.load(Ordering::SeqCst) == PLAYING {
                status.state.store(ENDED, Ordering::SeqCst);
            }
        }
    }
}

/// Opens a track for the decoder. Blocks until its first bytes arrive.
fn open_track(input: Input, rate: u32) -> Result<crate::decode::Track, String> {
    match input {
        Input::Http {
            url,
            headers,
            cache_as,
        } => {
            let source = HttpSource::open(url, headers, cache_as);
            let shared = source.shared.clone();
            let spare = source.reader();
            // Opening reads only the head of the file, so playback
            // starts after the first few kilobytes arrive.
            shared.opening.store(true, Ordering::SeqCst);
            let first = crate::decode::Track::open(Box::new(source), rate);
            shared.opening.store(false, Ordering::SeqCst);
            // A file with its index at the end cannot open from the
            // head. Try again the slow way, reading the whole file.
            first
                .or_else(|_| crate::decode::Track::open(Box::new(spare), rate))
                .map_err(|e| {
                    // A failed download looks like a broken file
                    // to the decoder. Report the download error.
                    shared.buffer.lock().unwrap().error.clone().unwrap_or(e)
                })
        }
        Input::File(path) => {
            let opened = std::fs::File::open(&path)
                .map_err(|e| format!("Cannot open the saved song: {e}"))
                .and_then(|file| crate::decode::Track::open(Box::new(file), rate));
            if opened.is_err() {
                crate::offline::discard_broken(&path);
            }
            opened
        }
    }
}

/// Plays `track` on a sink of its own, fading in over `fade_in` seconds.
/// Returns the sink and the handle that fades it out.
fn start(
    handle: &OutputStreamHandle,
    track: crate::decode::Track,
    volume: f32,
    fade_in: f32,
) -> Result<(Sink, Arc<AtomicU32>), String> {
    let sink = Sink::try_new(handle).map_err(|e| format!("No audio output: {e}"))?;
    sink.set_volume(volume);
    let fade = Fade::new(track, fade_in);
    let control = fade.fade_out.clone();
    sink.append(fade);
    Ok((sink, control))
}

// ---------------------------------------------------------------- fading

/// The gains `t` of the way through a crossfade, for the song coming in
/// and the one going out. Equal power: their squares sum to one, so two
/// unrelated songs together sound as loud as either alone, with no dip in
/// the middle.
fn fade_gains(t: f32) -> (f32, f32) {
    let angle = t.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (angle.sin(), angle.cos())
}

/// A track that can fade in at its start and fade out from any point.
/// The gain is worked out for every sample, so fades are smooth. The
/// sink's volume, the user's slider, applies on top.
struct Fade {
    track: crate::decode::Track,
    /// Samples handed out so far.
    played: u64,
    /// Samples the fade in lasts. Zero for none.
    fade_in: u64,
    /// Milliseconds to fade out over, set from the audio thread. Zero
    /// until the fade out starts.
    fade_out: Arc<AtomicU32>,
    /// Where the fade out began, in samples played, and how many it lasts.
    out: Option<(u64, u64)>,
}

impl Fade {
    fn new(track: crate::decode::Track, fade_in: f32) -> Fade {
        let fade_in = samples_in(&track, fade_in.max(0.0) * 1000.0);
        Fade {
            track,
            played: 0,
            fade_in,
            fade_out: Arc::new(AtomicU32::new(0)),
            out: None,
        }
    }
}

/// How many samples, across all channels, play in `ms` milliseconds.
fn samples_in(track: &crate::decode::Track, ms: f32) -> u64 {
    use rodio::Source;
    let frames = (ms as f64 / 1000.0 * track.sample_rate() as f64) as u64;
    frames * track.channels() as u64
}

impl Iterator for Fade {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        use rodio::Source;
        // Fades start and stop only between frames, so the channels of a
        // frame always come out together.
        if self.played % self.track.channels().max(1) as u64 == 0 {
            if self.out.is_none() {
                let ms = self.fade_out.load(Ordering::Relaxed);
                if ms > 0 {
                    self.out = Some((self.played, samples_in(&self.track, ms as f32).max(1)));
                }
            }
            if let Some((from, length)) = self.out {
                if self.played - from >= length {
                    return None;
                }
            }
        }
        let mut sample = self.track.next()?;
        if self.played < self.fade_in {
            sample *= fade_gains(self.played as f32 / self.fade_in as f32).0;
        }
        if let Some((from, length)) = self.out {
            sample *= fade_gains((self.played - from) as f32 / length as f32).1;
        }
        self.played += 1;
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (low, _) = self.track.size_hint();
        let left = match self.out {
            Some((from, length)) => (from + length).saturating_sub(self.played) as usize,
            None => usize::MAX,
        };
        (low.min(left), None)
    }
}

impl rodio::Source for Fade {
    fn current_frame_len(&self) -> Option<usize> {
        self.track.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.track.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.track.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.track.total_duration()
    }

    /// A seek lands at full volume, even midway through a fade in.
    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        self.fade_in = 0;
        self.track.try_seek(position)
    }
}

/// Opens the default output device at its own rate and remembers that
/// rate, so the decoder can resample to it and rodio never has to.
fn open_output() -> Result<(OutputStream, OutputStreamHandle, u32), String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let device = rodio::cpal::default_host()
        .default_output_device()
        .ok_or("no output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate().0;
    let (stream, handle) =
        OutputStream::try_from_device_config(&device, config).map_err(|e| e.to_string())?;
    Ok((stream, handle, rate))
}

// ------------------------------------------------------------ HTTP source

#[derive(Default)]
struct Buffer {
    data: Vec<u8>,
    total: Option<u64>,
    done: bool,
    error: Option<String>,
}

#[derive(Default)]
struct Shared {
    buffer: Mutex<Buffer>,
    ready: Condvar,
    cancelled: AtomicBool,
    /// Set while the decoder opens the track. The source then says it
    /// cannot seek, so symphonia's MP4 reader stops at the first fragment.
    /// Otherwise it walks every fragment header in the file before it
    /// returns, and that waits for the whole download.
    opening: AtomicBool,
    /// Told the bytes so far and the whole size as the download goes.
    progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
}

/// Stops the download when the last reader of it goes away.
struct Cancel(Arc<Shared>);

impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }
}

/// Fetches a whole track into memory. Blocks, so run it on a worker.
pub fn fetch_all(url: &str, headers: &[(String, String)]) -> Result<Vec<u8>, String> {
    fetch_all_with(url, headers, None)
}

/// `fetch_all`, telling `progress` the bytes so far and the whole size.
pub fn fetch_all_with(
    url: &str,
    headers: &[(String, String)],
    progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<Vec<u8>, String> {
    let shared = Shared {
        progress,
        ..Shared::default()
    };
    download(url, headers, &shared)?;
    let buffer = shared.buffer.into_inner().unwrap();
    if buffer.data.is_empty() || buffer.total.is_some_and(|t| t != buffer.data.len() as u64) {
        return Err("The download stopped early".into());
    }
    Ok(buffer.data)
}

/// A `Read + Seek` view over a download that is still running. Reads block
/// until the bytes they need arrive.
pub struct HttpSource {
    shared: Arc<Shared>,
    position: u64,
    _cancel: Arc<Cancel>,
}

impl HttpSource {
    pub fn open(url: String, headers: Vec<(String, String)>, cache_as: Option<String>) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("download".into())
            .spawn(move || {
                let result = download(&url, &headers, &worker);
                let mut buffer = worker.buffer.lock().unwrap();
                buffer.done = true;
                buffer.error = result.err();
                worker.ready.notify_all();
                // Keep a whole track for next time. A skipped track stops
                // downloading and is not kept.
                let whole = buffer.error.is_none()
                    && !worker.cancelled.load(Ordering::SeqCst)
                    && buffer.total.is_some_and(|t| t == buffer.data.len() as u64);
                if let (true, Some(id)) = (whole, cache_as) {
                    let bytes = buffer.data.clone();
                    drop(buffer);
                    crate::offline::store_cache(&id, &bytes);
                }
            })
            .expect("cannot start the download thread");
        HttpSource {
            _cancel: Arc::new(Cancel(shared.clone())),
            shared,
            position: 0,
        }
    }

    /// A second reader over the same download, from the start.
    fn reader(&self) -> Self {
        HttpSource {
            shared: self.shared.clone(),
            position: 0,
            _cancel: self._cancel.clone(),
        }
    }
}

/// Fetches in ranged chunks. YouTube throttles one long request hard but
/// serves short ranges at full speed.
fn download(url: &str, headers: &[(String, String)], shared: &Shared) -> Result<(), String> {
    let http = crate::innertube::agent(Duration::from_secs(20));
    let mut offset = 0u64;
    loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }
        let range = format!("bytes={offset}-{}", offset + CHUNK - 1);
        let mut request = http.get(url).set("Range", &range);
        if !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("User-Agent"))
        {
            request = request.set("User-Agent", crate::innertube::USER_AGENT);
        }
        for (key, value) in headers {
            request = request.set(key, value);
        }
        let response = match request.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(416, _)) => return Ok(()),
            Err(e) => return Err(stream_error(e)),
        };
        let total = response
            .header("Content-Range")
            .and_then(|h| h.rsplit('/').next())
            .and_then(|t| t.parse::<u64>().ok())
            .or_else(|| {
                (response.status() == 200)
                    .then(|| response.header("Content-Length")?.parse().ok())
                    .flatten()
            });
        let whole_file = response.status() == 200;
        let mut reader = response.into_reader();
        let mut chunk = [0u8; 16 * 1024];
        let mut received = 0u64;
        loop {
            let n = reader
                .read(&mut chunk)
                .map_err(|e| format!("{STREAM_FAILED}: the connection dropped ({})", e.kind()))?;
            if n == 0 || shared.cancelled.load(Ordering::SeqCst) {
                break;
            }
            let mut buffer = shared.buffer.lock().unwrap();
            if buffer.total.is_none() {
                buffer.total = total;
                if let Some(t) = total {
                    buffer.data.reserve_exact(t as usize);
                }
            }
            buffer.data.extend_from_slice(&chunk[..n]);
            received += n as u64;
            let (so_far, whole) = (buffer.data.len() as u64, buffer.total);
            drop(buffer);
            shared.ready.notify_all();
            if let (Some(report), Some(whole)) = (&shared.progress, whole) {
                report(so_far, whole);
            }
        }
        offset += received;
        if whole_file || received == 0 || total.is_some_and(|t| offset >= t) {
            return Ok(());
        }
    }
}

impl Read for HttpSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut buffer = self.shared.buffer.lock().unwrap();
        loop {
            let available = buffer.data.len() as u64;
            if self.position < available {
                let start = self.position as usize;
                let n = out.len().min(buffer.data.len() - start);
                out[..n].copy_from_slice(&buffer.data[start..start + n]);
                self.position += n as u64;
                return Ok(n);
            }
            if let Some(e) = &buffer.error {
                return Err(io::Error::other(e.clone()));
            }
            if buffer.done {
                return Ok(0);
            }
            buffer = self.shared.ready.wait(buffer).unwrap();
        }
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.position as i64 + n,
            SeekFrom::End(n) => {
                let mut buffer = self.shared.buffer.lock().unwrap();
                while buffer.total.is_none() && !buffer.done {
                    buffer = self.shared.ready.wait(buffer).unwrap();
                }
                buffer.total.unwrap_or(buffer.data.len() as u64) as i64 + n
            }
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}

impl symphonia::core::io::MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        !self.shared.opening.load(Ordering::SeqCst)
    }

    /// Waits for the first response, which carries the full length.
    fn byte_len(&self) -> Option<u64> {
        let mut buffer = self.shared.buffer.lock().unwrap();
        while buffer.total.is_none() && !buffer.done {
            buffer = self.shared.ready.wait(buffer).unwrap();
        }
        buffer.total.or(Some(buffer.data.len() as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crossfade_starts_with_the_old_song_and_ends_with_the_new() {
        assert_eq!(fade_gains(0.0), (0.0, 1.0));
        let (incoming, outgoing) = fade_gains(1.0);
        assert!((incoming - 1.0).abs() < 1e-6 && outgoing.abs() < 1e-6);
        // Out of range clamps rather than overshooting.
        assert_eq!(fade_gains(-1.0), fade_gains(0.0));
        assert_eq!(fade_gains(2.0), fade_gains(1.0));
    }

    #[test]
    fn a_crossfade_keeps_the_loudness_steady() {
        let mut last = (0.0, 1.0);
        for step in 0..=100 {
            let (incoming, outgoing) = fade_gains(step as f32 / 100.0);
            assert!((incoming * incoming + outgoing * outgoing - 1.0).abs() < 1e-5);
            // The new song only gets louder and the old one only quieter.
            assert!(incoming >= last.0 && outgoing <= last.1);
            last = (incoming, outgoing);
        }
        // Halfway, both play at the same level, about 3 dB down.
        let (incoming, outgoing) = fade_gains(0.5);
        assert!((incoming - outgoing).abs() < 1e-6);
        assert!((incoming - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }
}
