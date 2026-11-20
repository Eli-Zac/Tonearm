//! Works out a song's tempo, beats, bars and key from its audio, so songs
//! can be mixed on the beat.
//!
//! It runs on the computer, on a thread of its own, once per song. The
//! result goes in the library database (`library.rs`), so a song is only
//! ever analyzed again when the analyzer changes.
//!
//! The steps:
//! 1. Decode to mono and drop to about 11 kHz. Beats and keys need no more.
//! 2. A short time Fourier transform. Spectral flux, how much louder each
//!    frequency got since the last frame, rises at every note and drum hit.
//! 3. The tempo is the lag at which that onset curve best repeats.
//! 4. Beats are tracked with dynamic programming (Ellis, 2007): the path of
//!    onsets that best keeps to the tempo.
//! 5. A straight line through the beats gives the beat grid and an exact
//!    BPM. How many beats sit on it says how steady the tempo is.
//! 6. Bars: the beat in four where the bass hits and the chords change.
//! 7. Key: the song's pitch classes against the Krumhansl key profiles.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

/// What the analyzer tells the app. Sent from its thread.
pub enum Event {
    /// It started on a song.
    Started(String),
    /// It finished a song, or gave up on it.
    Ready(String),
}

static EVENTS: crate::hook::Hook<Event> = crate::hook::Hook::new();

/// Where analyzer events go. Set once at start.
pub fn listen(listener: impl Fn(Event) + Send + Sync + 'static) {
    EVENTS.set(listener);
}

/// Bump when the analysis changes, so every song is worked out again.
pub const ANALYZER_VERSION: u32 = 3;

/// The rate analysis runs at, about. The real one depends on the song.
const TARGET_RATE: f32 = 11_025.0;
/// Onset frames: about 93 ms long, every 11.6 ms.
const FRAME: usize = 1024;
const HOP: usize = 128;
/// Pitch frames: longer, to tell semitones apart in the bass.
const CHROMA_FRAME: usize = 4096;
const CHROMA_HOP: usize = 1024;
/// Tempos looked for, in BPM.
const SLOWEST: f32 = 60.0;
const FASTEST: f32 = 200.0;
/// A beat more than this far off the grid, in seconds, is off it.
const ON_GRID: f32 = 0.035;
/// Spectral flux peaks this many seconds before the hit that causes it,
/// as the compressed spectrum jumps while the hit is still at the window's
/// edge. Measured on drum loops from 92 to 174 BPM.
const ONSET_LEAD: f32 = 0.0245;

#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    /// Seconds of audio analyzed.
    pub length: f32,
    pub bpm: f32,
    /// The share of tracked beats on the grid, 0 to 1. Low for songs that
    /// speed up and slow down, or have no clear beat.
    pub bpm_confidence: f32,
    /// The first downbeat, in seconds. The grid runs from here every
    /// 60 / `bpm` seconds, both ways.
    pub grid_start: f32,
    /// Each beat as tracked, in seconds. Follows the song where the grid
    /// cannot, as through a tempo change.
    pub beats: Vec<f32>,
    /// Each bar's first beat, in seconds, found bar by bar, so a bar of two
    /// or six beats, or a beat the tracker missed, does not throw the rest.
    /// Empty from an analysis that did not find them.
    pub downbeats: Vec<f32>,
    /// 0 to 11 is C major to B major, 12 to 23 C minor to B minor.
    pub key: Option<u8>,
    /// How well the key profile fits, -1 to 1.
    pub key_confidence: f32,
    /// Low, mid and high loudness, a byte each, about 43 times a second.
    /// For drawing the song when you edit a mix.
    pub waveform: Vec<u8>,
}

impl Analysis {
    /// Seconds between beats.
    pub fn beat(&self) -> f64 {
        60.0 / self.bpm as f64
    }

    /// Whether the beat grid can be trusted for mixing.
    pub fn steady(&self) -> bool {
        self.bpm > 0.0 && self.bpm_confidence >= 0.6 && self.beats.len() >= 32
    }

    /// The key as DJs write it on the Camelot wheel, like 8A for A minor.
    /// Neighbours on the wheel mix in key.
    pub fn camelot(&self) -> Option<String> {
        let key = self.key? as usize;
        let (tonic, minor) = (key % 12, key >= 12);
        // Its relative major gives the number: C major and A minor are 8.
        let major = if minor { (tonic + 3) % 12 } else { tonic };
        let number = (major * 7 + 7) % 12 + 1;
        Some(format!("{number}{}", if minor { 'A' } else { 'B' }))
    }

    /// The key by name, like A minor.
    pub fn key_name(&self) -> Option<String> {
        const NAMES: [&str; 12] = ["C", "D♭", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];
        let key = self.key? as usize;
        let mode = if key >= 12 { "minor" } else { "major" };
        Some(format!("{} {mode}", NAMES[key % 12]))
    }

    /// Each tracked beat's number, counting from the first. A gap of two
    /// beats counts two, as the tracker can skip one, and the count follows
    /// the tempo as it changes, so bars stay bars through a tempo change.
    fn beat_numbers(&self) -> Vec<i64> {
        let gaps: Vec<f32> = self.beats.windows(2).map(|w| w[1] - w[0]).collect();
        let mut numbers = Vec::with_capacity(self.beats.len());
        let mut n = 0i64;
        numbers.push(n);
        for i in 0..gaps.len() {
            // The usual gap here, from the beats either side.
            let mut near: Vec<f32> = gaps[i.saturating_sub(4)..(i + 5).min(gaps.len())].to_vec();
            near.sort_by(f32::total_cmp);
            let usual = near[near.len() / 2].max(1e-3);
            n += (gaps[i] / usual).round() as i64;
            numbers.push(n);
        }
        numbers
    }

