//! Plain editor data and operations for playlist transitions.
//!
//! The desktop and phone draw this data themselves. In particular, the
//! picture is returned as SVG path strings so the core has no UI dependency.

use crate::analysis::Analysis;
use crate::mix::{Param, Side, Transition};
use crate::transition::{self, Preset, Settings};

pub const PICTURE_WIDTH: f64 = 340.0;
pub const PICTURE_HEIGHT: f64 = 230.0;
const STEPS: usize = 170;
const FREE_STEP: f64 = 0.5;

#[derive(Clone, Debug, PartialEq)]
pub struct SongInfo {
    pub bpm: String,
    pub camelot: String,
    pub key_color: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub title: String,
    pub options: Vec<String>,
    pub selected: usize,
    pub accent: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub out_wave: u32,
    pub in_wave: u32,
    pub low: u32,
    pub volume: u32,
    pub bass: u32,
    pub filter: u32,
    pub effect: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub card_from: f32,
    pub card_to: f32,
    pub span: f32,
    pub out_wave: String,
    pub out_low: String,
    pub in_wave: String,
    pub in_low: String,
    pub volume: String,
    pub bass: String,
    pub filter: String,
    pub effect: String,
    pub bars: String,
    pub beats: String,
    pub left: f64,
    pub right: f64,
    pub in_speed: f64,
    pub out_start: f64,
    pub in_from: f64,
    pub palette: Palette,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditorData {
    pub presets: Vec<String>,
    pub preset_selected: i32,
    pub choices: Vec<Choice>,
    pub changed: bool,
    pub can_beatmatch: bool,
    pub why_unmatched: String,
    pub beatmatch: bool,
    pub key_lock: bool,
    pub from: SongInfo,
    pub to: SongInfo,
    pub picture: Picture,
}

/// Key colour on the Camelot wheel, paler for the major keys.
pub fn key_color(camelot: &str) -> u32 {
    let number: f32 = camelot.trim_end_matches(['A', 'B']).parse().unwrap_or(1.0);
    let hue = (number - 1.0) * 30.0;
    let light = if camelot.ends_with('B') { 0.78 } else { 0.68 };
    let (r, g, b) = hsl(hue, 0.65, light);
    0xff00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

fn hsl(hue: f32, saturation: f32, light: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * light - 1.0).abs()) * saturation;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = light - c / 2.0;
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(r), byte(g), byte(b))
}

pub fn song_info(analysis: Option<&Analysis>) -> SongInfo {
    let camelot = analysis
        .filter(|a| a.key_confidence >= 0.4)
        .and_then(Analysis::camelot)
        .unwrap_or_default();
    SongInfo {
        bpm: analysis
            .filter(|a| a.bpm > 0.0)
            .map(|a| format!("{:.0} BPM", a.bpm))
            .unwrap_or_default(),
        key_color: key_color(&camelot),
        camelot,
    }
}

pub fn picture_data(
    plan: &Transition,
    out_length: f64,
    out: Option<&Analysis>,
    inn: Option<&Analysis>,
    snap_step: f64,
) -> Picture {
    Picture::new(plan, out_length, out, inn, snap_step)
}

pub fn incoming_speed(plan: &Transition) -> f64 {
    in_speed(plan)
}

pub fn drag_step(
    settings: &Settings,
    out: Option<&Analysis>,
    out_length: f64,
    inn: Option<&Analysis>,
    seconds: f64,
    inn_side: bool,
) -> f64 {
    let beat = transition::drag_step(settings, out, out_length, inn, seconds, inn_side);
    if beat > 0.0 { beat } else { FREE_STEP }
}

