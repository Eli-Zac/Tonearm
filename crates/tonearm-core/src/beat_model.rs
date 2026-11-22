//! Finds which beat of each bar is the first, with a neural network.
//!
//! Beat tracking from the sound's rhythm gets the tempo right, but nothing
//! in it says which of four beats is beat one, and a mix that lands beat
//! four on beat one sounds wrong even with every beat in time. Beat This!
//! (Foscarin, Schlüter and Widmer, ISMIR 2024, MIT license) is trained on
//! that. It runs here through ONNX Runtime.
//!
//! The model's weights are the official `final0` checkpoint of Beat This!
//! (https://github.com/CPJKU/beat_this, MIT), exported to ONNX with only the
//! matrix multiplications quantized to 8 bits, which keeps it to 23 MB and
//! finds the same beats and downbeats as the full model.
//!
//! The model is only used for where the bars start. Its tempo is not: it
//! reads some songs at half speed, so its bars can be two of ours long. The
//! analyzer's own beats are kept, and the bars are put on them.

use crate::analysis::Fft;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::{Mutex, OnceLock};

/// The model's input rate, and the spectrogram's frame size, hop and bands.
const RATE: f32 = 22_050.0;
const FRAME: usize = 1024;
const HOP: usize = 441;
const BANDS: usize = 128;
const FRAMES_A_SECOND: f32 = RATE / HOP as f32;
/// Frames run through the model at once, and the frames at each side of a
/// chunk that are left out, as the model sees too little there.
const CHUNK: usize = 1500;
const BORDER: usize = 6;
/// A peak is the highest of this many frames.
const PEAK_WINDOW: usize = 7;
/// Fewest beats that make a grid.
const FEWEST_BEATS: usize = 8;

static MODEL: &[u8] = include_bytes!("../../../assets/models/beat_this_q8.onnx");

struct Loaded {
    // Kept so the runtime outlives its session.
    _environment: ort::environment::Environment,
    session: Session,
}

fn session() -> Option<&'static Mutex<Loaded>> {
    static SESSION: OnceLock<Option<Mutex<Loaded>>> = OnceLock::new();
    SESSION
        .get_or_init(|| {
            let load = || -> Result<Loaded, ort::Error> {
                let environment = ort::init().with_name("tonearm").build()?;
                // Two threads: it runs in the background while songs play.
                let session = Session::builder(&environment)?.with_intra_threads(2)?.commit_from_memory(MODEL)?;
                Ok(Loaded { _environment: environment, session })
            };
            load().map_err(|e| eprintln!("beat model: {e}")).ok().map(Mutex::new)
        })
        .as_ref()
}

// ------------------------------------------------------------ front end

fn hz_to_mel(hz: f64) -> f64 {
    let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
    let (min_log_mel, logstep) = (min_log_hz / f_sp, 6.4f64.ln() / 27.0);
    if hz >= min_log_hz { min_log_mel + (hz / min_log_hz).ln() / logstep } else { hz / f_sp }
}

fn mel_to_hz(mel: f64) -> f64 {
    let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
    let (min_log_mel, logstep) = (min_log_hz / f_sp, 6.4f64.ln() / 27.0);
    if mel >= min_log_mel { min_log_hz * (logstep * (mel - min_log_mel)).exp() } else { f_sp * mel }
}