    /// The found downbeat nearest `t`, if one is within `reach` seconds.
    fn downbeat_near(&self, t: f64, reach: f64) -> Option<f64> {
        self.downbeats
            .iter()
            .map(|&d| d as f64)
            .min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs()))
            .filter(|d| (d - t).abs() <= reach)
    }

    /// The beat grid of the part of the song from `from` to `to` seconds,
    /// fitted to the beats tracked there and a little either side. Follows
    /// a song whose tempo changes, where the one grid of the whole song
    /// (`bpm`, `grid_start`) cannot. None with too few beats there.
    pub fn local(&self, from: f64, to: f64) -> Option<LocalGrid> {
        const MARGIN: f64 = 2.0;
        if self.beats.len() < 8 {
            return None;
        }
        let numbers = self.beat_numbers();
        let picked: Vec<(f64, f64)> = self
            .beats
            .iter()
            .zip(&numbers)
            .filter(|(&t, _)| (from - MARGIN..=to + MARGIN).contains(&(t as f64)))
            .map(|(&t, &n)| (n as f64, t as f64))
            .collect();
        if picked.len() < 8 {
            return None;
        }
        // A straight line through them, then again without the ones far off it.
        let fit = |points: &[(f64, f64)]| -> Option<(f64, f64)> {
            let count = points.len() as f64;
            let (mk, mt) = points.iter().fold((0.0, 0.0), |(a, b), (k, t)| (a + k / count, b + t / count));
            let (mut kk, mut kt) = (0.0, 0.0);
            for (k, t) in points {
                kk += (k - mk) * (k - mk);
                kt += (k - mk) * (t - mt);
            }
            (kk > 0.0).then(|| (mt - kt / kk * mk, kt / kk))
        };
        let (mut a, mut beat) = fit(&picked)?;
        let near: Vec<(f64, f64)> = picked
            .iter()
            .copied()
            .filter(|(k, t)| (t - (a + beat * k)).abs() < ON_GRID as f64 * 2.0)
            .collect();
        if near.len() >= 8 {
            (a, beat) = fit(&near)?;
        }
        if !(60.0 / FASTEST as f64..=60.0 / SLOWEST as f64).contains(&beat) {
            return None;
        }
        let on = picked
            .iter()
            .filter(|(k, t)| (t - (a + beat * k)).abs() < ON_GRID as f64)
            .count();
        let downbeat = match self.downbeat_near((from + to) / 2.0, (to - from) / 2.0 + MARGIN) {
            // The bar found there, on the grid of the part.
            Some(t) => a + beat * ((t - a) / beat).round(),
            // Without them, the tracked beat nearest the song's first
            // downbeat is one, and so is every fourth beat counted from it.
            None => {
                let first_down = self
                    .beats
                    .iter()
                    .position(|&t| t >= self.grid_start - 0.1)
                    .unwrap_or(0);
                a + beat * numbers[first_down] as f64
            }
        };
        Some(LocalGrid {
            beat,
            downbeat,
            confidence: on as f32 / picked.len() as f32,
            beats: picked.len(),
        })
    }
}

/// The beat grid of one part of a song.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalGrid {
    /// Seconds between beats.
    pub beat: f64,
    /// A downbeat on the grid, in seconds. It may lie outside the part.
    pub downbeat: f64,
    /// The share of the part's beats on the grid, 0 to 1.
    pub confidence: f32,
    /// How many tracked beats the part has.
    pub beats: usize,
}

impl LocalGrid {
    /// Whether the part keeps a steady enough beat to mix on.
    pub fn steady(&self) -> bool {
        self.confidence >= 0.6 && self.beats >= 8
    }

    /// The beat on the grid nearest `t`.
    pub fn nearest_beat(&self, t: f64) -> f64 {
        self.downbeat + ((t - self.downbeat) / self.beat).round() * self.beat
    }

    /// The downbeat on the grid nearest `t`.
    pub fn nearest_downbeat(&self, t: f64) -> f64 {
        let bar = 4.0 * self.beat;
        self.downbeat + ((t - self.downbeat) / bar).round() * bar
    }

    /// The last downbeat at or before `t`.
    pub fn downbeat_before(&self, t: f64) -> f64 {
        let bar = 4.0 * self.beat;
        self.downbeat + ((t - self.downbeat) / bar + 1e-9).floor() * bar
    }
}

// ------------------------------------------------------------------ worker

enum Audio {
    Bytes(Vec<u8>),
    File(PathBuf),
}

struct Worker {
    tx: Sender<(String, Audio)>,
    /// Songs waiting or under way, so none is done twice at once.
    busy: HashSet<String>,
    /// The song under way.
    current: Option<String>,
    /// Songs whose audio would not analyze, so they are not tried again
    /// until the next start.
    failed: HashSet<String>,
}

/// Where a song is in the analyzer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Not waiting: done, or not asked for.
    Idle,
    Waiting,
    Working,
}

/// Where a song is in the analyzer.
pub fn progress(video_id: &str) -> Progress {
    let worker = worker();
    if worker.current.as_deref() == Some(video_id) {
        Progress::Working
    } else if worker.busy.contains(video_id) {
        Progress::Waiting
    } else {
        Progress::Idle
    }
}

