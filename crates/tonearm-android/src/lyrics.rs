//! Time synced lyrics for Now Playing, as the core finds them: the same
//! services, in the same order, as the desktop app. Times are milliseconds
//! into the song.

use tonearm_core::lyrics as core;

#[derive(uniffi::Record, Clone, Debug)]
pub struct Syllable {
    pub start: u32,
    pub end: u32,
    /// Ends with a space when the syllable closes a word.
    pub text: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct LyricLine {
    pub start: u32,
    pub end: u32,
    pub text: String,
    /// Empty when the source only times whole lines.
    pub syllables: Vec<Syllable>,
    /// Where the singer's lines sit: 0 left, 1 right, 2 centred for a group.
    pub align: u8,
    /// Backing vocals sung along with the line, drawn smaller under it.
    pub background: Vec<Syllable>,
    pub background_text: String,
    /// The translation to show under the line. Empty for none.
    pub translation: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    /// False for plain text: then every start and end is zero.
    pub synced: bool,
    /// Where they came from, such as "LRCLIB", to credit under them.
    pub source: String,
}

fn syllables(from: &[core::Syllable]) -> Vec<Syllable> {
    from.iter()
        .map(|s| Syllable {
            start: s.start,
            end: s.end,
            text: s.text.clone(),
        })
        .collect()
}

impl From<&core::Lyrics> for Lyrics {
    fn from(found: &core::Lyrics) -> Self {
        Lyrics {
            synced: found.synced,
            source: found.source.to_string(),
            lines: found
                .lines
                .iter()
                .map(|line| LyricLine {
                    start: line.start,
                    end: line.end,
                    text: line.text.clone(),
                    syllables: syllables(&line.syllables),
                    align: line.align,
                    background: syllables(&line.background),
                    background_text: line.background_text.clone(),
                    translation: line.translation.clone(),
                })
                .collect(),
        }
    }
}

/// "3:12" or "1:02:03" in seconds. Zero when it isn't a length.
pub fn seconds_of(duration: &str) -> f32 {
    let mut total = 0u32;
    for part in duration.trim().split(':') {
        let Ok(n) = part.parse::<u32>() else { return 0.0 };
        total = total * 60 + n;
    }
    total as f32
}
