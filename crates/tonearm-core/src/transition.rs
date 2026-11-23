//! Transition styles: how one song mixes into the next.
//!
//! A transition is set by a few choices, like Spotify's editor: how the
//! volumes cross, what the EQ does, a filter sweep, an effect on the song
//! going out, a loop of its last beats, and whether the songs are beat
//! matched. Presets are named sets of those choices. Auto picks a preset
//! from the two songs' analyses.
//!
//! `plan` turns the choices and the two songs into a `mix::Transition`:
//! when it starts, where the next song starts from, and its lanes. The
//! mixer only ever plays those.
//!
//! Beat matching reads each song's tempo where the mix happens, not over
//! the whole song, so a song that changes tempo still mixes on the beat.

use crate::analysis::{Analysis, LocalGrid};
use crate::mix::{Curve, Effect, EffectKind, Lane, Param, Side, Transition};
use crate::structure;

/// Default transition length for a mixed playlist when crossfade is off.
pub const MIX_MODE_SECONDS: f64 = 8.0;

/// How the two songs' volumes cross.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Volume {
    /// Equal power across the whole mix.
    Smooth,
    /// The new song comes up over the first half, and the old one goes
    /// over the second, so both play full in the middle. Wants a bass swap
    /// under it, or the low end doubles.
    Overlap,
    /// The new song comes up over the first two thirds, the old one goes
    /// over the last two.
    FadeInFadeOut,
    /// The new song starts at full, and the old one fades under it.
    CutInFadeOut,
    /// The new song fades in, and the old one stops at the end.
    FadeInCutOut,
    /// A straight swap halfway through.
    CenterCut,
    /// Both straight across, in gain rather than power, so it dips a
    /// little in the middle.
    Crossfade,
    /// The new song fades in over the whole mix, and the old one goes over
    /// the last quarter.
    FadeInFastOut,
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
    /// The same, at the start of the mix.
    StartBassSwap,
    /// The same, at the end.
    EndBassSwap,
    /// The highs change over first, then the mids, then the bass.
    ThreeBand,
    /// Neither song has bass for a bar in the middle, then the new song's
    /// comes in.
    QuickBassCut,
    /// The old song's bass goes at the start, and the new song's comes in
    /// at the end, so the mix runs without bass.
    LongBassCut,
    /// The old song's bass fades out over the first half.
    StartFadeOut,
    /// The old song's bass fades out over the whole mix.
    BassFadeOut,
}

/// A filter sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    None,
    /// The old song goes muffled, as a low pass falls.
    LowOut,
    /// The new song opens up from muffled, as a low pass rises.
    LowIn,
    LowInLowOut,
    LowInHighOut,
    /// The old song thins out from the bottom up, as a high pass rises.
    HighOut,
    /// The new song fills in from the top down, as a high pass falls.
    HighIn,
    HighInHighOut,
    HighInLowOut,
    /// The old song thins out over the second half only.
    HighHalfOut,
    /// A rush of noise rises over the end of the old song.
    NoiseOutEnd,
}

/// An effect on the song going out. Its tail rings on over the new song.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fx {
    None,
    /// Reverb that builds to the middle, then fades.
    ReverbOutCenter,
    /// Reverb that builds to the end and rings on.
    ReverbCutEnd,
    /// Reverb that builds to the end and fades with it.
    ReverbOutEnd,
    /// A delay bouncing between the sides, every half or three quarters
    /// of a beat.
    Delay(Fraction, Ending),
    /// An echo every half, three quarters or one beat.
    Echo(Fraction, Ending),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fraction {
    Half,
    ThreeQuarters,
    One,
}

impl Fraction {
    fn beats(self) -> f64 {
        match self {
            Fraction::Half => 0.5,
            Fraction::ThreeQuarters => 0.75,
            Fraction::One => 1.0,
        }
    }
}

/// How an effect's tail ends after the old song stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// It rings on by itself.
    Cut,
    /// It fades out.
    Out,
}

/// The old song's last beats, looped through the mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Looping {
    None,
    Beats(u8),
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
    pub effect: Fx,
    pub looping: Looping,
    pub beatmatch: bool,
    /// The song coming in keeps its key while its speed changes to match,
    /// rather than going up or down in pitch as on a turntable.
    pub key_lock: bool,
    /// Seconds the mix moves within the song going out, later when above
    /// zero, from where it would be. Dragged in the editor. Whole beats
    /// when the songs beat match.
    pub out_shift: f64,
    /// Seconds the song coming in starts later in itself, from its first
    /// downbeat, or from its top when not beat matched.
    pub in_shift: f64,
    /// How many bars the mix lasts. 0 is Auto: `AUTO_BARS` bars when the
    /// songs are beat matched, as in a mix, else as long as the crossfade
    /// setting says.
    pub bars: u32,
}

/// How many bars an Auto length mix lasts.
pub const AUTO_BARS: u32 = 4;

/// The lengths in the editor's menu.
pub const LENGTH_CHOICES: [(u32, &str); 5] = [
    (0, "Auto"),
    (2, "2 bars"),
    (4, "4 bars"),
    (8, "8 bars"),
    (16, "16 bars"),
];

/// The choices in the editor's menus, in Spotify's order. The names are
/// also what is saved, so they stay put.
pub const VOLUME_CHOICES: [(Volume, &str); 9] = [
    (Volume::Smooth, "Smooth crossfade"),
    (Volume::Overlap, "Overlap"),
    (Volume::FadeInFadeOut, "Fade in fade out"),
    (Volume::CutInFadeOut, "Cut in fade out"),
    (Volume::FadeInCutOut, "Fade in cut out"),
    (Volume::CenterCut, "Center cut"),
    (Volume::Crossfade, "Crossfade"),
    (Volume::FadeInFastOut, "Fade in fast out"),
    (Volume::Cut, "Cut on the beat"),
];
pub const EQ_CHOICES: [(Eq, &str); 9] = [
    (Eq::BassSwap, "Center bass swap"),
    (Eq::EndBassSwap, "End bass swap"),
    (Eq::StartBassSwap, "Start bass swap"),
    (Eq::ThreeBand, "3-band fade"),
    (Eq::QuickBassCut, "Quick bass cut"),
    (Eq::LongBassCut, "Long bass cut"),
    (Eq::StartFadeOut, "Start fade out"),
    (Eq::BassFadeOut, "Bass fade out"),
    (Eq::None, "None"),
];
pub const FILTER_CHOICES: [(Filter, &str); 11] = [
    (Filter::LowOut, "Low pass filter out"),
    (Filter::LowIn, "Low pass filter in"),
    (Filter::LowInLowOut, "Low pass filter in low pass filter out"),
    (Filter::LowInHighOut, "Low pass filter in high pass filter out"),
    (Filter::HighOut, "High pass filter out"),
    (Filter::HighIn, "High pass filter in"),
    (Filter::HighInHighOut, "High pass filter in high pass filter out"),
    (Filter::HighInLowOut, "High pass filter in low pass filter out"),
    (Filter::HighHalfOut, "High pass filter half out"),
    (Filter::NoiseOutEnd, "Noise out end"),
    (Filter::None, "None"),
];
pub const EFFECT_CHOICES: [(Fx, &str); 12] = [
    (Fx::ReverbOutCenter, "Reverb out center"),
    (Fx::ReverbCutEnd, "Reverb cut end"),
    (Fx::ReverbOutEnd, "Reverb out end"),
    (Fx::Delay(Fraction::Half, Ending::Cut), "Delay ½ cut end"),
    (Fx::Delay(Fraction::ThreeQuarters, Ending::Cut), "Delay ¾ cut end"),
    (Fx::Echo(Fraction::Half, Ending::Cut), "Echo ½ cut end"),
    (Fx::Echo(Fraction::Half, Ending::Out), "Echo ½ out end"),
    (Fx::Echo(Fraction::ThreeQuarters, Ending::Cut), "Echo ¾ cut end"),
    (Fx::Echo(Fraction::ThreeQuarters, Ending::Out), "Echo ¾ out end"),
    (Fx::Echo(Fraction::One, Ending::Cut), "Echo 1 cut end"),
    (Fx::Echo(Fraction::One, Ending::Out), "Echo 1 out end"),
    (Fx::None, "None"),
];
pub const LOOP_CHOICES: [(Looping, &str); 6] = [
    (Looping::Beats(1), "1 beat loop"),
    (Looping::Beats(2), "2 beat loop"),
    (Looping::Beats(4), "4 beat loop"),
    (Looping::Beats(8), "8 beat loop"),
    (Looping::Beats(16), "16 beat loop"),
    (Looping::None, "None"),
];