fn worker() -> std::sync::MutexGuard<'static, Worker> {
    static WORKER: OnceLock<Mutex<Worker>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<(String, Audio)>();
            std::thread::Builder::new()
                .name("analysis".into())
                .spawn(move || {
                    for (video_id, audio) in rx {
                        worker().current = Some(video_id.clone());
                        EVENTS.emit(Event::Started(video_id.clone()));
                        let result = match audio {
                            Audio::Bytes(bytes) => analyze_source(Box::new(std::io::Cursor::new(bytes))),
                            Audio::File(path) => std::fs::File::open(&path)
                                .map_err(|e| e.to_string())
                                .and_then(|file| analyze_source(Box::new(file))),
                        };
                        let failed = result.is_err();
                        match result {
                            Ok(analysis) => {
                                crate::library::save_analysis(&video_id, &analysis);
                            }
                            Err(message) => eprintln!("analysis of {video_id}: {message}"),
                        }
                        EVENTS.emit(Event::Ready(video_id.clone()));
                        let mut done = worker();
                        done.busy.remove(&video_id);
                        done.current = None;
                        if failed {
                            done.failed.insert(video_id.clone());
                        }
                    }
                })
                .expect("cannot start the analysis thread");
            Mutex::new(Worker {
                tx,
                busy: HashSet::new(),
                current: None,
                failed: HashSet::new(),
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Whether songs are analyzed at all. An app that never beat matches (the
/// phone's) turns it off, to spare the battery.
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn queue(video_id: &str, audio: impl FnOnce() -> Option<Audio>) {
    if !ENABLED.load(std::sync::atomic::Ordering::Relaxed) || crate::library::has_analysis(video_id) {
        return;
    }
    let mut worker = worker();
    if worker.busy.contains(video_id) || worker.failed.contains(video_id) {
        return;
    }
    let Some(audio) = audio() else {
        return;
    };
    worker.busy.insert(video_id.to_string());
    let _ = worker.tx.send((video_id.to_string(), audio));
}

/// Analyzes a song from its saved file, if it has one and no analysis yet.
pub fn request(video_id: &str) {
    queue(video_id, || crate::offline::saved_file(video_id).map(Audio::File));
}

/// Analyzes a song from a whole copy of its file just fetched, if it has
/// no analysis yet.
pub fn submit(video_id: &str, bytes: &[u8]) {
    queue(video_id, || Some(Audio::Bytes(bytes.to_vec())));
}

// ---------------------------------------------------------------- decoding

/// The onset curve of mono samples, and its frames a second.
#[cfg(test)]
pub fn onset_curve(samples: &[f32], rate: f32) -> (Vec<f32>, f32) {
    let mut flux = Vec::new();
    let mut previous = vec![0.0f32; FRAME / 2];
    spectra(samples, FRAME, HOP, |_, magnitude| {
        let mut all = 0.0;
        for k in 1..FRAME / 2 {
            let now = (1.0 + 100.0 * magnitude[k]).ln();
            all += (now - previous[k]).max(0.0);
            previous[k] = now;
        }
        flux.push(all);
    });
    (normalized(&flux), rate / HOP as f32)
}

/// Analyzes a song file, on the calling thread.
#[cfg(test)]
pub fn analyze_file(path: &std::path::Path) -> Result<Analysis, String> {
    analyze_source(Box::new(std::fs::File::open(path).map_err(|e| e.to_string())?))
}

fn analyze_source(source: Box<dyn symphonia::core::io::MediaSource>) -> Result<Analysis, String> {
    let (samples, rate) = decode_mono(source)?;
    if samples.len() < (rate * 10.0) as usize {
        return Err("too short to analyze".into());
    }
    Ok(analyze(&samples, rate))
}

/// The song in mono at about `TARGET_RATE`, and the rate it came out at.
fn decode_mono(source: Box<dyn symphonia::core::io::MediaSource>) -> Result<(Vec<f32>, f32), String> {
    use rodio::Source;
    // Rate zero keeps the song's own rate, with no resampling.
    let track = crate::decode::Track::open(source, 0)?;
    let channels = track.channels().max(1) as usize;
    let rate = track.source_rate().max(1) as f32;
    // Averaging is a rough low pass, but enough for onsets and pitch.
    let factor = (rate / TARGET_RATE).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(track.total_duration().map_or(0, |d| {
        (d.as_secs_f32() * rate) as usize / factor + 1
    }));
    let (mut sum, mut count) = (0.0f32, 0usize);
    let scale = 1.0 / (channels * factor) as f32;
    for sample in track {
        sum += sample;
        count += 1;
        if count == channels * factor {
            out.push(sum * scale);
            sum = 0.0;
            count = 0;
        }
    }
    Ok((out, rate / factor as f32))
}

// ------------------------------------------------------------------- FFT

/// A radix 2 complex FFT of one fixed size.
struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    reverse: Vec<usize>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two());
        let bits = n.trailing_zeros();
        let reverse = (0..n)
            .map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (usize::BITS - bits) })
            .collect();
        let angle = |k: usize| -2.0 * std::f64::consts::PI * k as f64 / n as f64;
        Fft {
            n,
            cos: (0..n / 2).map(|k| angle(k).cos() as f32).collect(),
            sin: (0..n / 2).map(|k| angle(k).sin() as f32).collect(),
            reverse,
        }
    }

    fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.reverse[i];
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let stride = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let (c, s) = (self.cos[k * stride], self.sin[k * stride]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
    }
}

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos())
        .collect()
}

/// The magnitude spectrum of each frame, `hop` apart, handed to `each`
/// with its index.
fn spectra(samples: &[f32], frame: usize, hop: usize, mut each: impl FnMut(usize, &[f32])) {
    if samples.len() < frame {
        return;
    }
    let fft = Fft::new(frame);
    let window = hann(frame);
    let (mut re, mut im) = (vec![0.0; frame], vec![0.0; frame]);
    let mut magnitude = vec![0.0; frame / 2];
    for (index, start) in (0..=samples.len() - frame).step_by(hop).enumerate() {
        for i in 0..frame {
            re[i] = samples[start + i] * window[i];
            im[i] = 0.0;
        }
        fft.run(&mut re, &mut im);
        for k in 0..frame / 2 {
            magnitude[k] = (re[k] * re[k] + im[k] * im[k]).sqrt();
        }
        each(index, &magnitude);
    }
}

// ---------------------------------------------------------------- analysis

