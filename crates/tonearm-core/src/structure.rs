//! How a song is built, for mixing at musical points: where its music
//! really starts, where a slow fade or silence begins at the end, and where
//! its sections (verses, choruses, drops, bridges) start.
//!
//! A mix that starts or ends in the middle of a section leaves the song
//! sounding unfinished, so mixes are placed on section starts, found from
//! where the sound changes rather than from counting bars, which a rest or
//! a held beat throws off. All of this is worked out from what the analyzer
//! already stored (the beats and the loudness of each band), so songs
//! analyzed long ago need no new analysis.

use crate::analysis::Analysis;

/// Seconds a loudness window covers.
const WINDOW: f64 = 0.5;
/// The music has started once it is this share of the song's usual loudness
/// (in the waveform's scale, which is the square root of the amplitude).
const AUDIBLE: f32 = 0.35;
/// The end has begun to fade once it is below this share.
const FADED: f32 = 0.5;
/// A quiet end shorter than this is just how the song ends, not a fade
/// to skip.
const LONGEST_NATURAL_END: f64 = 6.0;

/// Where a song's music starts and ends, in its seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edges {
    /// Where the music starts, after any silence or near silence. 0 when
    /// it starts at once.
    pub start: f64,
    /// Where the song starts to fade or go quiet for good, when that takes
    /// long enough to be worth mixing before. None when it ends at full
    /// strength or only briefly fades.
    pub tail: Option<f64>,
}

/// The loudness of each half second, 0 to 1, or nothing without a waveform.
fn loudness(a: &Analysis) -> Vec<f32> {
    let frames = a.waveform.len() / 3;
    if frames < 8 || a.length <= 0.0 {
        return Vec::new();
    }
    let per_window = ((frames as f64 / a.length as f64) * WINDOW).round().max(1.0) as usize;
    a.waveform[..frames * 3]
        .chunks(3)
        .map(|f| f.iter().map(|&b| b as f32).sum::<f32>() / (3.0 * 255.0))
        .collect::<Vec<_>>()
        .chunks(per_window)
        .map(|w| w.iter().sum::<f32>() / w.len() as f32)
        .collect()
}

/// Where the music starts and where a fade or silence begins at the end.
pub fn edges(a: &Analysis) -> Edges {
    let loud = loudness(a);
    if loud.len() < 8 {
        return Edges { start: 0.0, tail: None };
    }
    // The song's usual loudness: what its louder part sits at.
    let mut sorted = loud.clone();
    sorted.sort_by(f32::total_cmp);
    let body = sorted[sorted.len() * 7 / 10];
    if body <= 0.0 {
        return Edges { start: 0.0, tail: None };
    }
    let seconds = |windows: usize| windows as f64 * WINDOW * (a.length as f64 / (loud.len() as f64 * WINDOW));
    // Two windows in a row, so a click in the silence does not count.
    let start = (0..loud.len() - 1)
        .find(|&i| loud[i] >= body * AUDIBLE && loud[i + 1] >= body * AUDIBLE)
        .map_or(0.0, seconds);
    let start = if start < 1.0 { 0.0 } else { start };
    let tail = loud
        .iter()
        .rposition(|&l| l >= body * FADED)
        .map(|last| seconds(last + 1))
        .filter(|&t| a.length as f64 - t >= LONGEST_NATURAL_END);
    Edges { start, tail }
}

/// Where a new section of a song starts, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Boundary {
    pub time: f64,
    /// How sure it is, 0 to 1.
    pub strength: f32,
}

/// Beats that make a long and a short look at the song either side.
const LONG: usize = 8;
const SHORT: usize = 4;
/// A boundary is this strong at least, and the strongest in this many
/// beats either side.
const FIRM: f32 = 0.35;
/// The change in loudness, summed over three bands and as a share of the
/// loudest, that the strongest boundaries are measured against at least.
const MIN_CHANGE: f32 = 0.25;
const APART: usize = 6;
/// Sections of a song are usually 8 or 16 bars, so boundaries that fall a
/// whole number of 4 bar phrases from each other back each other up.
const PHRASE_BEATS: usize = 16;

fn mean(cum: &[[f32; 3]], lo: usize, hi: usize) -> [f32; 3] {
    let n = (hi - lo).max(1) as f32;
    std::array::from_fn(|b| (cum[hi][b] - cum[lo][b]) / n)
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).sum()
}