impl Settings {
    /// A preset's choices. Auto's are Blend's until the songs say otherwise.
    pub fn preset(preset: Preset) -> Settings {
        let (volume, eq, filter) = match preset {
            Preset::Fade => (Volume::Smooth, Eq::None, Filter::None),
            Preset::Rise => (Volume::Overlap, Eq::BassSwap, Filter::HighOut),
            Preset::Cut => (Volume::Cut, Eq::None, Filter::None),
            Preset::Auto | Preset::Blend | Preset::Custom => (Volume::Overlap, Eq::BassSwap, Filter::None),
        };
        Settings {
            preset,
            volume,
            eq,
            filter,
            effect: Fx::None,
            looping: Looping::None,
            beatmatch: true,
            key_lock: false,
            out_shift: 0.0,
            in_shift: 0.0,
            bars: 0,
        }
    }

    /// What Auto becomes for these two songs. Songs that beat match get a
    /// blend with a bass swap. Others get a smooth fade, since a bass swap
    /// on drums that do not line up sounds like a mistake.
    pub fn auto(out: Option<&Analysis>, inn: Option<&Analysis>, beat_match: bool) -> Settings {
        let timing = match (out, inn) {
            (Some(a), Some(b)) if beat_match => timing(a, f64::MAX, b, 8.0, 0).ok(),
            _ => None,
        };
        let mut settings = if let Some(t) = timing {
            let mut blend = Settings::preset(Preset::Blend);
            // The new song plays at a different speed to match, which on its
            // own changes its pitch. Past a percent or so that puts it out of
            // tune with the old song through the mix (4% is most of a
            // semitone), so it keeps its key.
            blend.key_lock = (t.speed - 1.0).abs() > KEY_LOCK_ABOVE;
            blend
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
            "volume": name_of(&VOLUME_CHOICES, self.volume),
            "eq": name_of(&EQ_CHOICES, self.eq),
            "filter": name_of(&FILTER_CHOICES, self.filter),
            "effect": name_of(&EFFECT_CHOICES, self.effect),
            "looping": name_of(&LOOP_CHOICES, self.looping),
            "beatmatch": self.beatmatch,
            "key_lock": self.key_lock,
            "out_shift": self.out_shift,
            "in_shift": self.in_shift,
            "bars": self.bars,
        })
        .to_string()
    }

    pub fn from_json(text: &str) -> Option<Settings> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        // Saved by name. Older saves kept the place in shorter menus.
        fn pick<T: Copy + PartialEq>(v: &serde_json::Value, choices: &[(T, &str)], old: &[T]) -> Option<T> {
            match v {
                serde_json::Value::String(name) => choices.iter().find(|(_, n)| n == name).map(|(c, _)| *c),
                serde_json::Value::Number(n) => old.get(n.as_u64()? as usize).copied(),
                _ => None,
            }
        }
        Some(Settings {
            preset: Preset::from_key(v["preset"].as_str()?)?,
            volume: pick(&v["volume"], &VOLUME_CHOICES, &[Volume::Smooth, Volume::Overlap, Volume::Cut])?,
            eq: pick(&v["eq"], &EQ_CHOICES, &[Eq::None, Eq::BassSwap])?,
            filter: pick(&v["filter"], &FILTER_CHOICES, &[Filter::None, Filter::HighOut, Filter::LowIn])?,
            effect: pick(&v["effect"], &EFFECT_CHOICES, &[]).unwrap_or(Fx::None),
            looping: pick(&v["looping"], &LOOP_CHOICES, &[]).unwrap_or(Looping::None),
            beatmatch: v["beatmatch"].as_bool()?,
            key_lock: v["key_lock"].as_bool().unwrap_or(false),
            // Saved before they could be dragged: none.
            out_shift: v["out_shift"].as_f64().unwrap_or(0.0),
            in_shift: v["in_shift"].as_f64().unwrap_or(0.0),
            // Saved before a length could be picked: Auto.
            bars: v["bars"]
                .as_u64()
                .map(|n| n as u32)
                .filter(|n| LENGTH_CHOICES.iter().any(|(b, _)| b == n))
                .unwrap_or(0),
        })
    }
}

pub fn index_of<T: PartialEq + Copy>(choices: &[(T, &str)], value: T) -> usize {
    choices.iter().position(|(c, _)| *c == value).unwrap_or(0)
}