/// Everything at once, from mono samples at `rate`.
fn analyze(samples: &[f32], rate: f32) -> Analysis {
    let length = samples.len() as f32 / rate;
    let frame_rate = rate / HOP as f32;
    // A frame's time is its centre, moved to where its onsets really are.
    let frame_time = |f: f32| (f * HOP as f32 + FRAME as f32 / 2.0) / rate + ONSET_LEAD;
    let bin_hz = rate / FRAME as f32;
    let low_top = (150.0 / bin_hz).ceil() as usize;
    let (mid_from, high_from) = ((250.0 / bin_hz) as usize, (2500.0 / bin_hz) as usize);

    let mut flux = Vec::new();
    let mut low_flux = Vec::new();
    let mut bands: Vec<[f32; 3]> = Vec::new();
    let mut previous = vec![0.0f32; FRAME / 2];
    let mut current = vec![0.0f32; FRAME / 2];
    spectra(samples, FRAME, HOP, |_, magnitude| {
        let (mut all, mut low) = (0.0, 0.0);
        let mut energy = [0.0f32; 3];
        for k in 1..FRAME / 2 {
            // Compressed, so quiet notes count as well as loud ones.
            current[k] = (1.0 + 100.0 * magnitude[k]).ln();
            let rise = (current[k] - previous[k]).max(0.0);
            all += rise;
            if k <= low_top {
                low += rise;
            }
            let band = if k < mid_from { 0 } else if k < high_from { 1 } else { 2 };
            energy[band] += magnitude[k] * magnitude[k];
        }
        std::mem::swap(&mut previous, &mut current);
        flux.push(all);
        low_flux.push(low);
        bands.push(energy.map(f32::sqrt));
    });

    let onsets = normalized(&flux);
    let lows = normalized(&low_flux);
    // Each likely tempo, tracked. The one whose beats keep best to a grid
    // wins, the first guess on a near tie.
    let mut best: Option<(Vec<f32>, (f32, f32, f32))> = None;
    for (i, period) in tempo_periods(&onsets, frame_rate).into_iter().enumerate() {
        let beats: Vec<f32> = track_beats(&onsets, period).iter().map(|&f| frame_time(f as f32)).collect();
        let grid = fit_grid(&onsets, period, frame_time, frame_rate, &beats);
        let margin = if i == 0 { 0.0 } else { 0.05 };
        if best.as_ref().is_none_or(|(_, b)| grid.2 > b.2 + margin) {
            best = Some((beats, grid));
        }
    }
    let (beats, (bpm, offset, bpm_confidence)) = best.unwrap_or_default();

    let chroma = chroma_frames(samples, rate);
    let chroma_time = |i: usize| (i * CHROMA_HOP + CHROMA_FRAME / 2) as f32 / rate;
    let beat = 60.0 / bpm;
    let first = beats.first().copied().unwrap_or(0.0);
    let onset_frame = |t: f32| ((t - frame_time(0.0)) * frame_rate).round();
    let downbeats = find_downbeats(&beats, &lows, &bands, &chroma, onset_frame, chroma_time);
    let grid_start = if bpm > 0.0 {
        let phase = downbeat_phase(&lows, &chroma, onset_frame, chroma_time, offset, beat, length);
        // The first downbeat at or after the first tracked beat.
        let mut start = offset + phase as f32 * beat;
        let bar = 4.0 * beat;
        start -= ((start - first + beat / 2.0) / bar).floor() * bar;
        // The bars found one by one know better where the song starts.
        if let Some(&down) = downbeats.first() {
            start += ((down - start) / beat).round() * beat;
            start -= ((start - first + beat / 2.0) / bar).floor() * bar;
        }
        start
    } else {
        0.0
    };
    let (key, key_confidence) = find_key(&chroma);

    Analysis {
        length,
        bpm,
        bpm_confidence,
        grid_start,
        beats,
        downbeats,
        key,
        key_confidence,
        waveform: waveform(&bands),
    }
}

/// Divided by its standard deviation, as the beat tracker's weights expect.
fn normalized(values: &[f32]) -> Vec<f32> {
    let n = values.len().max(1) as f32;
    let mean = values.iter().sum::<f32>() / n;
    let deviation = (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n).sqrt();
    let scale = if deviation > 0.0 { 1.0 / deviation } else { 0.0 };
    values.iter().map(|v| v * scale).collect()
}

/// Frames between beats, from the onset curve's autocorrelation. Weighted
/// towards 120 BPM, as half and double the tempo repeat nearly as well.
///
/// The first is the best guess. Any after it are a third faster or slower
/// and nearly as likely: triplet hi-hats and 3+3+2 rhythms, as in drill,
/// repeat at those lags too. Tracking the beats at each tells them apart.
fn tempo_periods(onsets: &[f32], frame_rate: f32) -> Vec<f32> {
    let n = onsets.len();
    let mean = onsets.iter().sum::<f32>() / n.max(1) as f32;
    let centred: Vec<f32> = onsets.iter().map(|v| v - mean).collect();
    let shortest = (frame_rate * 60.0 / FASTEST).floor() as usize;
    let longest = (frame_rate * 60.0 / SLOWEST).ceil() as usize;
    // Out to four of the slowest beats, to see bars.
    let most = (longest * 4 + 4).min(n.saturating_sub(1));
    let ac: Vec<f32> = (0..=most)
        .map(|lag| {
            centred[..n - lag]
                .iter()
                .zip(&centred[lag..])
                .map(|(a, b)| a * b)
                .sum::<f32>()
                / (n - lag) as f32
        })
        .collect();
    let at = |lag: usize| ac.get(lag).copied().unwrap_or(0.0).max(0.0);
    let mut best = (shortest.max(1), f32::MIN);
    for lag in shortest.max(1)..=longest.min(most) {
        let bpm = frame_rate * 60.0 / lag as f32;
        let prior = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp();
        // A beat repeats at twice its lag too. Half the tempo would not.
        let score = prior * (at(lag) + 0.5 * at(lag * 2));
        if score > best.1 {
            best = (lag, score);
        }
    }
    // The highest repeat within 2% of a lag.
    let peak = |lag: f32| -> (usize, f32) {
        let (from, to) = ((lag * 0.98).floor() as usize, (lag * 1.02).ceil() as usize);
        (from.max(1)..=to.max(1))
            .map(|l| (l, at(l)))
            .fold((lag.round() as usize, f32::MIN), |b, c| if c.1 > b.1 { c } else { b })
    };
    // A real beat repeats at the half bar and, above all, the bar.
    let metre = |lag: f32| peak(lag).1 + peak(2.0 * lag).1 + 2.0 * peak(4.0 * lag).1;
    let found = metre(best.0 as f32);
    let mut lags = vec![best.0];
    for ratio in [4.0 / 3.0, 3.0 / 4.0, 3.0 / 2.0, 2.0 / 3.0] {
        let lag = peak(best.0 as f32 * ratio).0;
        if (shortest.max(1)..=longest).contains(&lag) && metre(lag as f32) > 0.9 * found && !lags.contains(&lag) {
            lags.push(lag);
        }
    }
    // Between frames, from each peak's neighbours.
    lags.into_iter()
        .map(|lag| {
            if lag == 0 || lag + 1 > most {
                return lag.max(1) as f32;
            }
            let (a, b, c) = (ac[lag - 1], ac[lag], ac[lag + 1]);
            let bend = a - 2.0 * b + c;
            let shift = if bend < 0.0 { (0.5 * (a - c) / bend).clamp(-0.5, 0.5) } else { 0.0 };
            lag as f32 + shift
        })
        .collect()
}

