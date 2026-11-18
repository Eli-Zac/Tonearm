//! Transition styles: how one song mixes into the next.
//!
//! A transition is set by a few choices, like Spotify's editor: how the
//! volumes cross, what the EQ does, a filter sweep, and whether the songs
//! are beat matched. Presets are named sets of those choices. Auto picks a
//! preset from the two songs' analyses.
//!
//! `plan` turns the choices and the two songs into a `mix::Transition`:
//! when it starts, where the next song starts from, and its lanes. The
//! mixer only ever plays those.

use crate::analysis::Analysis;
use crate::mix::{Curve, Lane, Param, Side, Transition};

/// How the two songs' volumes cross.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Volume {
    /// Equal power across the whole mix.
    Smooth,
    /// The new song comes up over the first half, and the old one goes
    /// over the second, so both play full in the middle. Wants a bass swap
    /// under it, or the low end doubles.
    Overlap,
    /// A straight swap on the downbeat.
    Cut,
}

/// What the EQ does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eq {
    None,
    /// The new song's bass stays out until the middle, then takes over
    /// from the old song's in one beat. Two kick drums never play at once.
    BassSwap,
}

/// A filter sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    None,
    /// The old song thins out from the bottom up, as a high pass rises.
    Rise,
    /// The new song opens up from muffled, as a low pass rises.
    Open,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Chosen from the songs: Blend when they beat match, Fade otherwise.
    Auto,
    Fade,
    Blend,
    Rise,
    Cut,
    /// Changed by hand from a preset.
    Custom,
}

impl Preset {
    /// The presets offered, in order. Custom is not picked, it happens.
    pub const OFFERED: [Preset; 5] = [Preset::Auto, Preset::Fade, Preset::Blend, Preset::Rise, Preset::Cut];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Auto => "Auto",
            Preset::Fade => "Fade",
            Preset::Blend => "Blend",
            Preset::Rise => "Rise",
            Preset::Cut => "Cut",
            Preset::Custom => "Custom",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Preset::Auto => "auto",
            Preset::Fade => "fade",
            Preset::Blend => "blend",
            Preset::Rise => "rise",
            Preset::Cut => "cut",
            Preset::Custom => "custom",
        }
    }

    fn from_key(key: &str) -> Option<Preset> {
        [Preset::Auto, Preset::Fade, Preset::Blend, Preset::Rise, Preset::Cut, Preset::Custom]
            .into_iter()
            .find(|p| p.key() == key)
    }
}

/// Every choice that makes a transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub preset: Preset,
    pub volume: Volume,
    pub eq: Eq,
    pub filter: Filter,
    pub beatmatch: bool,
    /// Seconds the mix moves within the song going out, later when above
    /// zero, from where it would be. Dragged in the editor. Whole bars when
    /// the songs beat match.
    pub out_shift: f64,
    /// Seconds the song coming in starts later in itself, from its first
    /// downbeat, or from its top when not beat matched.
    pub in_shift: f64,
}

pub const VOLUME_CHOICES: [(Volume, &str); 3] = [
    (Volume::Smooth, "Smooth crossfade"),
    (Volume::Overlap, "Overlap"),
    (Volume::Cut, "Cut on the beat"),
];
pub const EQ_CHOICES: [(Eq, &str); 2] = [(Eq::None, "None"), (Eq::BassSwap, "Bass swap")];
pub const FILTER_CHOICES: [(Filter, &str); 3] = [
    (Filter::None, "None"),
    (Filter::Rise, "Rise out"),
    (Filter::Open, "Open in"),
];

impl Settings {
    /// A preset's choices. Auto's are Blend's until the songs say otherwise.
    pub fn preset(preset: Preset) -> Settings {
        let (volume, eq, filter, beatmatch) = match preset {
            Preset::Fade => (Volume::Smooth, Eq::None, Filter::None, true),
            Preset::Rise => (Volume::Overlap, Eq::BassSwap, Filter::Rise, true),
            Preset::Cut => (Volume::Cut, Eq::None, Filter::None, true),
            Preset::Auto | Preset::Blend | Preset::Custom => (Volume::Overlap, Eq::BassSwap, Filter::None, true),
        };
        Settings { preset, volume, eq, filter, beatmatch, out_shift: 0.0, in_shift: 0.0 }
    }