fn name_of<T: PartialEq + Copy>(choices: &[(T, &'static str)], value: T) -> &'static str {
    choices.iter().find(|(c, _)| *c == value).map_or("None", |(_, n)| n)
}

// ------------------------------------------------------------------ timing

/// The most a song is sped up or slowed down to match the other, as a
/// share. Past about 6% the change of pitch starts to stand out.
const MOST_STRETCH: f64 = 0.06;
/// A song whose beat starts later than this has a long intro with no beat.
/// Skipping all of it would lose too much, so it fades in instead.
const LATEST_FIRST_BEAT: f64 = 30.0;

/// How far from the end of the old song, or from the start of the new
/// one, to look for a section to mix at, in seconds.
const SECTION_REACH: f64 = 45.0;
/// A song's tempo where it comes in is at its own if it is within this
/// share of the whole song's.
const OWN_TEMPO: f64 = 0.02;
/// How firmly a change in the sound must show to count as a section.
const SECTION_FIRM: f32 = 0.5;

/// Auto keeps the new song's key when its speed changes by more than this
/// share, about 17 cents of pitch.
const KEY_LOCK_ABOVE: f64 = 0.01;

/// How long a cut takes, so the swap does not click.
const CUT: f64 = 0.02;

/// When a beat matched mix happens.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Timing {
    out_at: f64,
    in_from: f64,
    length: f64,
    /// The speed the new song plays at to match, and the old song's beat
    /// where the mix is.
    speed: f64,
    beat: f64,
}

/// Why two songs cannot be beat matched.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Unmatched {
    /// Neither end of the old song nor the start of the new one keeps a
    /// steady beat.
    NoBeat,
    /// Their tempos where they meet are this share apart.
    TempoApart(f64),
    /// The old song's beat stops well before it ends.
    BeatEndsEarly,
    /// The new song's beat starts too late.
    BeatStartsLate,
}

/// The speed the new song plays at so its beat, `in_beat` seconds, lands
/// on the old song's, `out_beat`. Half or double time matches beat for beat.
fn speed_for(out_beat: f64, in_beat: f64) -> f64 {
    [0.5, 1.0, 2.0]
        .into_iter()
        .map(|m| in_beat / (out_beat * m))
        .min_by(|a, b| a.ln().abs().total_cmp(&b.ln().abs()))
        .unwrap_or(1.0)
}

fn steady_part(a: &Analysis, from: f64, to: f64) -> Option<LocalGrid> {
    a.local(from, to).filter(LocalGrid::steady)
}

/// Beat matched timing from `out`, `out_length` seconds long, into `inn`,
/// exactly `bars` bars long, or `AUTO_BARS` when that is 0. `seconds` is the
/// stretch of steady beat to look for before the end.
///
/// The mix starts on a downbeat of the song going out, as late as lets it
/// finish before that song's beat stops, and where its beat is steady. The
/// song coming in starts from its first downbeat, at the old song's tempo
/// there, so beats and bars land together.
fn timing(out: &Analysis, out_length: f64, inn: &Analysis, seconds: f64, bars: u32) -> Result<Timing, Unmatched> {
    if out.beats.len() < 16 || inn.beats.len() < 16 {
        return Err(Unmatched::NoBeat);
    }
    let room = out_length.min(out.length as f64);
    let last = *out.beats.last().unwrap() as f64;
    let last_beat = steady_part(out, last - 8.0, last).map_or(out.beat(), |g| g.beat);
    // From the end of the beat back, the last stretch that keeps a steady
    // beat for the whole mix.
    let mut end = (last + last_beat).min(out_length);
    // Not into a slow fade out or a silent end: the mix finishes while the
    // song is still at full strength.
    if let Some(tail) = structure::edges(out).tail.filter(|&t| t > room * 0.5) {
        end = end.min(tail);
    }
    let mut steady_somewhere = false;
    let mut found = None;
    while end > room * 0.5 {
        let Some(rough) = steady_part(out, end - seconds.max(4.0), end) else {
            end -= 1.0;
            continue;
        };
        steady_somewhere = true;
        let beats = 4.0 * if bars > 0 { bars } else { AUTO_BARS } as f64;
        let span = beats * rough.beat;
        // The old song's last section finishes as the mix does: the latest
        // section start it can end on, not too far before its end, whose
        // mix keeps a steady beat. `end` steps back a second at a time, so
        // a section start up to a beat past it counts. A section start the
        // beat is not steady for is passed over for the one before it.
        let sections =
            structure::section_starts(out, SECTION_FIRM, (end - SECTION_REACH).max(room * 0.5 + span), end + rough.beat);
        let mut from_section = None;
        for &b in sections.iter().rev() {
            let at = b - span;
            if at < room * 0.5 {
                break;
            }
            if let Some(part) = steady_part(out, at, at + span) {
                // On the beat, and on a bar start when one is near, so the
                // two songs' bars line up.
                let at = part.nearest_beat(structure::on_a_bar(out, at, 2.0 * part.beat));
                from_section = Some((at, beats * part.beat, part.beat));
                break;
            }
        }
        if from_section.is_some() {
            found = from_section;
            break;
        }
        // No section: the latest downbeat that lets the mix finish by `end`.
        let at = rough.downbeat_before(end - span);
        if at < room * 0.5 {
            break;
        }
        if let Some(part) = steady_part(out, at, at + span) {
            // Measured over the mix itself, which is a little different.
            found = Some((part.nearest_downbeat(at), beats * part.beat, part.beat));
            break;
        }
        end = at;
    }
    let Some((out_at, length, beat)) = found else {
        return Err(if steady_somewhere { Unmatched::BeatEndsEarly } else { Unmatched::NoBeat });
    };
    // Where the new song's music starts: not in its silence.
    let first = (inn.grid_start as f64).max(structure::edges(inn).start);
    if !(0.0..=LATEST_FIRST_BEAT).contains(&first) {
        return Err(Unmatched::BeatStartsLate);
    }
    // The new song comes in at the start of a section where its beat stays
    // steady for the whole mix: its first one, if it opens loosely, with an
    // ad lib or a beatless intro, the first after that. Without sections, at
    // its first downbeat after any silence.
    let sections = structure::section_starts(inn, SECTION_FIRM, first - 0.3, first + SECTION_REACH);
    // One at the song's own tempo is preferred: where the tempo is off, as
    // in an intro that runs slower or faster than the rest, the new song
    // would have to settle to its real tempo after the mix.
    let own_tempo = |g: &LocalGrid| (g.beat / inn.beat() - 1.0).abs() <= OWN_TEMPO;
    let entry = sections
        .iter()
        .find_map(|&b| steady_part(inn, b, b + length).filter(own_tempo).map(|g| (b, g)))
        .or_else(|| sections.iter().find_map(|&b| steady_part(inn, b, b + length).map(|g| (b, g))));
    let (start, mut in_from) = match entry {
        Some((b, part)) => (part, part.nearest_beat(structure::on_a_bar(inn, b, 2.0 * part.beat))),
        None => {
            let start = steady_part(inn, first, first + length).ok_or(Unmatched::NoBeat)?;
            (start, start.nearest_downbeat(first))
        }
    };
    let bar = 4.0 * start.beat;
    while in_from < first - 0.25 * start.beat {
        in_from += bar;
    }
    let speed = speed_for(beat, start.beat);
    if (speed - 1.0).abs() > MOST_STRETCH {
        return Err(Unmatched::TempoApart(speed));
    }
    Ok(Timing { out_at, in_from, length, speed, beat })
}

/// The timing with the songs moved by hand as `settings` say. Each song's
/// tempo is measured again where it was moved to, and the move lands on
/// the nearest beat there.
fn matched(settings: &Settings, out: &Analysis, out_length: f64, inn: &Analysis, seconds: f64) -> Option<Timing> {
    if !settings.beatmatch {
        return None;
    }
    let mut t = timing(out, out_length, inn, seconds, settings.bars).ok()?;
    let beats = (t.length / t.beat).round();
    if settings.out_shift != 0.0 {
        let mut at = t.out_at + settings.out_shift;
        if let Some(part) = steady_part(out, at, at + t.length) {
            t.beat = part.beat;
            t.length = beats * part.beat;
            at = part.nearest_beat(at);
        }
        // The old song must last through the mix. Kept in range by whole
        // beats, so a mix on the beat stays on it.
        let latest = (out_length - t.length).max(0.0);
        while at > latest && at - t.beat >= 0.0 {
            at -= t.beat;
        }
        while at < 0.0 {
            at += t.beat;
        }
        t.out_at = at.clamp(0.0, latest);
    }
    // The new song cannot start before its top.
    let mut from = t.in_from + settings.in_shift;
    let mut in_beat = t.beat * t.speed;
    if let Some(part) = steady_part(inn, from.max(0.0), from.max(0.0) + t.length) {
        let speed = speed_for(t.beat, part.beat);
        if (speed - 1.0).abs() <= MOST_STRETCH {
            t.speed = speed;
            in_beat = t.beat * speed;
        }
        from = part.nearest_beat(from);
    }
    while from < 0.0 {
        from += in_beat;
    }
    t.in_from = from.max(0.0);
    Some(t)
}

/// The beat a drag snaps to, in seconds of the song going out, or of the
/// song coming in with `inn_side`, where the mix is. Zero when the two do
/// not beat match.
pub fn drag_step(
    settings: &Settings,
    out: Option<&Analysis>,
    out_length: f64,
    inn: Option<&Analysis>,
    seconds: f64,
    inn_side: bool,
) -> f64 {
    let (Some(a), Some(b)) = (out, inn) else {
        return 0.0;
    };
    match matched(settings, a, out_length, b, seconds) {
        Some(t) if inn_side => t.beat * t.speed,
        Some(t) => t.beat,
        None => 0.0,
    }
}

/// `shift` to the nearest whole `step`, or as it is with no step.
pub fn snap(shift: f64, step: f64) -> f64 {
    if step > 0.0 { (shift / step).round() * step } else { shift }
}

/// Why two songs cannot be beat matched, for the warning on their tempos.
/// None when they can.
pub fn why_unmatched(
    out: Option<&Analysis>,
    out_length: f64,
    inn: Option<&Analysis>,
    seconds: f64,
    bars: u32,
) -> Option<String> {
    let (Some(a), Some(b)) = (out, inn) else {
        return Some("Beat matching waits until both songs are analyzed".into());
    };
    let why = match timing(a, out_length, b, seconds, bars).err()? {
        Unmatched::NoBeat => "neither keeps a steady beat where they meet".to_string(),
        Unmatched::TempoApart(speed) => {
            format!("their tempos where they meet are {:.0}% apart", (speed - 1.0).abs() * 100.0)
        }
        Unmatched::BeatEndsEarly => "the first song's beat stops well before it ends".to_string(),
        Unmatched::BeatStartsLate => "the second song's beat starts too late".to_string(),
    };
    Some(format!("Beat matching not possible: {why}"))
}

// -------------------------------------------------------------------- plan

/// How long an effect's tail rings on after the old song stops.
const TAIL: f64 = 4.0;

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
    let matched = match (out, inn) {
        (Some(a), Some(b)) => matched(&settings, a, out_length, b, seconds),
        _ => None,
    };
    // A cut swaps in an instant: on the downbeat where a blend would have
    // finished, so the new song's first downbeat lands on the old song's.
    let cut = settings.volume == Volume::Cut;
    // Not beat matched: the new song starts where its music does, after any
    // silence, and the old one is left before a slow fade or silent end.
    let unmatched_in = || inn.map_or(0.0, |a| structure::edges(a).start) + settings.in_shift.max(0.0);
    let (mut out_at, in_from, length, beat) = match matched {
        Some(t) if cut => (Some(t.out_at + t.length), t.in_from, CUT, t.beat),
        Some(t) => (Some(t.out_at), t.in_from, t.length, t.beat),
        None if cut => (unmatched_out_at(out, out_length, CUT), unmatched_in(), CUT, 0.5),
        None => {
            // Bars at the old song's own tempo, or at 120 bpm before it is
            // analyzed.
            let length = match settings.bars {
                // In a mix Auto is the same number of bars as when matched.
                // Outside one, a crossfade lasts as long as its setting says.
                0 if !settings.beatmatch => seconds.max(0.5),
                0 => 4.0 * AUTO_BARS as f64 * out.map_or(0.5, Analysis::beat),
                bars => 4.0 * bars as f64 * out.map_or(0.5, Analysis::beat),
            };
            (unmatched_out_at(out, out_length, length), unmatched_in(), length, length / 16.0)
        }
    };
    // Moved by hand, not beat matched: by as much as dragged, and the old
    // song must last through the mix.
    if matched.is_none() && settings.out_shift != 0.0 {
        let latest = (out_length - length).max(0.0);
        out_at = Some((out_at.unwrap_or(latest) + settings.out_shift).clamp(0.0, latest));
    }
    let mut lanes = Vec::new();
    lanes.extend(volume_lanes(settings.volume, length));
    lanes.extend(eq_lanes(settings.eq, length, beat));
    lanes.extend(filter_lanes(settings.filter, length));
    let effect = effect_lanes(settings.effect, length, beat, &mut lanes);
    let mut key_lock = false;
    if let Some(t) = matched {
        if (t.speed - 1.0).abs() > 0.001 {
            // Back to its own tempo over two bars once the old song is gone.
            let settle = length + 8.0 * t.beat;
            lanes.push(Lane::new(
                Side::In,
                Param::Speed,
                &[(0.0, t.speed as f32, Curve::Linear), (length, t.speed as f32, Curve::Linear), (settle, 1.0, Curve::Smooth)],
            ));
            key_lock = settings.key_lock;
        }
    }
    let out_loop = match settings.looping {
        Looping::Beats(n) if !cut => Some(n as f64 * beat),
        _ => None,
    };
    Transition { out_at, in_from, length, lanes, effect, out_loop, key_lock }
}