/// Where the song changes section: a drop, a verse or chorus starting, a
/// bridge. Found from how the loudness of the low, middle and high bands
/// changes from the beats before to the beats after each beat, with a
/// drum fill just before it (a burst or a gap against the bars before)
/// counting as a sign. Sorted by time. Empty when the song has no beats
/// or waveform to go on.
pub fn boundaries(a: &Analysis) -> Vec<Boundary> {
    let frames = a.waveform.len() / 3;
    let n = a.beats.len();
    if frames < 8 || n < 4 * LONG || a.length <= 0.0 {
        return Vec::new();
    }
    let fps = frames as f64 / a.length as f64;
    let at = |i: usize| ((a.beats[i.min(n - 1)] as f64 * fps) as usize).min(frames - 1);
    // Each beat's loudness in the three bands, summed up for quick windows.
    let mut cum = vec![[0.0f32; 3]; n + 1];
    for i in 0..n {
        let from = at(i);
        let to = if i + 1 < n { at(i + 1) } else { (from + (0.5 * fps) as usize).min(frames) }.max(from + 1).min(frames);
        let mut sum = [0.0f32; 3];
        for f in from..to {
            for b in 0..3 {
                sum[b] += a.waveform[f * 3 + b] as f32 / 255.0;
            }
        }
        let count = (to - from).max(1) as f32;
        for b in 0..3 {
            cum[i + 1][b] = cum[i][b] + sum[b] / count;
        }
    }
    let mut score = vec![0.0f32; n];
    for i in SHORT..n - SHORT {
        let mut s = 0.0;
        for (w, weight) in [(LONG, 1.0), (SHORT, 0.6)] {
            let w = w.min(i).min(n - i);
            s += weight * distance(mean(&cum, i - w, i), mean(&cum, i, i + w));
        }
        // A fill: the beat or two before is unlike the bars before that.
        if i >= LONG {
            s += 0.4 * distance(mean(&cum, i - 2, i), mean(&cum, i - LONG, i - 2));
        }
        score[i] = s;
    }
    let mut sorted: Vec<f32> = score.iter().copied().filter(|&s| s > 0.0).collect();
    sorted.sort_by(f32::total_cmp);
    let Some(&top) = sorted.get(sorted.len() * 9 / 10) else {
        return Vec::new();
    };
    // A song that hardly changes has nothing to find, however its noise
    // looks next to itself.
    let top = top.max(MIN_CHANGE);
    let mut found: Vec<(usize, f32)> = (SHORT..n - SHORT)
        .filter(|&i| {
            let around = &score[i.saturating_sub(APART)..(i + APART + 1).min(n)];
            score[i] / top >= FIRM && score[i] >= around.iter().copied().fold(0.0, f32::max)
        })
        .map(|i| (i, (score[i] / top).min(1.0)))
        .collect();
    // Backed up by others a whole number of phrases away.
    let beats: Vec<usize> = found.iter().map(|f| f.0).collect();
    for f in &mut found {
        let backing = beats
            .iter()
            .filter(|&&o| {
                let d = o.abs_diff(f.0);
                d > 0 && (d % PHRASE_BEATS <= 1 || d % PHRASE_BEATS == PHRASE_BEATS - 1)
            })
            .count();
        f.1 = (f.1 + 0.1 * backing.min(3) as f32).min(1.0);
    }
    found.into_iter().map(|(i, strength)| Boundary { time: a.beats[i] as f64, strength }).collect()
}

/// `t` moved to the nearest bar start, when one is within `reach` seconds:
/// a section starts on a bar, and the sound changes a beat early or late of
/// it (a fill, a crash) often enough that the bars are the better guide.
pub fn on_a_bar(a: &Analysis, t: f64, reach: f64) -> f64 {
    a.downbeats
        .iter()
        .map(|&d| d as f64)
        .min_by(|x, y| (x - t).abs().total_cmp(&(y - t).abs()))
        .filter(|d| (d - t).abs() <= reach)
        .unwrap_or(t)
}

/// Every firm enough section start from `from` to `to` seconds, in order.
pub fn section_starts(a: &Analysis, firm: f32, from: f64, to: f64) -> Vec<f64> {
    boundaries(a)
        .into_iter()
        .filter(|b| b.strength >= firm)
        .map(|b| on_a_bar(a, b.time, 1.5 * a.beat()))
        .filter(|t| (from..=to).contains(t))
        .collect()
}