    /// What Auto becomes for these two songs. Songs that beat match get a
    /// blend with a bass swap. Others get a smooth fade, since a bass swap
    /// on drums that do not line up sounds like a mistake.
    pub fn auto(out: Option<&Analysis>, inn: Option<&Analysis>, beat_match: bool) -> Settings {
        let matched = beat_match
            && match (out, inn) {
                (Some(a), Some(b)) => timing(a, f64::MAX, b, 8.0).is_some(),
                _ => false,
            };
        let mut settings = if matched {
            Settings::preset(Preset::Blend)
        } else {
            let mut fade = Settings::preset(Preset::Fade);
            fade.beatmatch = beat_match;
            fade
        };
        settings.preset = Preset::Auto;
        settings
    }

    /// The label on the chip between two songs.
    pub fn label(&self) -> &'static str {
        self.preset.name()
    }

    pub fn to_json(&self) -> String {
        serde_json::json!({
            "preset": self.preset.key(),
            "volume": index_of(&VOLUME_CHOICES, self.volume),
            "eq": index_of(&EQ_CHOICES, self.eq),
            "filter": index_of(&FILTER_CHOICES, self.filter),
            "beatmatch": self.beatmatch,
            "out_shift": self.out_shift,
            "in_shift": self.in_shift,
        })
        .to_string()
    }

    pub fn from_json(text: &str) -> Option<Settings> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let pick = |key: &str, len: usize| v[key].as_u64().map(|i| i as usize).filter(|&i| i < len);
        Some(Settings {
            preset: Preset::from_key(v["preset"].as_str()?)?,
            volume: VOLUME_CHOICES[pick("volume", VOLUME_CHOICES.len())?].0,
            eq: EQ_CHOICES[pick("eq", EQ_CHOICES.len())?].0,
            filter: FILTER_CHOICES[pick("filter", FILTER_CHOICES.len())?].0,
            beatmatch: v["beatmatch"].as_bool()?,
            // Saved before they could be dragged: none.
            out_shift: v["out_shift"].as_f64().unwrap_or(0.0),
            in_shift: v["in_shift"].as_f64().unwrap_or(0.0),
        })
    }
}

pub fn index_of<T: PartialEq + Copy>(choices: &[(T, &str)], value: T) -> usize {
    choices.iter().position(|(c, _)| *c == value).unwrap_or(0)
}

// ------------------------------------------------------------------ timing

/// The most a song is sped up or slowed down to match the other, as a
/// share. Past about 6% the change of pitch starts to stand out.
const MOST_STRETCH: f64 = 0.06;
/// A song whose beat starts later than this has a long intro with no beat.
/// Skipping all of it would lose too much, so it fades in instead.
const LATEST_FIRST_BEAT: f64 = 30.0;

/// How long a cut takes, so the swap does not click.
const CUT: f64 = 0.02;

/// When a beat matched mix happens.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Timing {
    out_at: f64,
    in_from: f64,
    length: f64,
    /// The speed the new song plays at to match, and the old song's beat.
    speed: f64,
    beat: f64,
}

/// Beat matched timing from `out`, `out_length` seconds long, into `inn`,
/// about `seconds` long. None when the two cannot be beat matched: a tempo
/// that wanders, or tempos too far apart.
///
/// The mix starts on a downbeat of the song going out, as late as lets it
/// finish before that song's beat stops. The song coming in starts from its
/// first downbeat, at the old song's tempo, so beats and bars land together.
fn timing(out: &Analysis, out_length: f64, inn: &Analysis, seconds: f64) -> Option<Timing> {
    if !out.steady() || !inn.steady() {
        return None;
    }
    let out_beat = out.beat();
    // A song at half or twice the tempo matches beat for beat.
    let speed = [0.5, 1.0, 2.0]
        .into_iter()
        .map(|m| out.bpm as f64 / (inn.bpm as f64 * m))
        .min_by(|a, b| a.ln().abs().total_cmp(&b.ln().abs()))?;
    if (speed - 1.0).abs() > MOST_STRETCH {
        return None;
    }
    // Mixes run a whole number of bars, about as long as asked.
    let beats = [4.0, 8.0, 16.0, 32.0].into_iter().min_by(|a, b| {
        let off = |n: f64| (n * out_beat / seconds.max(1.0)).ln().abs();
        off(*a).total_cmp(&off(*b))
    })?;
    let length = beats * out_beat;
    // The last downbeat that leaves room for the mix before the beat ends.
    let beat_ends = (*out.beats.last()? as f64 + out_beat).min(out_length);
    let bar = 4.0 * out_beat;
    let grid = out.grid_start as f64;
    let bars = ((beat_ends - length - grid) / bar).floor();
    let out_at = grid + bars * bar;
    let room = out_length.min(out.length as f64);
    if bars < 0.0 || out_at < room * 0.5 {
        return None;
    }
    let in_from = inn.grid_start as f64;
    if !(0.0..=LATEST_FIRST_BEAT).contains(&in_from) {
        return None;
    }
    Some(Timing { out_at, in_from, length, speed, beat: out_beat })
}