pub fn editor_data(
    settings: Settings,
    saved: Settings,
    out: Option<&Analysis>,
    out_length: f64,
    inn: Option<&Analysis>,
    seconds: f64,
    snap_step: f64,
) -> EditorData {
    use transition::*;
    let why_unmatched = transition::why_unmatched(out, out_length, inn, seconds, settings.bars).unwrap_or_default();
    let plan = transition::plan(settings, out, out_length, inn, seconds);
    let picture = Picture::new(&plan, out_length, out, inn, snap_step);
    let choices = vec![
        choice("Length", &LENGTH_CHOICES, settings.bars, 0xff8a_8a92),
        choice("Volume", &VOLUME_CHOICES, settings.volume, 0xff4f_b3ff),
        choice("EQ", &EQ_CHOICES, settings.eq, 0xfff5_c542),
        choice("Filter", &FILTER_CHOICES, settings.filter, 0xffc5_8af9),
        choice("Effects", &EFFECT_CHOICES, settings.effect, 0xffe8_e8ec),
        choice("Looping", &LOOP_CHOICES, settings.looping, 0xffff_9f43),
    ];
    EditorData {
        presets: Preset::OFFERED.iter().map(|p| p.name().to_string()).collect(),
        preset_selected: Preset::OFFERED.iter().position(|p| *p == settings.preset).map_or(-1, |i| i as i32),
        choices,
        changed: settings != saved,
        can_beatmatch: why_unmatched.is_empty(),
        why_unmatched,
        beatmatch: settings.beatmatch,
        key_lock: settings.key_lock,
        from: song_info(out),
        to: song_info(inn),
        picture,
    }
}

fn choice<T: Copy + PartialEq>(title: &str, values: &[(T, &str)], selected: T, accent: u32) -> Choice {
    Choice {
        title: title.to_string(),
        options: values.iter().map(|(_, label)| (*label).to_string()).collect(),
        selected: transition::index_of(values, selected),
        accent,
    }
}

pub fn pick_preset(
    mut settings: Settings,
    index: usize,
    out: Option<&Analysis>,
    inn: Option<&Analysis>,
) -> Result<Settings, String> {
    let preset = *Preset::OFFERED.get(index).ok_or_else(|| "Unknown transition preset".to_string())?;
    let bars = settings.bars;
    settings = if preset == Preset::Auto {
        Settings::auto(out, inn, true)
    } else {
        Settings::preset(preset)
    };
    settings.bars = bars;
    Ok(settings)
}

pub fn pick_choice(mut settings: Settings, part: usize, index: usize) -> Result<Settings, String> {
    use transition::*;
    fn at<T: Copy>(values: &[(T, &str)], index: usize) -> Result<T, String> {
        values.get(index).map(|(value, _)| *value).ok_or_else(|| "Unknown transition choice".to_string())
    }
    match part {
        // A length of its own is a choice like the others: it makes the
        // transition custom, so it is kept when saved and not read as Auto.
        0 => {
            settings.bars = at(&LENGTH_CHOICES, index)?;
            settings.preset = Preset::Custom;
        }
        1 => {
            settings.volume = at(&VOLUME_CHOICES, index)?;
            settings.preset = Preset::Custom;
        }
        2 => {
            settings.eq = at(&EQ_CHOICES, index)?;
            settings.preset = Preset::Custom;
        }
        3 => {
            settings.filter = at(&FILTER_CHOICES, index)?;
            settings.preset = Preset::Custom;
        }
        4 => {
            settings.effect = at(&EFFECT_CHOICES, index)?;
            settings.preset = Preset::Custom;
        }
        5 => {
            settings.looping = at(&LOOP_CHOICES, index)?;
            settings.preset = Preset::Custom;
        }
        _ => return Err("Unknown transition choice".into()),
    }
    Ok(settings)
}

pub fn drag(
    mut settings: Settings,
    side: usize,
    delta: f64,
    exact: bool,
    out: Option<&Analysis>,
    out_length: f64,
    inn: Option<&Analysis>,
    seconds: f64,
) -> Result<Settings, String> {
    if side > 1 {
        return Err("Unknown transition side".into());
    }
    let plan = transition::plan(settings, out, out_length, inn, seconds);
    let speed = plan
        .lanes
        .iter()
        .find(|lane| lane.side == Side::In && lane.param == Param::Speed)
        .and_then(|lane| lane.value(0.0))
        .unwrap_or(1.0) as f64;
    let step = if exact { 0.0 } else { drag_step(&settings, out, out_length, inn, seconds, side == 1) };
    if side == 0 {
        settings.out_shift = transition::snap(settings.out_shift + delta, step);
    } else {
        settings.in_shift = transition::snap(settings.in_shift + delta * speed, step);
    }
    settings.preset = Preset::Custom;
    Ok(settings)
}

pub fn toggle_beatmatch(mut settings: Settings, can_beatmatch: bool) -> Settings {
    if can_beatmatch {
        settings.beatmatch = !settings.beatmatch;
        settings.preset = Preset::Custom;
    }
    settings
}

pub fn toggle_key_lock(mut settings: Settings, can_beatmatch: bool) -> Settings {
    if can_beatmatch && settings.beatmatch {
        settings.key_lock = !settings.key_lock;
        settings.preset = Preset::Custom;
    }
    settings
}

