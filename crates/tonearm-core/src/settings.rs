//! Settings both apps keep the same way: what each choice in Settings
//! means, and how it is read from and saved to the store. The apps show
//! the choices in the same order as these lists.

use crate::store;

/// Crossfade lengths offered in Settings, in seconds. Zero is off.
pub const CROSSFADE_CHOICES: [u32; 9] = [0, 1, 2, 3, 4, 6, 8, 10, 12];
/// Their names in the dropdown.
pub const CROSSFADE_NAMES: [&str; 9] = ["Off", "1 s", "2 s", "3 s", "4 s", "6 s", "8 s", "10 s", "12 s"];

/// The crossfade length saved in Settings, in seconds. Off when none is.
pub fn crossfade() -> u32 {
    store::load("crossfade")
        .and_then(|text| text.trim().parse().ok())
        .filter(|seconds| CROSSFADE_CHOICES.contains(seconds))
        .unwrap_or(0)
}

/// Which of `CROSSFADE_CHOICES` is saved.
pub fn crossfade_choice() -> usize {
    let seconds = crossfade();
    CROSSFADE_CHOICES.iter().position(|&c| c == seconds).unwrap_or(0)
}

/// Saves one of `CROSSFADE_CHOICES`, giving its length in seconds.
pub fn set_crossfade_choice(choice: i32) -> u32 {
    let seconds = CROSSFADE_CHOICES.get(choice.max(0) as usize).copied().unwrap_or(0);
    let _ = store::save("crossfade", &seconds.to_string());
    seconds
}

/// Whether a song changed by hand (skip, previous, picking another to play)
/// crossfades with the length above rather than cutting. Off when not saved.
pub fn crossfade_manual() -> bool {
    store::load("crossfade-manual").is_some_and(|text| text.trim() == "1")
}

/// Saves whether songs changed by hand crossfade.
pub fn set_crossfade_manual(on: bool) {
    let _ = store::save("crossfade-manual", if on { "1" } else { "0" });
}

/// Volume levelling choices in Settings: the loudness each aims for, in
/// LKFS, and how far it may lift a quiet song, in dB. Off is first.
pub const LEVELLING: [(f32, f32); 4] = [(0.0, 0.0), (-19.0, 0.0), (-14.0, 0.0), (-11.0, 4.0)];
pub const LEVELLING_NAMES: [&str; 4] = ["Off", "Quiet", "Normal", "Loud"];

/// The volume levelling choice saved in Settings. Off when none is (the
/// user's choice, on desktop and phone alike).
pub fn levelling() -> i32 {
    store::load("levelling")
        .and_then(|text| text.trim().parse().ok())
        .filter(|choice: &i32| (0..LEVELLING.len() as i32).contains(choice))
        .unwrap_or(0)
}

/// Saves a levelling choice, giving it back in range.
pub fn set_levelling(choice: i32) -> i32 {
    let choice = choice.clamp(0, LEVELLING.len() as i32 - 1);
    let _ = store::save("levelling", &choice.to_string());
    choice
}

/// The gain that brings a song YouTube measures at `lkfs` to what levelling
/// `choice` aims for. One while levelling is off or the loudness unknown.
pub fn levelling_gain(choice: i32, lkfs: Option<f32>) -> f32 {
    let Some(&(target, lift)) = LEVELLING.get(choice.max(0) as usize).filter(|_| choice > 0) else {
        return 1.0;
    };
    let Some(lkfs) = lkfs else {
        return 1.0;
    };
    let db = (target - lkfs).clamp(-20.0, lift);
    10f32.powf(db / 20.0)
}

/// Languages lyrics can be translated into in Settings, as the tag Google
/// Translate takes. The first, empty, is off.
pub const TRANSLATE_CHOICES: [&str; 13] = [
    "", "en", "es", "fr", "de", "it", "pt", "ru", "ja", "ko", "zh-CN", "zh-TW", "id",
];
/// Their names in the dropdown.
pub const TRANSLATE_NAMES: [&str; 13] = [
    "Off",
    "English",
    "Spanish",
    "French",
    "German",
    "Italian",
    "Portuguese",
    "Russian",
    "Japanese",
    "Korean",
    "Chinese (Simplified)",
    "Chinese (Traditional)",
    "Indonesian",
];

/// The language saved to translate lyrics into, as an index into
/// `TRANSLATE_CHOICES`. Off when none is saved.
pub fn translate() -> usize {
    store::load("translate")
        .and_then(|tag| TRANSLATE_CHOICES.iter().position(|&t| t == tag.trim()))
        .unwrap_or(0)
}

/// Saves a language choice, giving it back in range.
pub fn set_translate(choice: i32) -> usize {
    let index = (choice.max(0) as usize).min(TRANSLATE_CHOICES.len() - 1);
    let _ = store::save("translate", TRANSLATE_CHOICES[index]);
    index
}

/// What the Now playing panel shows for the cover, as chosen in Settings.
pub const PANEL_CANVAS: i32 = 0;
pub const PANEL_ANIMATED: i32 = 1;
/// What expanded Now playing (all of Now Playing, on a phone) shows.
pub const WIDE_ANIMATED: i32 = 0;
pub const WIDE_NAMES: [&str; 2] = ["Animated cover", "Static cover"];

/// The cover choices, for the panel and the expanded view. The moving
/// album cover in both when none are saved.
pub fn cover_choices() -> (i32, i32) {
    let text = store::load("cover-choices").unwrap_or_default();
    let mut parts = text.trim().split(',').map(|p| p.parse::<i32>().ok());
    let panel = parts.next().flatten().filter(|c| (0..3).contains(c)).unwrap_or(PANEL_ANIMATED);
    let wide = parts.next().flatten().filter(|c| (0..2).contains(c)).unwrap_or(WIDE_ANIMATED);
    (panel, wide)
}

pub fn save_cover_choices(choices: (i32, i32)) {
    let _ = store::save("cover-choices", &format!("{},{}", choices.0, choices.1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_choices() {
        assert_eq!(CROSSFADE_NAMES.len(), CROSSFADE_CHOICES.len());
        assert_eq!(TRANSLATE_NAMES.len(), TRANSLATE_CHOICES.len());
        assert_eq!(LEVELLING_NAMES.len(), LEVELLING.len());
    }

    #[test]
    fn levelling_gain_is_capped() {
        assert_eq!(levelling_gain(0, Some(-30.0)), 1.0);
        assert_eq!(levelling_gain(2, None), 1.0);
        // Normal aims for -14: a song at -8 comes down 6 dB.
        assert!((levelling_gain(2, Some(-8.0)) - 10f32.powf(-6.0 / 20.0)).abs() < 1e-6);
        // Normal never lifts; Loud lifts at most 4 dB.
        assert_eq!(levelling_gain(2, Some(-30.0)), 1.0);
        assert!((levelling_gain(3, Some(-30.0)) - 10f32.powf(4.0 / 20.0)).abs() < 1e-6);
    }
}