/// Where a mix `length` seconds long starts in a song that is not beat
/// matched, when the song fades out slowly or goes quiet before its end:
/// so it finishes about where the fade begins, on a section start if one
/// is near. None leaves it to start `length` before the end.
fn unmatched_out_at(out: Option<&Analysis>, out_length: f64, length: f64) -> Option<f64> {
    let out = out?;
    let room = out_length.min(out.length as f64);
    let tail = structure::edges(out).tail.filter(|&t| t > room * 0.5)?;
    let latest = (out_length - length).max(0.0);
    let mut end = tail + length * 0.25;
    if let Some(section) = structure::last_boundary(out, (end - 20.0).max(room * 0.5), end, SECTION_FIRM) {
        end = section;
    }
    let at = (end - length).min(latest);
    (at > room * 0.25 && at < latest - 1.0).then_some(at)
}

/// Chooses the saved setting, or Auto's pick when none was saved.
pub fn settings_between(
    from: &str,
    to: &str,
    mix_mode: bool,
    out: Option<&Analysis>,
    inn: Option<&Analysis>,
    override_settings: Option<Settings>,
) -> Settings {
    override_settings
        .or_else(|| crate::library::transition(from, to))
        .unwrap_or_else(|| Settings::auto(out, inn, mix_mode))
}