impl Picture {
    fn new(
        plan: &Transition,
        out_length: f64,
        out: Option<&Analysis>,
        inn: Option<&Analysis>,
        snap_step: f64,
    ) -> Self {
        let pad = (plan.length * 0.3).max(2.0);
        let from = -pad;
        let to = plan.length + pad;
        let x = |t: f64| (t - from) / (to - from) * PICTURE_WIDTH;
        let speed = in_speed(plan);
        let out_start = plan.out_at.unwrap_or(out_length - plan.length);
        let (out_wave, out_low) = wave(plan, out_length, from, to, speed, out, Side::Out);
        let (in_wave, in_low) = wave(plan, out_length, from, to, speed, inn, Side::In);
        let (bars, beats) = grid(from, to, snap_step);
        Self {
            card_from: (x(0.0) / PICTURE_WIDTH) as f32,
            card_to: (x(plan.length) / PICTURE_WIDTH) as f32,
            span: (to - from) as f32,
            out_wave,
            out_low,
            in_wave,
            in_low,
            volume: lanes(plan, from, to, Param::Volume),
            bass: lanes(plan, from, to, Param::Bass),
            filter: filters(plan, from, to),
            effect: lanes(plan, from, to, Param::Send),
            bars,
            beats,
            left: from,
            right: to,
            in_speed: speed,
            out_start,
            in_from: plan.in_from,
            palette: Palette {
                out_wave: 0xffef_e6dc,
                in_wave: 0xff2f_62e0,
                low: 0xffe0_884a,
                volume: 0xff4f_b3ff,
                bass: 0xfff5_c542,
                filter: 0xffc5_8af9,
                effect: 0xffe8_e8ec,
            },
        }
    }
}

fn in_speed(plan: &Transition) -> f64 {
    plan.lanes
        .iter()
        .find(|lane| lane.side == Side::In && lane.param == Param::Speed)
        .and_then(|lane| lane.value(0.0))
        .unwrap_or(1.0) as f64
}

fn half(side: Side) -> (f64, f64) {
    let h = PICTURE_HEIGHT / 2.0;
    if side == Side::Out { (0.0, h) } else { (h, h) }
}

fn y(side: Side, value: f32) -> f64 {
    let (top, h) = half(side);
    top + h * 0.16 + (1.0 - value.clamp(0.0, 1.0) as f64) * h * 0.68
}

fn line(
    plan: &Transition,
    from: f64,
    to: f64,
    side: Side,
    value: impl Fn(f64) -> Option<f32>,
) -> String {
    let mut path = String::new();
    for i in 0..=STEPS {
        let t = plan.length * i as f64 / STEPS as f64;
        let Some(v) = value(t) else { return String::new() };
        let x = (t - from) / (to - from) * PICTURE_WIDTH;
        path.push_str(&format!("{} {:.1} {:.1} ", if path.is_empty() { 'M' } else { 'L' }, x, y(side, v)));
    }
    path
}

fn value(plan: &Transition, side: Side, param: Param, t: f64) -> Option<f32> {
    plan.lanes.iter().find(|lane| lane.side == side && lane.param == param)?.value(t)
}

fn lanes(plan: &Transition, from: f64, to: f64, param: Param) -> String {
    nonempty(format!(
        "{}{}",
        line(plan, from, to, Side::Out, |t| value(plan, Side::Out, param, t)),
        line(plan, from, to, Side::In, |t| value(plan, Side::In, param, t)),
    ))
}

fn filters(plan: &Transition, from: f64, to: f64) -> String {
    let span = (1000.0f32).ln();
    let out = line(plan, from, to, Side::Out, |t| {
        let hz = value(plan, Side::Out, Param::HighPass, t)?;
        Some(1.0 - (hz.max(20.0) / 20.0).ln() / span)
    });
    let inn = line(plan, from, to, Side::In, |t| {
        let hz = value(plan, Side::In, Param::LowPass, t)?;
        Some((hz.clamp(20.0, 20_000.0) / 20.0).ln() / span)
    });
    nonempty(format!("{out}{inn}"))
}