/// Beat frames by dynamic programming: each frame's best score is its own
/// onset plus the best earlier beat, less a penalty for straying from the
/// tempo. The best path back from the end is the beats.
fn track_beats(onsets: &[f32], period: f32) -> Vec<usize> {
    let n = onsets.len();
    if n == 0 || period < 2.0 {
        return Vec::new();
    }
    // Onsets near each frame count, the nearest the most.
    let reach = period.round() as isize;
    let kernel: Vec<f32> = (-reach..=reach)
        .map(|i| (-0.5 * (i as f32 * 32.0 / period).powi(2)).exp())
        .collect();
    let local: Vec<f32> = (0..n as isize)
        .map(|t| {
            kernel
                .iter()
                .enumerate()
                .map(|(k, w)| {
                    let at = t + k as isize - reach;
                    if (0..n as isize).contains(&at) {
                        w * onsets[at as usize]
                    } else {
                        0.0
                    }
                })
                .sum()
        })
        .collect();
    const TIGHTNESS: f32 = 100.0;
    let (near, far) = ((period / 2.0).round() as usize, (period * 2.0).round() as usize);
    let mut score = vec![0.0f32; n];
    let mut back = vec![usize::MAX; n];
    for t in 0..n {
        let mut best = (usize::MAX, f32::MIN);
        if t >= near {
            for from in t.saturating_sub(far)..=t - near {
                let gap = (t - from) as f32 / period;
                let s = score[from] - TIGHTNESS * gap.ln().powi(2);
                if s > best.1 {
                    best = (from, s);
                }
            }
        }
        if best.0 != usize::MAX && best.1 > 0.0 {
            score[t] = local[t] + best.1;
            back[t] = best.0;
        } else {
            score[t] = local[t];
        }
    }
    // The last beat: the best scoring frame in the last beat or so.
    let tail = n.saturating_sub(period.ceil() as usize + 1);
    let mut t = (tail..n).max_by(|&a, &b| score[a].total_cmp(&score[b])).unwrap_or(n - 1);
    let mut beats = vec![t];
    while back[t] != usize::MAX {
        t = back[t];
        beats.push(t);
    }
    beats.reverse();
    // Quiet ends have no beat to follow. Trim beats there.
    let mut strengths: Vec<f32> = beats.iter().map(|&b| local[b]).collect();
    strengths.sort_by(f32::total_cmp);
    let floor = 0.25 * strengths.get(strengths.len() / 2).copied().unwrap_or(0.0);
    let first = beats.iter().position(|&b| local[b] >= floor).unwrap_or(0);
    let last = beats.iter().rposition(|&b| local[b] >= floor).unwrap_or(0);
    beats.truncate(last + 1);
    beats.drain(..first);
    beats
}

/// The beat grid: the BPM, a beat on the grid in seconds, and the share of
/// tracked beats that sit on it.
///
/// The grid is the period and phase whose beats land on the most onset
/// strength across the whole song, searched near the tempo estimate. Over
/// hundreds of beats that pins the tempo to a few hundredths of a BPM, and
/// a skipped or doubled beat in the tracking cannot throw it.
fn fit_grid(
    onsets: &[f32],
    rough: f32,
    frame_time: impl Fn(f32) -> f32,
    frame_rate: f32,
    beats: &[f32],
) -> (f32, f32, f32) {
    let n = onsets.len();
    if beats.len() < 8 || rough < 2.0 || n < (rough * 16.0) as usize {
        return (0.0, 0.0, 0.0);
    }
    // For the coarse search, each frame's strongest onset nearby, so a hit
    // a frame early or late still counts. The fine search uses the onsets
    // as they are, so the grid lands on the hits and not a frame early.
    let peaks: Vec<f32> = (0..n)
        .map(|t| onsets[t.saturating_sub(1)..(t + 2).min(n)].iter().copied().fold(0.0, f32::max))
        .collect();
    let comb = |curve: &[f32], period: f64, phase: f64| {
        let mut sum = 0.0f32;
        let mut t = phase;
        while t < (n - 1) as f64 {
            let i = t as usize;
            let f = (t - i as f64) as f32;
            sum += curve[i] + (curve[i + 1] - curve[i]) * f;
            t += period;
        }
        sum
    };
    let search = |curve: &[f32], periods: Vec<f64>, phases: &dyn Fn(f64) -> Vec<f64>| {
        let mut best = (rough as f64, 0.0f64, f32::MIN);
        for period in periods {
            for phase in phases(period) {
                let score = comb(curve, period, phase);
                if score > best.2 {
                    best = (period, phase, score);
                }
            }
        }
        (best.0, best.1)
    };
    // The period within 4% of the estimate, then closer.
    let coarse: Vec<f64> = (-80..=80).map(|k| rough as f64 * (1.0 + k as f64 * 0.0005)).collect();
    let whole = |period: f64| -> Vec<f64> { (0..(period * 2.0).ceil() as usize).map(|i| i as f64 * 0.5).collect() };
    let (period, phase) = search(&peaks, coarse, &whole);
    let fine: Vec<f64> = (-25..=25).map(|k| period * (1.0 + k as f64 * 0.00002)).collect();
    let near = move |_: f64| -> Vec<f64> { (-20..=20).map(|i| (phase + i as f64 * 0.05).max(0.0)).collect() };
    let (period, phase) = search(onsets, fine, &near);

    let beat = period / frame_rate as f64;
    let offset = frame_time(phase as f32) as f64;
    let on = beats
        .iter()
        .filter(|&&t| {
            let along = (t as f64 - offset) / beat;
            ((along - along.round()) * beat).abs() < ON_GRID as f64
        })
        .count();
    ((60.0 / beat) as f32, offset as f32, on as f32 / beats.len() as f32)
}