/// The beat a drag snaps to, in seconds of the song going out, or of the
/// song coming in with `inn_side`. Zero when the two do not beat match.
pub fn drag_step(out: Option<&Analysis>, out_length: f64, inn: Option<&Analysis>, seconds: f64, inn_side: bool) -> f64 {
    let (Some(a), Some(b)) = (out, inn) else {
        return 0.0;
    };
    match timing(a, out_length, b, seconds) {
        Some(t) if inn_side => t.beat * t.speed,
        Some(t) => t.beat,
        None => 0.0,
    }
}

/// `shift` to the nearest whole `step`, or as it is with no step.
pub fn snap(shift: f64, step: f64) -> f64 {
    if step > 0.0 { (shift / step).round() * step } else { shift }
}

/// Whether two songs can be beat matched, in a mix about `seconds` long.
pub fn beat_matched(out: Option<&Analysis>, out_length: f64, inn: Option<&Analysis>, seconds: f64) -> bool {
    match (out, inn) {
        (Some(a), Some(b)) => timing(a, out_length, b, seconds).is_some(),
        _ => false,
    }
}

// -------------------------------------------------------------------- plan

/// The transition `settings` make from `out`, `out_length` seconds long,
/// into `inn`, about `seconds` long.
///
/// Beat matched when the settings ask and the songs allow: it then starts
/// on a downbeat (`out_at`). Otherwise `out_at` is None, and the player
/// starts it `length` seconds before the old song ends.
pub fn plan(
    settings: Settings,
    out: Option<&Analysis>,
    out_length: f64,
    inn: Option<&Analysis>,
    seconds: f64,
) -> Transition {
    let matched = match (settings.beatmatch, out, inn) {
        (true, Some(a), Some(b)) => timing(a, out_length, b, seconds),
        _ => None,
    };
    // A cut swaps in an instant: on the downbeat where a blend would have
    // finished, so the new song's first downbeat lands on the old song's.
    let cut = settings.volume == Volume::Cut;
    let (mut out_at, mut in_from, length, beat) = match matched {
        Some(t) if cut => (Some(t.out_at + t.length), t.in_from, CUT, t.beat),
        Some(t) => (Some(t.out_at), t.in_from, t.length, t.beat),
        None if cut => (None, 0.0, CUT, 0.5),
        None => (None, 0.0, seconds.max(0.5), seconds.max(0.5) / 16.0),
    };
    // Moved by hand, by as much as dragged: the editor snaps drags to
    // beats. The old song must last through the mix, and the new one cannot
    // start before its top. Kept in range by whole beats, so a mix on the
    // beat stays on it.
    let (out_beat, in_beat) = match matched {
        Some(t) => (t.beat, t.beat * t.speed),
        None => (0.0, 0.0),
    };
    if settings.out_shift != 0.0 {
        let base = out_at.unwrap_or(out_length - length);
        let latest = (out_length - length).max(0.0);
        let mut at = base + settings.out_shift;
        while out_beat > 0.0 && at > latest && at - out_beat >= 0.0 {
            at -= out_beat;
        }
        while out_beat > 0.0 && at < 0.0 {
            at += out_beat;
        }
        out_at = Some(at.clamp(0.0, latest));
    }
    let mut from = in_from + settings.in_shift;
    while in_beat > 0.0 && from < 0.0 {
        from += in_beat;
    }
    in_from = from.max(0.0);
    let mut lanes = Vec::new();
    lanes.extend(volume_lanes(settings.volume, length));
    if settings.eq == Eq::BassSwap {
        lanes.extend(bass_swap(length, beat));
    }
    lanes.extend(filter_lanes(settings.filter, length));
    if let Some(t) = matched {
        if (t.speed - 1.0).abs() > 0.001 {
            // Back to its own tempo over two bars once the old song is gone.
            let settle = length + 8.0 * t.beat;
            lanes.push(Lane::new(
                Side::In,
                Param::Speed,
                &[(0.0, t.speed as f32, Curve::Linear), (length, t.speed as f32, Curve::Linear), (settle, 1.0, Curve::Smooth)],
            ));
        }
    }
    Transition { out_at, in_from, length, lanes }
}