fn grid(from: f64, to: f64, step: f64) -> (String, String) {
    let (mut bars, mut beats) = (String::new(), String::new());
    if step > 0.0 {
        let first = (from / step).ceil() as i64;
        let last = (to / step).floor() as i64;
        for n in first..=last {
            let x = (n as f64 * step - from) / (to - from) * PICTURE_WIDTH;
            let path = format!("M {x:.1} 0 L {x:.1} {PICTURE_HEIGHT} ");
            if n.rem_euclid(4) == 0 { bars.push_str(&path) } else { beats.push_str(&path) }
        }
    }
    (nonempty(bars), nonempty(beats))
}

fn wave(
    plan: &Transition,
    out_length: f64,
    from: f64,
    to: f64,
    speed: f64,
    analysis: Option<&Analysis>,
    side: Side,
) -> (String, String) {
    let nothing = || (nonempty(String::new()), nonempty(String::new()));
    let Some(a) = analysis.filter(|a| a.waveform.len() >= 3 && a.length > 0.0) else { return nothing() };
    let frames = a.waveform.len() / 3;
    let rate = frames as f64 / a.length as f64;
    let start = plan.out_at.unwrap_or(out_length - plan.length);
    let at = |t: f64| match side {
        Side::Out => start + t,
        Side::In => plan.in_from + t * speed,
    };
    let columns = 3 * PICTURE_WIDTH as usize;
    let step = (to - from) / PICTURE_WIDTH;
    let left = from - (to - from);
    let mut levels = Vec::with_capacity(columns);
    for column in 0..columns {
        let t = left + step * column as f64;
        let (a0, a1) = (at(t), at(t + step));
        let (f0, f1) = ((a0 * rate).floor() as isize, (a1 * rate).ceil() as isize);
        if f1 <= 0 || f0 >= frames as isize { continue; }
        let (mut all, mut low) = (0u8, 0u8);
        for f in f0.max(0) as usize..(f1.max(f0 + 1) as usize).min(frames) {
            let bands = &a.waveform[f * 3..f * 3 + 3];
            all = all.max(*bands.iter().max().unwrap_or(&0));
            low = low.max(bands[0]);
        }
        let x = ((t + step / 2.0 - from) / (to - from) * PICTURE_WIDTH) + PICTURE_WIDTH;
        levels.push((x, all as f64 / 255.0, low as f64 / 255.0));
    }
    if levels.is_empty() { return nothing(); }
    let (top, h) = half(side);
    let middle = top + h / 2.0;
    let reach = h * 0.44;
    let shape = |pick: fn(&(f64, f64, f64)) -> f64| {
        let mut path = String::new();
        for (i, level) in levels.iter().enumerate() {
            path.push_str(&format!("{} {:.1} {:.1} ", if i == 0 { 'M' } else { 'L' }, level.0, middle - pick(level) * reach));
        }
        for level in levels.iter().rev() {
            path.push_str(&format!("L {:.1} {:.1} ", level.0, middle + pick(level) * reach));
        }
        path.push('Z');
        path
    };
    (shape(|level| level.1), shape(|level| level.2 * level.2))
}

fn nonempty(path: String) -> String {
    if path.is_empty() { "M 0 0".into() } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_data_keeps_auto_unselected_and_reports_changes() {
        let settings = Settings::preset(Preset::Auto);
        let mut saved = settings;
        saved.bars = 4;
        let view = editor_data(settings, saved, None, 180.0, None, 8.0, 0.5);
        assert_eq!(view.preset_selected, 0);
        assert!(view.changed);
        assert!(!view.can_beatmatch);
        assert_eq!(view.choices.len(), 6);
        assert!(view.picture.card_from < view.picture.card_to);
        assert_eq!(view.picture.palette.out_wave, 0xffef_e6dc);
    }

    #[test]
    fn editor_operations_reject_bad_choices_and_preserve_preset_length() {
        let mut settings = Settings::preset(Preset::Fade);
        settings.bars = 8;
        assert!(pick_choice(settings, 1, usize::MAX).is_err());
        let picked = pick_preset(settings, 0, None, None).unwrap();
        assert_eq!(picked.bars, 8);
        assert!(pick_preset(settings, usize::MAX, None, None).is_err());
    }

    #[test]
    fn changing_the_length_makes_an_auto_transition_custom() {
        let auto = Settings::preset(Preset::Auto);
        let index = transition::LENGTH_CHOICES.iter().position(|(bars, _)| *bars == 8).unwrap();
        let picked = pick_choice(auto, 0, index).unwrap();
        assert_eq!(picked.bars, 8);
        // Not Auto, so saving keeps it.
        assert_eq!(picked.preset, Preset::Custom);
    }
}
