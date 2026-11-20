//! When a play counts, for both apps: told a few times a second whether
//! the song under a generation is playing, it says when to tell Last.fm
//! (and Discord) it started, paused or resumed, when it has played long
//! enough to scrobble, and when it counts as a listen in the history.

use std::time::{Duration, Instant};

/// Something to act on, from `Listen::tick`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moment {
    /// A new song started playing: tell the services, and leave a small
    /// mark in the history.
    Started,
    /// It played for half its length or four minutes, as Last.fm asks.
    Scrobble,
    /// Half a minute in, or half a short song: a real listen.
    Counted,
    Paused,
    Resumed,
}

#[derive(Default)]
pub struct Listen {
    generation: u64,
    played: Duration,
    last: Option<Instant>,
    scrobbled: bool,
    counted: bool,
    was_playing: bool,
}

impl Listen {
    /// The song under `generation`, `duration` seconds long, is `playing`
    /// or not. Gives what happened since the last tick.
    pub fn tick(&mut self, generation: u64, playing: bool, duration: f32) -> Vec<Moment> {
        let now = Instant::now();
        let mut moments = Vec::new();
        if self.generation != generation {
            if !playing {
                return moments;
            }
            *self = Listen {
                generation,
                was_playing: true,
                last: Some(now),
                ..Listen::default()
            };
            moments.push(Moment::Started);
            return moments;
        }
        if playing {
            if let Some(last) = self.last {
                self.played += now.duration_since(last).min(Duration::from_secs(1));
            }
            let needed = (duration / 2.0).min(240.0);
            if !self.scrobbled && duration > 30.0 && self.played.as_secs_f32() >= needed {
                self.scrobbled = true;
                moments.push(Moment::Scrobble);
            }
            // Skips only leave the small mark from the start.
            if !self.counted && self.played.as_secs_f32() >= needed.min(30.0).max(5.0) {
                self.counted = true;
                moments.push(Moment::Counted);
            }
        }
        if playing != self.was_playing {
            self.was_playing = playing;
            moments.push(if playing { Moment::Resumed } else { Moment::Paused });
        }
        self.last = playing.then_some(now);
        moments
    }
}

/// The first credited artist, the one Last.fm is told of: the first linked
/// artist, else the joined names up to the first comma or ampersand.
pub fn lead_artist(artists: &[(String, Option<String>)], artist: &str) -> String {
    if let Some((name, _)) = artists.iter().find(|(name, _)| !name.trim().is_empty()) {
        return name.trim().to_string();
    }
    let cut = [", ", " & "].iter().filter_map(|sep| artist.find(sep)).min().unwrap_or(artist.len());
    artist[..cut].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_song_starts_once_playing() {
        let mut listen = Listen::default();
        assert!(listen.tick(1, false, 200.0).is_empty());
        assert_eq!(listen.tick(1, true, 200.0), vec![Moment::Started]);
        assert!(listen.tick(1, true, 200.0).is_empty());
        assert_eq!(listen.tick(1, false, 200.0), vec![Moment::Paused]);
        assert_eq!(listen.tick(1, true, 200.0), vec![Moment::Resumed]);
    }

    #[test]
    fn lead_artist_cuts_at_the_first_join() {
        assert_eq!(lead_artist(&[], "A, B & C"), "A");
        assert_eq!(lead_artist(&[("X".into(), None)], "A, B"), "X");
    }
}