/// The downbeats among the tracked `beats`, in seconds.
///
/// Each beat gets a score for how much it sounds like a bar's first: the
/// bass hits hard and the chords change there. Then the best way through,
/// counting one to four along the beats, with the strongest beats counted
/// one. A beat the tracker skipped is counted too. The count only starts
/// over where the music says so for bars on end, as where a song drops a
/// beat or adds two, so one stray accent cannot shift it.
fn find_downbeats(
    beats: &[f32],
    lows: &[f32],
    bands: &[[f32; 3]],
    chroma: &[[f32; 12]],
    onset_frame: impl Fn(f32) -> f32,
    chroma_time: impl Fn(usize) -> f32,
) -> Vec<f32> {
    // What starting the count over costs, in accents: some ten bars of
    // clear evidence. Any less and the count wanders in steady songs.
    const RESTART: f32 = 40.0;
    let n = beats.len();
    if n < 16 {
        return Vec::new();
    }
    // How many beats each step along the tracked beats is.
    let gaps: Vec<f32> = beats.windows(2).map(|w| w[1] - w[0]).collect();
    let steps: Vec<usize> = (0..gaps.len())
        .map(|i| {
            let mut near: Vec<f32> = gaps[i.saturating_sub(4)..(i + 5).min(gaps.len())].to_vec();
            near.sort_by(f32::total_cmp);
            let usual = near[near.len() / 2].max(1e-3);
            ((gaps[i] / usual).round() as usize).clamp(1, 4)
        })
        .collect();
    // Each beat's pitch classes, up to the next beat.
    let mut per_beat: Vec<[f32; 12]> = Vec::with_capacity(n);
    let mut c = 0;
    for i in 0..n {
        let from = beats[i];
        let to = beats.get(i + 1).copied().unwrap_or(from + gaps[gaps.len() - 1]);
        while c < chroma.len() && chroma_time(c) < from {
            c += 1;
        }
        let mut sum = [0.0f32; 12];
        let mut k = c;
        while k < chroma.len() && chroma_time(k) < to {
            for p in 0..12 {
                sum[p] += chroma[k][p];
            }
            k += 1;
        }
        per_beat.push(sum);
    }
    let distance = |a: &[f32; 12], b: &[f32; 12]| {
        let dot: f32 = (0..12).map(|p| a[p] * b[p]).sum();
        let size = |v: &[f32; 12]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let both = size(a) * size(b);
        if both > 0.0 { 1.0 - dot / both } else { 0.0 }
    };
    let add = |a: &[f32; 12], b: &[f32; 12]| -> [f32; 12] { std::array::from_fn(|p| a[p] + b[p]) };
    let bass: Vec<f32> = beats
        .iter()
        .map(|&t| {
            let at = onset_frame(t) as i64;
            (at - 2..=at + 2)
                .filter_map(|f| usize::try_from(f).ok().and_then(|f| lows.get(f).copied()))
                .fold(0.0f32, f32::max)
        })
        .collect();
    // The two beats before against the two from here.
    let change: Vec<f32> = (0..n)
        .map(|i| {
            if i >= 2 && i + 2 <= n {
                distance(&add(&per_beat[i - 2], &per_beat[i - 1]), &add(&per_beat[i], &per_beat[i + 1]))
            } else {
                0.0
            }
        })
        .collect();
    // On one scale each, so neither drowns out the other.
    let standard = |v: &[f32]| -> Vec<f32> {
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        let spread = (v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / v.len() as f32).sqrt();
        v.iter()
            .map(|x| if spread > 0.0 { ((x - mean) / spread).clamp(-3.0, 3.0) } else { 0.0 })
            .collect()
    };
    // How much louder the two beats from here are than the two before:
    // sections, drops and the beat coming in all start on a downbeat.
    let loudness: Vec<f32> = beats
        .iter()
        .map(|&t| {
            let from = onset_frame(t).max(0.0) as usize;
            let to = onset_frame(t + gaps[0].max(0.1)).max(0.0) as usize;
            let frames = &bands[from.min(bands.len())..to.min(bands.len())];
            let sum: f32 = frames.iter().map(|b| b[0] + b[1] + b[2]).sum();
            (1e-6 + sum / frames.len().max(1) as f32).ln()
        })
        .collect();
    let jump: Vec<f32> = (0..n)
        .map(|i| {
            if i >= 2 && i + 2 <= n {
                ((loudness[i] + loudness[i + 1]) - (loudness[i - 2] + loudness[i - 1])).max(0.0)
            } else {
                0.0
            }
        })
        .collect();
    let (bass, change, jump) = (standard(&bass), standard(&change), standard(&jump));
    let accent: Vec<f32> = (0..n).map(|i| bass[i] + 1.5 * change[i] + jump[i]).collect();

    // Viterbi over where in the bar each beat falls, 0 being the downbeat.
    let mut score: [f32; 4] = std::array::from_fn(|p| if p == 0 { accent[0] } else { 0.0 });
    let mut back = vec![[0u8; 4]; n];
    for i in 1..n {
        let step = steps[i - 1];
        let top = (0..4).max_by(|&a, &b| score[a].total_cmp(&score[b])).unwrap_or(0);
        let restart = score[top] - RESTART;
        let mut next = [0.0f32; 4];
        for q in 0..4 {
            // Counted on from the beat before, or the count started over.
            let from = (q + 4 * 4 - step) % 4;
            let (best, came) = if score[from] >= restart { (score[from], from) } else { (restart, top) };
            next[q] = best + if q == 0 { accent[i] } else { 0.0 };
            back[i][q] = came as u8;
        }
        score = next;
    }
    let mut p = (0..4).max_by(|&a, &b| score[a].total_cmp(&score[b])).unwrap_or(0);
    let mut place = vec![0usize; n];
    for i in (0..n).rev() {
        place[i] = p;
        p = back[i][p] as usize;
    }
    // A downbeat on each beat counted one, and between beats where the
    // tracker skipped one that would have been.
    let mut out = Vec::new();
    for i in 0..n {
        if place[i] == 0 {
            out.push(beats[i]);
        }
        if i + 1 < n && steps[i] > 1 {
            for k in 1..steps[i] {
                if (place[i] + k) % 4 == 0 {
                    out.push(beats[i] + gaps[i] * k as f32 / steps[i] as f32);
                }
            }
        }
    }
    out
}

/// Which beat of four is the downbeat, counting from `offset`. Bars start
/// where the bass hits hardest and where the chords change.
fn downbeat_phase(
    lows: &[f32],
    chroma: &[[f32; 12]],
    onset_frame: impl Fn(f32) -> f32,
    chroma_time: impl Fn(usize) -> f32,
    offset: f32,
    beat: f32,
    length: f32,
) -> usize {
    // Every grid beat in the song.
    let first = (-offset / beat).ceil() as i64;
    let last = ((length - offset) / beat).floor() as i64;
    if last - first < 16 {
        return 0;
    }
    // Each beat's pitch classes, for the chord changes.
    let mut per_beat: Vec<[f32; 12]> = Vec::new();
    let mut c = 0;
    for n in first..=last {
        let (from, to) = (offset + n as f32 * beat, offset + (n + 1) as f32 * beat);
        let mut sum = [0.0f32; 12];
        while c < chroma.len() && chroma_time(c) < from {
            c += 1;
        }
        let mut k = c;
        while k < chroma.len() && chroma_time(k) < to {
            for p in 0..12 {
                sum[p] += chroma[k][p];
            }
            k += 1;
        }
        per_beat.push(sum);
    }
    let distance = |a: &[f32; 12], b: &[f32; 12]| {
        let dot: f32 = (0..12).map(|p| a[p] * b[p]).sum();
        let size = |v: &[f32; 12]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let both = size(a) * size(b);
        if both > 0.0 { 1.0 - dot / both } else { 0.0 }
    };
    let mut scores = [0.0f32; 4];
    let mut counts = [0usize; 4];
    for (i, n) in (first..=last).enumerate() {
        let phase = n.rem_euclid(4) as usize;
        let at = onset_frame(offset + n as f32 * beat);
        let bass = (-2..=2)
            .filter_map(|d| {
                let f = at as i64 + d;
                (f >= 0).then(|| lows.get(f as usize).copied()).flatten()
            })
            .fold(0.0f32, f32::max);
        // The two beats before against the two from here.
        let change = if i >= 2 && i + 2 <= per_beat.len() {
            let add = |a: &[f32; 12], b: &[f32; 12]| std::array::from_fn(|p| a[p] + b[p]);
            distance(&add(&per_beat[i - 2], &per_beat[i - 1]), &add(&per_beat[i], &per_beat[i + 1]))
        } else {
            0.0
        };
        scores[phase] += bass + 4.0 * change;
        counts[phase] += 1;
    }
    (0..4)
        .max_by(|&a, &b| {
            let mean = |p: usize| scores[p] / counts[p].max(1) as f32;
            mean(a).total_cmp(&mean(b))
        })
        .unwrap_or(0)
}