/// Chooses and plans the transition between two songs. An explicit setting
/// (for example one being previewed) takes precedence over a saved setting;
/// otherwise Auto uses the songs' analyses and whether this is a mixed playlist.
pub fn plan_between(
    from: &str,
    to: &str,
    mix_mode: bool,
    out: Option<&Analysis>,
    out_length: f64,
    inn: Option<&Analysis>,
    seconds: f64,
    override_settings: Option<Settings>,
) -> (Settings, Transition) {
    let settings = settings_between(from, to, mix_mode, out, inn, override_settings);
    let plan = plan(settings, out, out_length, inn, seconds);
    (settings, plan)
}

/// A lane for one song, from (seconds, value, curve) keys.
fn lane(side: Side, param: Param, keys: &[(f64, f32, Curve)]) -> Lane {
    Lane::new(side, param, keys)
}

fn volume_lanes(style: Volume, length: f64) -> [Lane; 2] {
    use Curve::*;
    let l = length;
    let half = l / 2.0;
    // A swap this quick does not click.
    let snap = CUT.min(l / 4.0);
    let (out, inn): (Vec<(f64, f32, Curve)>, Vec<(f64, f32, Curve)>) = match style {
        Volume::Smooth => (vec![(0.0, 1.0, Linear), (l, 0.0, EaseIn)], vec![(0.0, 0.0, Linear), (l, 1.0, EaseOut)]),
        Volume::Overlap => (
            vec![(0.0, 1.0, Linear), (half, 1.0, Linear), (l, 0.0, EaseIn)],
            vec![(0.0, 0.0, Linear), (half, 1.0, EaseOut)],
        ),
        Volume::FadeInFadeOut => (
            vec![(0.0, 1.0, Linear), (l / 3.0, 1.0, Linear), (l, 0.0, EaseIn)],
            vec![(0.0, 0.0, Linear), (l * 2.0 / 3.0, 1.0, EaseOut)],
        ),
        Volume::CutInFadeOut => (vec![(0.0, 1.0, Linear), (l, 0.0, EaseIn)], vec![(0.0, 1.0, Linear)]),
        Volume::FadeInCutOut => (
            vec![(0.0, 1.0, Linear), (l - snap, 1.0, Linear), (l, 0.0, Linear)],
            vec![(0.0, 0.0, Linear), (l, 1.0, EaseOut)],
        ),
        Volume::CenterCut => (
            vec![(0.0, 1.0, Linear), (half, 1.0, Linear), (half + snap, 0.0, Linear)],
            vec![(0.0, 0.0, Linear), (half, 0.0, Linear), (half + snap, 1.0, Linear)],
        ),
        Volume::Crossfade => (vec![(0.0, 1.0, Linear), (l, 0.0, Linear)], vec![(0.0, 0.0, Linear), (l, 1.0, Linear)]),
        Volume::FadeInFastOut => (
            vec![(0.0, 1.0, Linear), (l * 0.75, 1.0, Linear), (l, 0.0, EaseIn)],
            vec![(0.0, 0.0, Linear), (l, 1.0, EaseOut)],
        ),
        Volume::Cut => (vec![(0.0, 1.0, Linear), (l, 0.0, Linear)], vec![(0.0, 0.0, Linear), (l, 1.0, Linear)]),
    };
    [lane(Side::Out, Param::Volume, &out), lane(Side::In, Param::Volume, &inn)]
}

/// The bass of the two songs trades places at `at`, over `swap` seconds.
fn bass_swap_at(at: f64, swap: f64) -> [Lane; 2] {
    use Curve::*;
    [
        lane(Side::Out, Param::Bass, &[(0.0, 1.0, Linear), (at, 1.0, Linear), (at + swap, 0.0, Smooth)]),
        lane(Side::In, Param::Bass, &[(0.0, 0.0, Linear), (at, 0.0, Linear), (at + swap, 1.0, Smooth)]),
    ]
}

/// The bass of the two songs trades places in the middle, over one beat.
fn bass_swap(length: f64, beat: f64) -> [Lane; 2] {
    let swap = beat.min(length / 4.0);
    bass_swap_at(length / 2.0 - swap / 2.0, swap)
}

fn eq_lanes(style: Eq, length: f64, beat: f64) -> Vec<Lane> {
    use Curve::*;
    let l = length;
    let swap = beat.min(l / 4.0);
    match style {
        Eq::None => Vec::new(),
        Eq::BassSwap => bass_swap(l, beat).to_vec(),
        Eq::StartBassSwap => bass_swap_at(0.0, swap).to_vec(),
        Eq::EndBassSwap => bass_swap_at(l - swap, swap).to_vec(),
        Eq::ThreeBand => {
            // Each band changes over in its own third: highs, mids, bass.
            let third = l / 3.0;
            let mut lanes = Vec::new();
            for (n, param) in [Param::Treble, Param::Mid, Param::Bass].into_iter().enumerate() {
                let (from, to) = (third * n as f64, third * (n + 1) as f64);
                lanes.push(lane(Side::Out, param, &[(0.0, 1.0, Linear), (from, 1.0, Linear), (to, 0.0, Smooth)]));
                lanes.push(lane(Side::In, param, &[(0.0, 0.0, Linear), (from, 0.0, Linear), (to, 1.0, Smooth)]));
            }
            lanes
        }
        Eq::QuickBassCut => {
            let bar = (4.0 * beat).min(l / 2.0);
            let (gone, back) = (l / 2.0 - bar / 2.0, l / 2.0 + bar / 2.0);
            vec![
                lane(Side::Out, Param::Bass, &[(0.0, 1.0, Linear), (gone, 1.0, Linear), (gone + CUT, 0.0, Linear)]),
                lane(Side::In, Param::Bass, &[(0.0, 0.0, Linear), (back, 0.0, Linear), (back + CUT, 1.0, Linear)]),
            ]
        }
        Eq::LongBassCut => vec![
            lane(Side::Out, Param::Bass, &[(0.0, 1.0, Linear), (swap, 0.0, Smooth)]),
            lane(Side::In, Param::Bass, &[(0.0, 0.0, Linear), (l - swap, 0.0, Linear), (l, 1.0, Smooth)]),
        ],
        Eq::StartFadeOut => vec![lane(Side::Out, Param::Bass, &[(0.0, 1.0, Linear), (l / 2.0, 0.0, Smooth)])],
        Eq::BassFadeOut => vec![lane(Side::Out, Param::Bass, &[(0.0, 1.0, Linear), (l, 0.0, Smooth)])],
    }
}