fn volume_lanes(style: Volume, length: f64) -> [Lane; 2] {
    let half = length / 2.0;
    match style {
        Volume::Smooth => [
            Lane::new(Side::Out, Param::Volume, &[(0.0, 1.0, Curve::Linear), (length, 0.0, Curve::EaseIn)]),
            Lane::new(Side::In, Param::Volume, &[(0.0, 0.0, Curve::Linear), (length, 1.0, Curve::EaseOut)]),
        ],
        Volume::Overlap => [
            Lane::new(Side::Out, Param::Volume, &[(0.0, 1.0, Curve::Linear), (half, 1.0, Curve::Linear), (length, 0.0, Curve::EaseIn)]),
            Lane::new(Side::In, Param::Volume, &[(0.0, 0.0, Curve::Linear), (half, 1.0, Curve::EaseOut)]),
        ],
        Volume::Cut => [
            Lane::new(Side::Out, Param::Volume, &[(0.0, 1.0, Curve::Linear), (length, 0.0, Curve::Linear)]),
            Lane::new(Side::In, Param::Volume, &[(0.0, 0.0, Curve::Linear), (length, 1.0, Curve::Linear)]),
        ],
    }
}

/// The bass of the two songs trades places in the middle, over one beat.
fn bass_swap(length: f64, beat: f64) -> [Lane; 2] {
    let mid = length / 2.0;
    let swap = beat.min(length / 4.0);
    [
        Lane::new(Side::Out, Param::Bass, &[(0.0, 1.0, Curve::Linear), (mid, 1.0, Curve::Linear), (mid + swap, 0.0, Curve::Smooth)]),
        Lane::new(Side::In, Param::Bass, &[(0.0, 0.0, Curve::Linear), (mid, 0.0, Curve::Linear), (mid + swap, 1.0, Curve::Smooth)]),
    ]
}