/// How strong each pitch class is in each pitch frame.
fn chroma_frames(samples: &[f32], rate: f32) -> Vec<[f32; 12]> {
    let bin_hz = rate / CHROMA_FRAME as f32;
    // Each bin's pitch class and weight, over the five whole octaves from
    // C2, so no pitch class gets an octave more than the rest. Bins between
    // two semitones count less.
    let bins: Vec<(usize, usize, f32)> = (1..CHROMA_FRAME / 2)
        .filter_map(|k| {
            let hz = k as f32 * bin_hz;
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            if !(35.5..95.5).contains(&midi) {
                return None;
            }
            let nearest = midi.round();
            let weight = 1.0 - 2.0 * (midi - nearest).abs();
            (weight > 0.0).then_some((k, nearest as usize % 12, weight))
        })
        .collect();
    // Bins are spaced evenly in hertz, so higher notes in each octave get
    // more of them. Each class is the mean over its bins, not the sum.
    let mut share = [0.0f32; 12];
    for &(_, pitch, weight) in &bins {
        share[pitch] += weight;
    }
    let mut frames = Vec::new();
    spectra(samples, CHROMA_FRAME, CHROMA_HOP, |_, magnitude| {
        let mut chroma = [0.0f32; 12];
        for &(k, pitch, weight) in &bins {
            chroma[pitch] += weight * (1.0 + 100.0 * magnitude[k]).ln();
        }
        for p in 0..12 {
            chroma[p] /= share[p].max(1e-6);
        }
        frames.push(chroma);
    });
    frames
}

/// The key whose Krumhansl-Kessler profile best matches the song's pitch
/// classes, and how well it does.
fn find_key(chroma: &[[f32; 12]]) -> (Option<u8>, f32) {
    const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let mut total = [0.0f32; 12];
    for frame in chroma {
        for p in 0..12 {
            total[p] += frame[p];
        }
    }
    if total.iter().all(|&v| v <= 0.0) {
        return (None, 0.0);
    }
    let mut best = (None, f32::MIN);
    for tonic in 0..12 {
        for (mode, profile) in [(0u8, &MAJOR), (12u8, &MINOR)] {
            let rotated: [f32; 12] = std::array::from_fn(|p| profile[(p + 12 - tonic) % 12]);
            let r = correlation(&total, &rotated);
            if r > best.1 {
                best = (Some(mode + tonic as u8), r);
            }
        }
    }
    best
}