fn filter_lanes(style: Filter, length: f64) -> Vec<Lane> {
    use Curve::*;
    let l = length;
    // Rising in pitch sounds even, so the cutoffs move by curves: slow at
    // the low end, quick at the high.
    let low_out = || lane(Side::Out, Param::LowPass, &[(0.0, 20_000.0, Linear), (l, 300.0, EaseOut)]);
    let low_in = || lane(Side::In, Param::LowPass, &[(0.0, 300.0, Linear), (l * 0.75, 20_000.0, EaseIn)]);
    let high_out = || lane(Side::Out, Param::HighPass, &[(0.0, 20.0, Linear), (l, 2000.0, EaseIn)]);
    let high_in = || lane(Side::In, Param::HighPass, &[(0.0, 2000.0, Linear), (l * 0.75, 20.0, EaseOut)]);
    match style {
        Filter::None => Vec::new(),
        Filter::LowOut => vec![low_out()],
        Filter::LowIn => vec![low_in()],
        Filter::LowInLowOut => vec![low_in(), low_out()],
        Filter::LowInHighOut => vec![low_in(), high_out()],
        Filter::HighOut => vec![high_out()],
        Filter::HighIn => vec![high_in()],
        Filter::HighInHighOut => vec![high_in(), high_out()],
        Filter::HighInLowOut => vec![high_in(), low_out()],
        Filter::HighHalfOut => vec![lane(
            Side::Out,
            Param::HighPass,
            &[(0.0, 20.0, Linear), (l / 2.0, 20.0, Linear), (l, 2000.0, EaseIn)],
        )],
        Filter::NoiseOutEnd => vec![lane(
            Side::Out,
            Param::Noise,
            &[(0.0, 0.0, Linear), (l / 2.0, 0.0, Linear), (l, 1.0, EaseIn)],
        )],
    }
}