/// Triangular filters on the mel scale from 30 Hz to 11 kHz, each from its
/// first bin and with its weight at each bin.
fn filterbank() -> Vec<(usize, Vec<f32>)> {
    let bins = FRAME / 2 + 1;
    let (lo, hi) = (hz_to_mel(30.0), hz_to_mel(11_000.0));
    let edges: Vec<f64> = (0..BANDS + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (BANDS + 1) as f64)).collect();
    let to_bin = |hz: f64| hz * FRAME as f64 / RATE as f64;
    (0..BANDS)
        .map(|m| {
            let (left, centre, right) = (edges[m], edges[m + 1], edges[m + 2]);
            let first = (to_bin(left).floor().max(0.0)) as usize;
            let last = (to_bin(right).ceil() as usize).min(bins - 1);
            let weights = (first..=last)
                .map(|b| {
                    let hz = b as f64 * RATE as f64 / FRAME as f64;
                    let rising = if centre > left { (hz - left) / (centre - left) } else { 0.0 };
                    let falling = if right > centre { (right - hz) / (right - centre) } else { 0.0 };
                    rising.min(falling).max(0.0) as f32
                })
                .collect();
            (first, weights)
        })
        .collect()
}

/// The song at `RATE`, from mono samples at `rate`. A windowed sinc, which
/// keeps what is above the new rate out of what is below.
fn resample(samples: &[f32], rate: f32) -> Vec<f32> {
    if (rate - RATE).abs() < 0.5 {
        return samples.to_vec();
    }
    let ratio = rate as f64 / RATE as f64;
    // Narrower than the input's band when going down, so nothing folds back.
    let cutoff = (1.0 / ratio).min(1.0);
    const TAPS: i64 = 12;
    let out_len = (samples.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let centre = n as f64 * ratio;
        let from = (centre.floor() as i64 - TAPS + 1).max(0);
        let to = (centre.floor() as i64 + TAPS).min(samples.len() as i64 - 1);
        let (mut sum, mut weight) = (0.0f64, 0.0f64);
        for i in from..=to {
            let x = (i as f64 - centre) * cutoff;
            let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
            let window = 0.5 + 0.5 * (std::f64::consts::PI * (i as f64 - centre) / TAPS as f64).cos();
            let w = sinc * window * cutoff;
            sum += samples[i as usize] as f64 * w;
            weight += w;
        }
        out.push(if weight.abs() > 1e-9 { (sum / weight) as f32 } else { 0.0 });
    }
    out
}

/// The log mel spectrogram the model reads: frames of `BANDS` values.
fn spectrogram(samples: &[f32]) -> (Vec<f32>, usize) {
    let pad = FRAME / 2;
    if samples.len() <= pad + 1 {
        return (Vec::new(), 0);
    }
    // Reflected at both ends, as torch.stft pads with center on.
    let mut padded = Vec::with_capacity(samples.len() + 2 * pad);
    padded.extend((0..pad).map(|i| samples[pad - i]));
    padded.extend_from_slice(samples);
    padded.extend((1..=pad).map(|i| samples[samples.len() - 1 - i]));
    let frames = (padded.len() - FRAME) / HOP + 1;
    let fft = Fft::new(FRAME);
    let window: Vec<f32> = (0..FRAME).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos()).collect();
    let filters = filterbank();
    let norm = (FRAME as f32).sqrt();
    let mut values = vec![0.0f32; frames * BANDS];
    let (mut re, mut im) = (vec![0.0f32; FRAME], vec![0.0f32; FRAME]);
    let mut magnitude = vec![0.0f32; FRAME / 2 + 1];
    for f in 0..frames {
        let start = f * HOP;
        for i in 0..FRAME {
            re[i] = padded[start + i] * window[i];
            im[i] = 0.0;
        }
        fft.run(&mut re, &mut im);
        for (b, m) in magnitude.iter_mut().enumerate() {
            *m = re[b].hypot(im[b]) / norm;
        }
        for (band, (first, weights)) in filters.iter().enumerate() {
            let e: f32 = weights.iter().enumerate().map(|(k, w)| magnitude[first + k] * w).sum();
            values[f * BANDS + band] = (1000.0 * e.max(1e-10)).ln_1p();
        }
    }
    (values, frames)
}

// ---------------------------------------------------------------- model