/// The first section start from `from` to `to` seconds that is firm enough.
pub fn first_boundary(a: &Analysis, from: f64, to: f64, firm: f32) -> Option<f64> {
    section_starts(a, firm, from, to).into_iter().next()
}

/// The last section start from `from` to `to` seconds that is firm enough.
pub fn last_boundary(a: &Analysis, from: f64, to: f64, firm: f32) -> Option<f64> {
    section_starts(a, firm, from, to).into_iter().last()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f64 = 43.0;

    /// A song `seconds` long whose loudness at each time is `level(t)`.
    fn song(seconds: f64, downbeats: Vec<f32>, level: impl Fn(f64) -> f32) -> Analysis {
        let frames = (seconds * FPS) as usize;
        let waveform = (0..frames)
            .flat_map(|f| {
                let l = (level(f as f64 / FPS).clamp(0.0, 1.0) * 255.0) as u8;
                [l, l, l]
            })
            .collect();
        Analysis {
            length: seconds as f32,
            bpm: 120.0,
            bpm_confidence: 0.9,
            grid_start: downbeats.first().copied().unwrap_or(0.0),
            beats: Vec::new(),
            downbeats,
            key: None,
            key_confidence: 0.0,
            waveform,
        }
    }

    /// A bar every 2 seconds (120 bpm) from `first`.
    fn bars(first: f32, count: usize) -> Vec<f32> {
        (0..count).map(|i| first + 2.0 * i as f32).collect()
    }

    #[test]
    fn a_silent_start_is_skipped_and_a_song_that_starts_at_once_is_not() {
        let quiet = song(120.0, bars(0.0, 60), |t| if t < 9.0 { 0.0 } else { 0.8 });
        let e = edges(&quiet);
        assert!((e.start - 9.0).abs() <= 0.6, "start {}", e.start);
        assert_eq!(edges(&song(120.0, bars(0.0, 60), |_| 0.8)).start, 0.0);
    }

    #[test]
    fn a_slow_fade_out_is_found_and_a_short_one_is_not() {
        let fade = |from: f64, to: f64| move |t: f64| if t < from { 0.8 } else { (0.8 * (to - t) / (to - from)).max(0.0) as f32 };
        let slow = edges(&song(120.0, bars(0.0, 60), fade(80.0, 120.0)));
        let tail = slow.tail.expect("a 40 second fade is a tail");
        assert!((80.0..105.0).contains(&tail), "tail {tail}");
        assert!(edges(&song(120.0, bars(0.0, 60), fade(116.0, 120.0))).tail.is_none());
        assert!(edges(&song(120.0, bars(0.0, 60), |_| 0.8)).tail.is_none());
    }

    #[test]
    fn silence_after_the_song_counts_as_a_tail() {
        let e = edges(&song(120.0, bars(0.0, 60), |t| if t < 90.0 { 0.8 } else { 0.0 }));
        assert!((e.tail.unwrap() - 90.0).abs() <= 0.6);
    }

    /// A song of 120 bpm with beats all through, whose loudness at each
    /// time is `level`.
    fn beaty(seconds: f64, level: impl Fn(f64) -> f32) -> Analysis {
        let mut a = song(seconds, bars(0.0, (seconds / 2.0) as usize), level);
        a.beats = (0..(seconds / 0.5) as usize).map(|i| i as f32 * 0.5).collect();
        a
    }

    #[test]
    fn a_drop_is_a_section_start() {
        // Quiet for 20 seconds, then loud, then quiet again from 60.
        let a = beaty(100.0, |t| if (20.0..60.0).contains(&t) { 0.9 } else { 0.3 });
        let found = boundaries(&a);
        let near = |t: f64| found.iter().any(|b| (b.time - t).abs() <= 1.0 && b.strength > 0.5);
        assert!(near(20.0) && near(60.0), "{found:?}");
        assert!(found.iter().all(|b| (b.time - 20.0).abs() <= 3.0 || (b.time - 60.0).abs() <= 3.0), "{found:?}");
        assert_eq!(first_boundary(&a, 0.0, 100.0, 0.5).map(|t| t.round()), Some(20.0));
        assert_eq!(last_boundary(&a, 0.0, 100.0, 0.5).map(|t| t.round()), Some(60.0));
    }

    #[test]
    fn an_even_song_has_no_sections() {
        assert!(boundaries(&beaty(100.0, |_| 0.6)).is_empty());
        assert!(boundaries(&song(100.0, Vec::new(), |_| 0.6)).is_empty());
    }
}