/// The lanes that feed the old song into an effect and bring the effect
/// back, and the effect itself.
fn effect_lanes(style: Fx, length: f64, beat: f64, lanes: &mut Vec<Lane>) -> Option<Effect> {
    use Curve::*;
    let l = length;
    let (kind, ending, center) = match style {
        Fx::None => return None,
        Fx::ReverbOutCenter => (EffectKind::Reverb, Ending::Out, true),
        Fx::ReverbCutEnd => (EffectKind::Reverb, Ending::Cut, false),
        Fx::ReverbOutEnd => (EffectKind::Reverb, Ending::Out, false),
        Fx::Delay(f, e) => (EffectKind::PingPong(f.beats() * beat), e, false),
        Fx::Echo(f, e) => (EffectKind::Echo(f.beats() * beat), e, false),
    };
    if center {
        // Builds to the middle, stops taking the song, and fades by the end.
        let half = l / 2.0;
        lanes.push(lane(Side::Out, Param::Send, &[(0.0, 0.0, Linear), (half, 1.0, EaseIn), (half + CUT, 0.0, Linear)]));
        lanes.push(lane(Side::Out, Param::Wet, &[(0.0, 1.0, Linear), (half, 1.0, Linear), (l, 0.0, Smooth)]));
        return Some(Effect { kind });
    }
    match ending {
        // Takes the end of the song, and rings on after it.
        Ending::Cut => {
            lanes.push(lane(Side::Out, Param::Send, &[(0.0, 0.0, Linear), (l / 2.0, 0.0, Linear), (l, 1.0, EaseIn)]));
            lanes.push(lane(Side::Out, Param::Wet, &[(0.0, 1.0, Linear), (l + TAIL, 1.0, Linear), (l + TAIL + 0.5, 0.0, Smooth)]));
        }
        // Takes more of the song as it goes, and fades soon after it.
        Ending::Out => {
            lanes.push(lane(Side::Out, Param::Send, &[(0.0, 0.0, Linear), (l, 1.0, EaseIn)]));
            lanes.push(lane(Side::Out, Param::Wet, &[(0.0, 1.0, Linear), (l, 1.0, Linear), (l + TAIL / 2.0, 0.0, Smooth)]));
        }
    }
    Some(Effect { kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(beats: Vec<f32>, bpm: f32, grid_start: f32, length: f32) -> Analysis {
        Analysis {
            length,
            bpm,
            bpm_confidence: 0.9,
            grid_start,
            beats,
            downbeats: Vec::new(),
            key: None,
            key_confidence: 0.0,
            waveform: Vec::new(),
        }
    }

    fn steady(bpm: f32, grid_start: f32, length: f32) -> Analysis {
        let beat = 60.0 / bpm;
        let beats = (0..).map(|n| grid_start + n as f32 * beat).take_while(|&t| t < length - 10.0).collect();
        song(beats, bpm, grid_start, length)
    }

    fn speed_of(plan: &Transition) -> f64 {
        plan.lanes.iter().find(|l| l.param == Param::Speed).map_or(1.0, |l| l.value(0.0).unwrap() as f64)
    }

    #[test]
    fn a_beat_mix_lands_both_songs_on_the_same_downbeats() {
        let (out, inn) = (steady(128.0, 0.4, 200.0), steady(124.0, 1.1, 180.0));
        let plan = plan(Settings::preset(Preset::Blend), Some(&out), 200.0, Some(&inn), 8.0);
        let beat = 60.0 / 128.0;
        let at = plan.out_at.unwrap();
        let bars = (at - 0.4) / (4.0 * beat);
        // Beats are kept to f32, so lined up to a fraction of a millisecond.
        assert!((bars - bars.round()).abs() < 1e-4);
        assert!(at + plan.length <= *out.beats.last().unwrap() as f64 + beat + 1e-4);
        assert!((plan.length - 16.0 * beat).abs() < 1e-4);
        assert!((plan.in_from - 1.1).abs() < 1e-4);
        let speed = plan.lanes.iter().find(|l| l.param == Param::Speed).unwrap();
        assert!((speed.value(1.0).unwrap() as f64 - 128.0 / 124.0).abs() < 1e-4);
        assert_eq!(speed.value(1000.0), Some(1.0));
    }

    #[test]
    fn a_mix_lasts_the_bars_picked() {
        let (out, inn) = (steady(128.0, 0.4, 200.0), steady(124.0, 1.1, 180.0));
        let beat = 60.0 / 128.0;
        for bars in [2, 4, 8, 16] {
            let mut blend = Settings::preset(Preset::Blend);
            blend.bars = bars;
            let p = plan(blend, Some(&out), 200.0, Some(&inn), 8.0);
            let at = p.out_at.expect("beat matched");
            assert!((p.length - 4.0 * bars as f64 * beat).abs() < 1e-4, "{bars} bars: {}", p.length);
            // Still on a downbeat, and done before the beat stops.
            let downbeats = (at - 0.4) / (4.0 * beat);
            assert!((downbeats - downbeats.round()).abs() < 1e-4);
            assert!(at + p.length <= *out.beats.last().unwrap() as f64 + beat + 1e-4);
        }
        // Not beat matched, the bars are counted at the old song's tempo.
        let mut fade = Settings::preset(Preset::Fade);
        fade.beatmatch = false;
        fade.bars = 4;
        let p = plan(fade, Some(&out), 200.0, Some(&inn), 8.0);
        assert!(p.out_at.is_none());
        assert!((p.length - 16.0 * out.beat()).abs() < 1e-4);
    }

    #[test]
    fn a_song_that_changes_tempo_mixes_at_its_tempo_where_it_ends() {
        // 90 BPM for a minute, then 128 to the end. The whole song's grid
        // fits neither.
        let mut beats: Vec<f32> = (0..90).map(|n| 0.5 + n as f32 * 60.0 / 90.0).collect();
        let change = *beats.last().unwrap();
        beats.extend((1..).map(|n| change + n as f32 * 60.0 / 128.0).take_while(|&t| t < 190.0));
        let mut out = song(beats, 105.0, 0.5, 200.0);
        out.bpm_confidence = 0.3;
        let inn = steady(126.0, 0.8, 180.0);
        let plan = plan(Settings::preset(Preset::Blend), Some(&out), 200.0, Some(&inn), 8.0);
        let at = plan.out_at.expect("beat matched");
        assert!(at > change as f64);
        assert!((speed_of(&plan) - 128.0 / 126.0).abs() < 1e-3);
        // On one of the tracked beats, and on a downbeat counted through
        // the change: every fourth beat from the first.
        let n = out.beats.iter().position(|&b| (b as f64 - at).abs() < 1e-3).expect("on a beat");
        assert_eq!(n % 4, 0);
    }

    #[test]
    fn songs_too_far_apart_in_tempo_are_not_beat_matched() {
        let out = steady(128.0, 0.0, 200.0);
        let fade = Settings::preset(Preset::Fade);
        assert!(plan(fade, Some(&out), 200.0, Some(&steady(100.0, 0.0, 200.0)), 8.0).out_at.is_none());
        // But half time matches.
        assert!(plan(fade, Some(&out), 200.0, Some(&steady(64.5, 0.0, 200.0)), 8.0).out_at.is_some());
        // Beats all over the place do not.
        let mut seed = 7u32;
        let wobbly: Vec<f32> = (0..300)
            .map(|n| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                n as f32 * 0.47 + (seed >> 16) as f32 / 65_536.0 * 0.2
            })
            .collect();
        let wobbly = song(wobbly, 128.0, 0.0, 200.0);
        assert!(plan(fade, Some(&out), 200.0, Some(&wobbly), 8.0).out_at.is_none());
        assert!(why_unmatched(Some(&out), 200.0, Some(&wobbly), 8.0, 0).is_some());
        // And without analyses a plain crossfade, which does not beat match,
        // is a fade of the length asked.
        let mut crossfade = fade;
        crossfade.beatmatch = false;
        let plain = plan(crossfade, None, 200.0, None, 6.0);
        assert_eq!((plain.out_at, plain.length), (None, 6.0));
    }

    #[test]
    fn an_auto_length_mix_is_four_bars_whatever_the_crossfade_setting() {
        let (a, b) = (steady(128.0, 0.0, 200.0), steady(126.0, 0.0, 200.0));
        let auto = Settings::auto(Some(&a), Some(&b), true);
        assert_eq!(auto.bars, 0);
        for seconds in [4.0, 8.0, 30.0] {
            let p = plan(auto, Some(&a), 200.0, Some(&b), seconds);
            assert!((p.length - 16.0 * a.beat()).abs() < 0.2, "{seconds}: {}", p.length);
        }
        // Not analysed, in a mix: four bars at 120 bpm.
        let blind = plan(Settings::auto(None, None, true), None, 200.0, None, 30.0);
        assert_eq!(blind.length, 8.0);
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
        for style in [Eq::BassSwap, Eq::StartBassSwap, Eq::EndBassSwap, Eq::QuickBassCut, Eq::LongBassCut, Eq::ThreeBand] {
            let lanes = eq_lanes(style, 8.0, 0.5);
            let value = |side, t| {
                lanes
                    .iter()
                    .find(|l| l.side == side && l.param == Param::Bass)
                    .map_or(1.0, |l| l.value(t).unwrap())
            };
            for step in 0..=80 {
                let t = step as f64 / 10.0;
                let (out, inn) = (value(Side::Out, t), value(Side::In, t));
                assert!(out + inn <= 1.0 + 1e-5, "{style:?} at {t}: {out} + {inn}");
            }
        }
    }

    #[test]
    fn every_volume_style_starts_on_the_old_song_and_ends_on_the_new() {
        for (style, name) in VOLUME_CHOICES {
            let [out, inn] = volume_lanes(style, 8.0);
            assert_eq!(out.value(0.0), Some(1.0), "{name}");
            assert!(out.value(8.0).unwrap() < 1e-6, "{name}");
            assert!((inn.value(8.0).unwrap() - 1.0).abs() < 1e-6, "{name}");
        }
    }

    #[test]
    fn effects_ring_on_after_the_old_song() {
        let mut lanes = Vec::new();
        let effect = effect_lanes(Fx::Echo(Fraction::ThreeQuarters, Ending::Cut), 8.0, 0.5, &mut lanes);
        assert_eq!(effect.map(|e| e.kind), Some(EffectKind::Echo(0.375)));
        let wet = lanes.iter().find(|l| l.param == Param::Wet).unwrap();
        assert_eq!(wet.value(10.0), Some(1.0));
        let send = lanes.iter().find(|l| l.param == Param::Send).unwrap();
        assert_eq!(send.value(0.0), Some(0.0));
        assert_eq!(send.value(8.0), Some(1.0));
    }

    #[test]
    fn a_loop_is_whole_beats_of_the_old_song() {
        let (out, inn) = (steady(120.0, 0.0, 200.0), steady(120.0, 0.0, 200.0));
        let mut s = Settings::preset(Preset::Blend);
        s.looping = Looping::Beats(4);
        let plan = plan(s, Some(&out), 200.0, Some(&inn), 8.0);
        assert!((plan.out_loop.unwrap() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn auto_keeps_the_key_when_the_tempos_are_a_few_percent_apart() {
        let out = steady(142.0, 0.4, 200.0);
        // 4% apart: out of tune by most of a semitone without it.
        assert!(Settings::auto(Some(&out), Some(&steady(148.0, 0.4, 200.0)), true).key_lock);
        // Hardly apart, and with nothing to match: no need.
        assert!(!Settings::auto(Some(&out), Some(&steady(142.5, 0.4, 200.0)), true).key_lock);
        assert!(!Settings::auto(None, None, true).key_lock);
    }

    #[test]
    fn key_lock_holds_only_when_the_speed_changes() {
        let out = steady(128.0, 0.0, 200.0);
        let mut s = Settings::preset(Preset::Blend);
        s.key_lock = true;
        assert!(plan(s, Some(&out), 200.0, Some(&steady(124.0, 0.0, 200.0)), 8.0).key_lock);
        assert!(!plan(s, Some(&out), 200.0, Some(&steady(128.0, 0.0, 200.0)), 8.0).key_lock);
        s.key_lock = false;
        assert!(!plan(s, Some(&out), 200.0, Some(&steady(124.0, 0.0, 200.0)), 8.0).key_lock);
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
        let step = drag_step(&blend, Some(&out), 200.0, Some(&inn), 8.0, false);
        assert!((step - beat).abs() < 1e-6);
        let mut moved = blend;
        moved.out_shift = snap(-2.2 * beat, step);
        moved.in_shift = snap(0.9 * beat, drag_step(&blend, Some(&out), 200.0, Some(&inn), 8.0, true));
        let shifted = plan(moved, Some(&out), 200.0, Some(&inn), 8.0);
        assert!((shifted.out_at.unwrap() - (auto.out_at.unwrap() - 2.0 * beat)).abs() < 1e-6);
        assert!((shifted.in_from - (auto.in_from + beat)).abs() < 1e-6);
        // Off the beat by a little, it lands back on one.
        moved.out_shift = -2.0 * beat + 0.05;
        let near = plan(moved, Some(&out), 200.0, Some(&inn), 8.0);
        assert!((near.out_at.unwrap() - (auto.out_at.unwrap() - 2.0 * beat)).abs() < 1e-6);
        // The new song cannot start before its top, and stays on a beat.
        moved.in_shift = -5.0 * beat;
        let early = plan(moved, Some(&out), 200.0, Some(&inn), 8.0);
        assert!(early.in_from >= 0.0);
        let beats = (early.in_from - 1.1) / beat;
        assert!((beats - beats.round()).abs() < 1e-5);
        // Not beat matched: nothing to snap to, and it moves by the second.
        assert_eq!(drag_step(&blend, None, 200.0, None, 8.0, false), 0.0);
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
        s.key_lock = true;
        s.effect = Fx::Echo(Fraction::One, Ending::Out);
        s.looping = Looping::Beats(8);
        s.eq = Eq::ThreeBand;
        s.out_shift = -3.75;
        s.in_shift = 1.875;
        s.bars = 16;
        assert_eq!(Settings::from_json(&s.to_json()), Some(s));
        assert_eq!(Settings::from_json("{}"), None);
        // Saved by an older version, by place in the menus.
        let old = r#"{"preset":"rise","volume":1,"eq":1,"filter":1,"beatmatch":true}"#;
        assert_eq!(Settings::from_json(old), Some(Settings::preset(Preset::Rise)));
    }

    /// Looks at the mix between two songs in this computer's library. Run
    /// with `PROBE_FROM=<id> PROBE_TO=<id> cargo test probe_mix -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn probe_mix() {
        let (from, to) = (std::env::var("PROBE_FROM").unwrap(), std::env::var("PROBE_TO").unwrap());
        let (a, b) = (crate::library::analysis(&from).unwrap(), crate::library::analysis(&to).unwrap());
        let length = a.length as f64;
        let (settings, plan) = plan_between(&from, &to, true, Some(&a), length, Some(&b), 8.0, None);
        println!("settings {settings:?}");
        println!("plan out_at {:?} in_from {:.3} length {:.3}", plan.out_at, plan.in_from, plan.length);
        println!("why not matched: {:?}", why_unmatched(Some(&a), length, Some(&b), 8.0, 0));
        for end in [214.0, 206.0, 198.0, 190.0, 180.0, 170.0, 150.0, 120.0] {
            let g = a.local(end - 8.0, end);
            println!(
                "out local {end:.0}: {}",
                g.map_or("none".to_string(), |g| format!(
                    "beat {:.4} ({:.2} bpm, global {:.2}) conf {:.2} on {} beats",
                    g.beat,
                    60.0 / g.beat,
                    a.bpm,
                    g.confidence,
                    g.beats
                ))
            );
        }
        let g = b.local(0.0, 8.0);
        println!("in local 0-8: {:?}", g);
        for (name, song, times) in [("out", &a, [170.0, 178.7, 187.9, 197.0, 204.2]), ("in", &b, [1.6, 7.9, 13.6, 27.1, 36.8])] {
            for t in times {
                let g = song.local(t, t + 9.0);
                println!(
                    "{name} steady stretch {t:.1}-{:.1}: {}",
                    t + 9.0,
                    g.map_or("none".to_string(), |g| format!("conf {:.2} on {} beats, beat {:.4}", g.confidence, g.beats, g.beat))
                );
            }
        }
        println!("edges out {:?} in {:?}", structure::edges(&a), structure::edges(&b));
        println!(
            "steady out {} (conf {:.2}, {} beats) in {} (conf {:.2}, {} beats); last beat {:.1}s of {:.1}s",
            a.steady(),
            a.bpm_confidence,
            a.beats.len(),
            b.steady(),
            b.bpm_confidence,
            b.beats.len(),
            a.beats.last().copied().unwrap_or(0.0),
            a.length
        );
        println!("edges out {:?} in {:?}", structure::edges(&a), structure::edges(&b));
        let show = |name: &str, a: &Analysis| {
            let found: Vec<String> = structure::boundaries(a).iter().map(|b| format!("{:.1}s({:.2})", b.time, b.strength)).collect();
            println!("sections {name}: {}", found.join(" "));
        };
        show("out", &a);
        show("in", &b);
        // Where a time falls among the beats and found bars of a song.
        let place = |a: &Analysis, t: f64| {
            let beat = a.beats.iter().map(|&x| x as f64).min_by(|x, y| (x - t).abs().total_cmp(&(y - t).abs())).unwrap();
            let down = a.downbeats.iter().map(|&x| x as f64).min_by(|x, y| (x - t).abs().total_cmp(&(y - t).abs())).unwrap_or(f64::NAN);
            // Tracked beats between the nearest found downbeat at or before the beat and the beat.
            let earlier = a.downbeats.iter().map(|&x| x as f64).filter(|&d| d <= beat + 0.05).last().unwrap_or(f64::NAN);
            let k = a.beats.iter().filter(|&&x| (x as f64) >= earlier - 0.05 && (x as f64) < beat - 0.05).count();
            format!("t {t:.3}: nearest beat {beat:.3} ({:+.3}), nearest found downbeat {down:.3} ({:+.3}), beat {} of the bar", beat - t, down - t, k + 1)
        };
        if let Some(at) = plan.out_at {
            println!("OUT {}", place(&a, at));
        }
        println!("IN  {}", place(&b, plan.in_from));
        println!("out grid_start {:.3} bpm {:.2}; in grid_start {:.3} bpm {:.2}", a.grid_start, a.bpm, b.grid_start, b.bpm);
    }
}