/// The model's beat and downbeat logits for each frame.
fn run(spec: &[f32], frames: usize, progress: &dyn Fn(f32)) -> Option<(Vec<f32>, Vec<f32>)> {
    let mut loaded = session()?.lock().unwrap_or_else(|e| e.into_inner());
    let (mut beat, mut down) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let stride = CHUNK - 2 * BORDER;
    let mut start = 0;
    while start < frames {
        let length = CHUNK.min(frames - start);
        if length <= 2 * BORDER && start > 0 {
            break;
        }
        let chunk = spec[start * BANDS..(start + length) * BANDS].to_vec();
        let input = Tensor::from_array(([1usize, length, BANDS], chunk)).map_err(|e| eprintln!("beat model: {e}")).ok()?;
        let out = loaded.session.run(ort::inputs![input]).map_err(|e| eprintln!("beat model: {e}")).ok()?;
        let (_, b) = out[0].try_extract_tensor::<f32>().map_err(|e| eprintln!("beat model: {e}")).ok()?;
        let (_, d) = out[1].try_extract_tensor::<f32>().map_err(|e| eprintln!("beat model: {e}")).ok()?;
        let keep_from = if start == 0 { 0 } else { BORDER };
        let keep_to = if start + length >= frames { length } else { length - BORDER };
        for i in keep_from..keep_to {
            beat[start + i] = b[i];
            down[start + i] = d[i];
        }
        progress((start + length) as f32 / frames as f32);
        if start + length >= frames {
            break;
        }
        start += stride;
    }
    Some((beat, down))
}

/// Frames where the logits peak above zero: the highest of seven, ties
/// merged, and refined to a fraction of a frame.
fn peaks(logits: &[f32]) -> Vec<f32> {
    let half = PEAK_WINDOW / 2;
    let mut found = Vec::new();
    for i in 0..logits.len() {
        if logits[i] <= 0.0 {
            continue;
        }
        let around = &logits[i.saturating_sub(half)..(i + half + 1).min(logits.len())];
        if around.iter().all(|&x| x <= logits[i]) {
            found.push(i);
        }
    }
    let mut merged = Vec::new();
    let mut i = 0;
    while i < found.len() {
        let (mut mean, mut count) = (found[i] as f64, 1.0);
        while i + 1 < found.len() && found[i + 1] as f64 - mean <= 1.0 {
            i += 1;
            count += 1.0;
            mean += (found[i] as f64 - mean) / count;
        }
        merged.push(mean.round() as usize);
        i += 1;
    }
    merged
        .into_iter()
        .map(|f| {
            if f == 0 || f + 1 >= logits.len() {
                return f as f32;
            }
            let (l, c, r) = (logits[f - 1], logits[f], logits[f + 1]);
            let d = l - 2.0 * c + r;
            if d.abs() <= 1e-9 { f as f32 } else { f as f32 + (0.5 * (l - r) / d).clamp(-0.5, 0.5) }
        })
        .collect()
}

/// Where the bars start, in seconds, from mono samples at `rate` and the
/// analyzer's tracked `beats`. Every bar start is one of the beats: the
/// model's downbeats are moved to the nearest beat, and the bars go on four
/// beats at a time from each until the next, and back to the first beat.
/// None when the model is not there or finds too little. `progress` hears
/// how far it has got, 0 to 1.
pub fn bars(samples: &[f32], rate: f32, beats: &[f32], progress: &dyn Fn(f32)) -> Option<Vec<f32>> {
    if beats.len() < FEWEST_BEATS {
        return None;
    }
    let (spec, frames) = spectrogram(&resample(samples, rate));
    if frames < 2 * BORDER + 1 {
        return None;
    }
    progress(0.1);
    let (_, down) = run(&spec, frames, &|x| progress(0.1 + 0.9 * x))?;
    let model: Vec<f32> = peaks(&down).into_iter().map(|f| f / FRAMES_A_SECOND).collect();
    on_beats(&model, beats)
}

