//! The sleep timer, the same on the computer and the phone: it pauses the
//! music after a while, or stops it once the playing song ends.

use std::time::{Duration, Instant};

/// The choices, in the order the apps show them. 0 is off.
pub const OPTIONS: [&str; 7] = [
    "Off",
    "5 minutes",
    "15 minutes",
    "30 minutes",
    "45 minutes",
    "1 hour",
    "End of song",
];

/// The minutes behind choices 1 to 5.
const MINUTES: [u64; 5] = [5, 15, 30, 45, 60];

/// The choice that stops once the playing song ends.
pub const END_OF_SONG: usize = 6;

/// What the timer is set to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Sleep {
    #[default]
    Off,
    /// Pauses at this time.
    At(Instant),
    /// Stops once the playing song ends. Nothing follows it, not even a
    /// crossfade.
    EndOfSong,
}

impl Sleep {
    /// The timer for a choice in `OPTIONS`, started now.
    pub fn from_choice(choice: usize) -> Sleep {
        match choice {
            END_OF_SONG => Sleep::EndOfSong,
            n if n >= 1 => MINUTES
                .get(n - 1)
                .map_or(Sleep::Off, |m| Sleep::At(Instant::now() + Duration::from_secs(m * 60))),
            _ => Sleep::Off,
        }
    }

    /// Whether the music should stop at the end of the playing song.
    pub fn ends_with_song(self) -> bool {
        self == Sleep::EndOfSong
    }

    /// Whether a timed timer has run out, and the music should pause now.
    pub fn is_due(self) -> bool {
        matches!(self, Sleep::At(at) if Instant::now() >= at)
    }

    /// What the player shows beside the timer: the minutes left, rounded
    /// up, or "End of song". Empty when off.
    pub fn text(self) -> String {
        match self {
            Sleep::Off => String::new(),
            Sleep::EndOfSong => "End of song".into(),
            Sleep::At(at) => {
                let left = at.saturating_duration_since(Instant::now());
                format!("{} min left", (left.as_secs() + 59) / 60)
            }
        }
    }
}

/// What a timer that ran out says, on both apps.
pub const STOPPED: &str = "Sleep timer stopped the music";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices() {
        assert_eq!(Sleep::from_choice(0), Sleep::Off);
        assert_eq!(Sleep::from_choice(END_OF_SONG), Sleep::EndOfSong);
        assert_eq!(Sleep::from_choice(99), Sleep::Off);
        let five = Sleep::from_choice(1);
        assert!(!five.is_due());
        assert_eq!(five.text(), "5 min left");
        assert_eq!(Sleep::from_choice(5).text(), "60 min left");
        assert!(Sleep::At(Instant::now()).is_due());
        assert!(Sleep::EndOfSong.ends_with_song());
        assert_eq!(OPTIONS[END_OF_SONG], "End of song");
    }
}