fn filter_lanes(style: Filter, length: f64) -> Vec<Lane> {
    match style {
        Filter::None => Vec::new(),
        // Rising in pitch sounds even, so the cutoff rises by a curve.
        Filter::Rise => vec![Lane::new(
            Side::Out,
            Param::HighPass,
            &[(0.0, 20.0, Curve::Linear), (length, 2000.0, Curve::EaseIn)],
        )],
        Filter::Open => vec![Lane::new(
            Side::In,
            Param::LowPass,
            &[(0.0, 300.0, Curve::Linear), (length * 0.75, 20_000.0, Curve::EaseIn)],
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady(bpm: f32, grid_start: f32, length: f32) -> Analysis {
        let beat = 60.0 / bpm;
        Analysis {
            length,
            bpm,
            bpm_confidence: 0.9,
            grid_start,
            beats: (0..).map(|n| grid_start + n as f32 * beat).take_while(|&t| t < length - 10.0).collect(),
            key: None,
            key_confidence: 0.0,
            waveform: Vec::new(),
        }
    }

    #[test]
    fn a_beat_mix_lands_both_songs_on_the_same_downbeats() {
        let (out, inn) = (steady(128.0, 0.4, 200.0), steady(124.0, 1.1, 180.0));
        let plan = plan(Settings::preset(Preset::Blend), Some(&out), 200.0, Some(&inn), 8.0);
        let beat = 60.0 / 128.0;
        let at = plan.out_at.unwrap();
        let bars = (at - 0.4) / (4.0 * beat);
        assert!((bars - bars.round()).abs() < 1e-6);
        assert!(at + plan.length <= *out.beats.last().unwrap() as f64 + beat + 1e-6);
        assert_eq!(plan.length, 16.0 * beat);
        assert!((plan.in_from - 1.1).abs() < 1e-6);
        let speed = plan.lanes.iter().find(|l| l.param == Param::Speed).unwrap();
        assert!((speed.value(1.0).unwrap() as f64 - 128.0 / 124.0).abs() < 1e-4);
        assert_eq!(speed.value(1000.0), Some(1.0));
    }

    #[test]
    fn songs_too_far_apart_in_tempo_are_not_beat_matched() {
        let out = steady(128.0, 0.0, 200.0);
        let fade = Settings::preset(Preset::Fade);
        assert!(plan(fade, Some(&out), 200.0, Some(&steady(100.0, 0.0, 200.0)), 8.0).out_at.is_none());
        // But half time matches.
        assert!(plan(fade, Some(&out), 200.0, Some(&steady(64.5, 0.0, 200.0)), 8.0).out_at.is_some());
        let mut wobbly = steady(128.0, 0.0, 200.0);
        wobbly.bpm_confidence = 0.3;
        assert!(plan(fade, Some(&out), 200.0, Some(&wobbly), 8.0).out_at.is_none());
        // And without analyses it is a plain fade of the length asked.
        let plain = plan(fade, None, 200.0, None, 6.0);
        assert_eq!((plain.out_at, plain.length), (None, 6.0));
    }

    #[test]
    fn auto_blends_songs_that_match_and_fades_the_rest() {
        let (a, b) = (steady(128.0, 0.0, 200.0), steady(126.0, 0.0, 200.0));
        let auto = Settings::auto(Some(&a), Some(&b), true);
        assert_eq!((auto.preset, auto.eq), (Preset::Auto, Eq::BassSwap));
        let far = steady(90.0, 0.0, 200.0);
        let auto = Settings::auto(Some(&a), Some(&far), true);
        assert_eq!((auto.eq, auto.volume), (Eq::None, Volume::Smooth));
    }

    #[test]
    fn a_bass_swap_never_plays_both_basses_at_full() {
        let lanes = bass_swap(8.0, 0.5);
        for step in 0..=80 {
            let t = step as f64 / 10.0;
            let (out, inn) = (lanes[0].value(t).unwrap(), lanes[1].value(t).unwrap());
            assert!(out + inn <= 1.0 + 1e-5, "at {t}: {out} + {inn}");
        }
    }

    #[test]
    fn a_cut_swaps_on_the_downbeat_a_blend_would_end_on() {
        let (out, inn) = (steady(128.0, 0.0, 200.0), steady(128.0, 0.0, 200.0));
        let blend = plan(Settings::preset(Preset::Blend), Some(&out), 200.0, Some(&inn), 8.0);
        let cut = plan(Settings::preset(Preset::Cut), Some(&out), 200.0, Some(&inn), 8.0);
        assert_eq!(cut.length, CUT);
        assert!((cut.out_at.unwrap() - (blend.out_at.unwrap() + blend.length)).abs() < 1e-9);
        assert_eq!(cut.in_from, blend.in_from);
    }

    #[test]
    fn drags_snap_to_beats_and_stay_in_the_songs() {
        let (out, inn) = (steady(128.0, 0.4, 200.0), steady(128.0, 1.1, 180.0));
        let beat = 60.0 / 128.0;
        let blend = Settings::preset(Preset::Blend);
        let auto = plan(blend, Some(&out), 200.0, Some(&inn), 8.0);
        // A little over two beats earlier snaps to two.
        let step = drag_step(Some(&out), 200.0, Some(&inn), 8.0, false);
        assert!((step - beat).abs() < 1e-6);
        let mut moved = blend;
        moved.out_shift = snap(-2.2 * beat, step);
        moved.in_shift = snap(0.9 * beat, drag_step(Some(&out), 200.0, Some(&inn), 8.0, true));
        let shifted = plan(moved, Some(&out), 200.0, Some(&inn), 8.0);
        assert!((shifted.out_at.unwrap() - (auto.out_at.unwrap() - 2.0 * beat)).abs() < 1e-6);
        assert!((shifted.in_from - (auto.in_from + beat)).abs() < 1e-6);
        // The new song cannot start before its top, and stays on a beat.
        moved.in_shift = -5.0 * beat;
        let early = plan(moved, Some(&out), 200.0, Some(&inn), 8.0);
        assert!(early.in_from >= 0.0);
        let beats = (early.in_from - 1.1) / beat;
        assert!((beats - beats.round()).abs() < 1e-5);
        // Not beat matched: nothing to snap to, and it moves by the second.
        assert_eq!(drag_step(None, 200.0, None, 8.0, false), 0.0);
        let mut plain = Settings::preset(Preset::Fade);
        plain.beatmatch = false;
        plain.out_shift = -10.25;
        let p = plan(plain, None, 200.0, None, 8.0);
        assert_eq!(p.out_at, Some(200.0 - 8.0 - 10.25));
    }

    #[test]
    fn settings_survive_json() {
        let mut s = Settings::preset(Preset::Rise);
        s.preset = Preset::Custom;
        s.beatmatch = false;
        s.out_shift = -3.75;
        s.in_shift = 1.875;
        assert_eq!(Settings::from_json(&s.to_json()), Some(s));
        assert_eq!(Settings::from_json("{}"), None);
    }
}