/// The bar starts, given the model's downbeats and the tracked beats.
fn on_beats(model: &[f32], beats: &[f32]) -> Option<Vec<f32>> {
    let mut index: Vec<usize> = model
        .iter()
        .map(|&t| {
            beats
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1 - t).abs().total_cmp(&(b.1 - t).abs()))
                .map_or(0, |(i, _)| i)
        })
        .collect();
    index.sort_unstable();
    index.dedup();
    if index.len() < 2 {
        return None;
    }
    let mut starts: Vec<usize> = Vec::new();
    // Back from the first to the first beat.
    let mut i = index[0];
    while i >= 4 {
        i -= 4;
    }
    starts.extend((i..index[0]).step_by(4));
    for (k, &from) in index.iter().enumerate() {
        let to = index.get(k + 1).copied().unwrap_or(beats.len());
        starts.extend((from..to).step_by(4));
    }
    Some(starts.into_iter().map(|i| beats[i]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_go_on_four_beats_at_a_time_from_each_downbeat() {
        // A beat every half second, the model's downbeats at beats 2 and 11.
        let beats: Vec<f32> = (0..24).map(|i| i as f32 * 0.5).collect();
        let bars = on_beats(&[1.02, 5.48], &beats).unwrap();
        let at: Vec<usize> = bars.iter().map(|b| (b * 2.0).round() as usize).collect();
        // Back from beat 2 to beat 2 (nothing before it), then 2, 6, 10, then
        // from beat 11: 11, 15, 19, 23.
        assert_eq!(at, vec![2, 6, 10, 11, 15, 19, 23]);
    }

    #[test]
    fn the_first_beats_before_the_first_downbeat_get_bars_too() {
        let beats: Vec<f32> = (0..30).map(|i| i as f32 * 0.5).collect();
        let bars = on_beats(&[4.0, 12.0], &beats).unwrap();
        let at: Vec<usize> = bars.iter().map(|b| (b * 2.0).round() as usize).collect();
        assert_eq!(&at[..3], &[0, 4, 8]);
    }

    #[test]
    fn a_peak_is_the_top_of_its_neighbourhood_above_zero() {
        let mut logits = vec![-1.0f32; 40];
        logits[10] = 2.0;
        logits[11] = 1.0;
        logits[30] = 3.0;
        logits[20] = -0.5;
        let found = peaks(&logits);
        assert_eq!(found.len(), 2);
        assert!((found[0] - 10.0).abs() < 0.6 && (found[1] - 30.0).abs() < 0.6, "{found:?}");
    }

    #[test]
    fn resampling_keeps_a_tone_at_its_pitch() {
        let rate = 48_000.0;
        let tone: Vec<f32> = (0..48_000).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate).sin()).collect();
        let out = resample(&tone, rate);
        assert!((out.len() as i64 - 22_050).abs() <= 2, "{}", out.len());
        // Zero crossings give the frequency.
        let crossings = out.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((crossings as i64 - 440).abs() <= 2, "{crossings} cycles");
    }

    /// Runs the whole analyzer on a song and compares its bars with a JSON
    /// file of {"downbeats": [seconds]} from the Python version:
    /// `BEAT_FILE=<m4a> BEAT_JSON=<json> cargo test --features beat-model
    /// matches_python -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn matches_python() {
        let (file, json) = (std::env::var("BEAT_FILE").unwrap(), std::env::var("BEAT_JSON").unwrap());
        let started = std::time::Instant::now();
        let a = crate::analysis::analyze_file(std::path::Path::new(&file)).unwrap();
        println!("analyzed in {:.1}s: {:.2} BPM, {} beats, {} bars", started.elapsed().as_secs_f32(), a.bpm, a.beats.len(), a.downbeats.len());
        let want: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(json).unwrap()).unwrap();
        let want: Vec<f32> = want["downbeats"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
        let same = a.downbeats.iter().filter(|d| want.iter().any(|w| (w - **d).abs() < 0.03)).count();
        println!("{same} of {} bars are where Python put them ({} there)", a.downbeats.len(), want.len());
    }
}