fn correlation(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let mean = |v: &[f32; 12]| v.iter().sum::<f32>() / 12.0;
    let (ma, mb) = (mean(a), mean(b));
    let (mut top, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for p in 0..12 {
        let (x, y) = (a[p] - ma, b[p] - mb);
        top += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa > 0.0 && bb > 0.0 { top / (aa * bb).sqrt() } else { 0.0 }
}

/// Every other onset frame's band loudness as bytes, each band scaled to
/// its loudest moment.
fn waveform(bands: &[[f32; 3]]) -> Vec<u8> {
    let mut top = [0.0f32; 3];
    for frame in bands {
        for b in 0..3 {
            top[b] = top[b].max(frame[b]);
        }
    }
    bands
        .chunks(2)
        .flat_map(|pair| {
            (0..3).map(move |b| {
                let peak = pair.iter().map(|f| f[b]).fold(0.0, f32::max);
                if top[b] > 0.0 { ((peak / top[b]).sqrt() * 255.0) as u8 } else { 0 }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 11_025.0;

    /// A kick on every beat, louder on the downbeat, with a chord that
    /// changes each bar, a beat late to start.
    fn drum_loop(bpm: f32, seconds: f32, first_beat: f32) -> Vec<f32> {
        let mut out = vec![0.0f32; (seconds * RATE) as usize];
        let beat = 60.0 / bpm;
        let mut n = 0;
        loop {
            let at = first_beat + n as f32 * beat;
            let start = (at * RATE) as usize;
            if start >= out.len() {
                break;
            }
            let loud = if n % 4 == 0 { 1.0 } else { 0.5 };
            for i in 0..(0.12 * RATE) as usize {
                if let Some(s) = out.get_mut(start + i) {
                    let t = i as f32 / RATE;
                    // A falling sine, as a kick drum sounds.
                    let hz = 50.0 + 100.0 * (-t * 30.0).exp();
                    *s += loud * (-t * 25.0).exp() * (2.0 * std::f32::consts::PI * hz * t).sin();
                }
            }
            n += 1;
        }
        // A minor and C major chords, a bar each.
        let chords = [[220.0, 261.63, 329.63], [261.63, 329.63, 392.0]];
        for (i, s) in out.iter_mut().enumerate() {
            let t = i as f32 / RATE;
            if t < first_beat {
                continue;
            }
            let bar = ((t - first_beat) / (beat * 4.0)) as usize;
            for hz in chords[bar % 2] {
                *s += 0.05 * (2.0 * std::f32::consts::PI * hz * t).sin();
            }
        }
        out
    }

    #[test]
    fn fft_matches_a_plain_dft() {
        let n = 16;
        let input: Vec<f32> = (0..n).map(|i| ((i * 7) % 5) as f32 - 2.0).collect();
        let (mut re, mut im) = (input.clone(), vec![0.0; n]);
        Fft::new(n).run(&mut re, &mut im);
        for k in 0..n {
            let (mut r, mut i) = (0.0f32, 0.0f32);
            for (t, x) in input.iter().enumerate() {
                let a = -2.0 * std::f32::consts::PI * (k * t) as f32 / n as f32;
                r += x * a.cos();
                i += x * a.sin();
            }
            assert!((re[k] - r).abs() < 1e-3 && (im[k] - i).abs() < 1e-3, "bin {k}");
        }
    }

    #[test]
    fn finds_the_tempo_and_the_downbeat_of_a_drum_loop() {
        for (bpm, first) in [(92.0, 0.5), (124.0, 1.37), (128.0, 0.5), (174.0, 2.0)] {
            let a = analyze(&drum_loop(bpm, 60.0, first), RATE);
            assert!((a.bpm - bpm).abs() < 0.1, "found {} for {bpm}", a.bpm);
            assert!(a.steady(), "confidence {} for {bpm}", a.bpm_confidence);
            // On a downbeat, within a few milliseconds.
            let bar = 4.0 * 60.0 / bpm;
            let off = ((a.grid_start - first) / bar).round() * bar + first - a.grid_start;
            assert!(off.abs() < 0.005, "grid starts {} for {bpm}", a.grid_start);
        }
    }

    /// Kicks on `beats`, louder on `downs`, with the chord changing on each.
    fn song_of(beats: &[f32], downs: &[f32], seconds: f32) -> Vec<f32> {
        let mut out = vec![0.0f32; (seconds * RATE) as usize];
        for &at in beats {
            let loud = if downs.iter().any(|d| (d - at).abs() < 1e-3) { 1.0 } else { 0.5 };
            let start = (at * RATE) as usize;
            for i in 0..(0.12 * RATE) as usize {
                if let Some(s) = out.get_mut(start + i) {
                    let t = i as f32 / RATE;
                    let hz = 50.0 + 100.0 * (-t * 30.0).exp();
                    *s += loud * (-t * 25.0).exp() * (2.0 * std::f32::consts::PI * hz * t).sin();
                }
            }
        }
        let chords = [[220.0, 261.63, 329.63], [261.63, 329.63, 392.0]];
        for (i, s) in out.iter_mut().enumerate() {
            let t = i as f32 / RATE;
            let Some(bar) = downs.iter().rposition(|&d| d <= t) else { continue };
            for hz in chords[bar % 2] {
                *s += 0.05 * (2.0 * std::f32::consts::PI * hz * t).sin();
            }
        }
        out
    }

    #[test]
    fn keeps_the_bars_after_a_bar_of_two() {
        // 16 bars of four, one of two, then 16 of four.
        let beat = 0.5f32;
        let beats: Vec<f32> = (0..16 * 4 + 2 + 16 * 4).map(|n| 1.0 + n as f32 * beat).collect();
        let mut downs: Vec<f32> = (0..16).map(|b| beats[b * 4]).collect();
        downs.push(beats[16 * 4]);
        downs.extend((0..16).map(|b| beats[16 * 4 + 2 + b * 4]));
        let a = analyze(&song_of(&beats, &downs, 70.0), RATE);
        assert!((a.bpm - 120.0).abs() < 0.2, "found {}", a.bpm);
        // Each true downbeat after the short bar is found, within 30 ms.
        for &d in downs.iter().filter(|&&d| d > 45.0 && d < 60.0) {
            assert!(a.downbeats.iter().any(|f| (f - d).abs() < 0.03), "missed {d}: {:?}", a.downbeats);
        }
        // And the mix code's grid there agrees.
        let part = a.local(50.0, 58.0).unwrap();
        let d = part.nearest_downbeat(54.0);
        assert!(downs.iter().any(|t| (*t as f64 - d).abs() < 0.03), "local downbeat {d}");
    }

    #[test]
    fn camelot_names_match_the_wheel() {
        let with_key = |key| Analysis {
            length: 0.0,
            bpm: 0.0,
            bpm_confidence: 0.0,
            grid_start: 0.0,
            beats: Vec::new(),
            downbeats: Vec::new(),
            key: Some(key),
            key_confidence: 1.0,
            waveform: Vec::new(),
        };
        assert_eq!(with_key(0).camelot().as_deref(), Some("8B")); // C major
        assert_eq!(with_key(7).camelot().as_deref(), Some("9B")); // G major
        assert_eq!(with_key(21).camelot().as_deref(), Some("8A")); // A minor
        assert_eq!(with_key(16).camelot().as_deref(), Some("9A")); // E minor
        assert_eq!(with_key(21).key_name().as_deref(), Some("A minor"));
    }

    #[test]
    fn finds_a_minor_in_an_a_minor_drone() {
        let notes = [220.0, 261.63, 329.63, 440.0];
        let samples: Vec<f32> = (0..(20.0 * RATE) as usize)
            .map(|i| {
                let t = i as f32 / RATE;
                notes.iter().map(|hz| 0.1 * (2.0 * std::f32::consts::PI * hz * t).sin()).sum()
            })
            .collect();
        let (key, _) = find_key(&chroma_frames(&samples, RATE));
        // A minor, or C major, its relative, which shares every note.
        assert!(matches!(key, Some(21) | Some(0)), "found {key:?}");
    }
}

/// Analyzes every song in `TONEARM_ANALYZE_DIR` and prints what it finds:
/// `cargo test analyze_folder -- --ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore]
fn analyze_folder() {
    let Some(dir) = std::env::var_os("TONEARM_ANALYZE_DIR") else {
        return;
    };
    let mut paths: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths.iter().filter(|p| p.extension().is_some_and(|e| e == "m4a")).take(40) {
        let started = std::time::Instant::now();
        let file = std::fs::File::open(path).unwrap();
        match analyze_source(Box::new(file)) {
            Ok(a) => eprintln!(
                "{:<14} {:>6.2} BPM  steady {:.2}  grid {:>6.2}s  beats {:>4}  key {:<10} {:<4} ({:.2})  {:.1}s",
                path.file_stem().unwrap().to_string_lossy(),
                a.bpm,
                a.bpm_confidence,
                a.grid_start,
                a.beats.len(),
                a.key_name().unwrap_or_default(),
                a.camelot().unwrap_or_default(),
                a.key_confidence,
                started.elapsed().as_secs_f32(),
            ),
            Err(e) => eprintln!("{}: {e}", path.display()),
        }
    }
}

