//! What was playing when the app closed: the queue, the song in it, how
//! far into that song you were, and the volume. Reopening the app puts it
//! all back, paused, so pressing play carries on where you left off.
//!
//! The file is encrypted with your Windows account, like the history.

use crate::innertube::model::Item;
use serde_json::{json, Value};

/// The repeat button: off, the whole queue, or the one song.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
    pub fn number(self) -> i32 {
        self as i32
    }
    pub fn from_number(n: i64) -> Repeat {
        match n {
            1 => Repeat::All,
            2 => Repeat::One,
            _ => Repeat::Off,
        }
    }
}

/// Not "session": that name holds the sign in cookie.
const FILE: &str = "playback";
/// Songs kept before and after the playing one. A long radio queue can
/// grow without end, and most of it never gets played.
const BEFORE: usize = 50;
const AFTER: usize = 200;

#[derive(Debug, Default)]
pub struct Session {
    pub queue: Vec<Item>,
    pub index: usize,
    /// Seconds into the playing song.
    pub position: f32,
    /// The volume slider, from 0 to 1.
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: Repeat,
}

impl Session {
    /// The session as text for `store::save`. Trims the queue around the
    /// playing song.
    pub fn serialize(&self) -> String {
        let (queue, index) = (&self.queue, self.index);
        let start = index.saturating_sub(BEFORE);
        let end = (index + 1 + AFTER).min(queue.len());
        let items: Vec<Value> = queue
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .map(Item::to_json)
            .collect();
        json!({
            "queue": items,
            "index": index - start,
            "position": self.position,
            "volume": self.volume,
            "shuffle": self.shuffle,
            "repeat": self.repeat.number(),
        })
        .to_string()
    }

    pub fn parse(text: &str) -> Option<Session> {
        let v: Value = serde_json::from_str(text).ok()?;
        let queue: Vec<Item> = v["queue"]
            .as_array()?
            .iter()
            .map(Item::from_json)
            .collect();
        let index = v["index"].as_u64().unwrap_or(0) as usize;
        if index >= queue.len() {
            return None;
        }
        Some(Session {
            queue,
            index,
            position: v["position"].as_f64().unwrap_or(0.0).max(0.0) as f32,
            volume: v["volume"].as_f64().unwrap_or(1.0).clamp(0.0, 1.0) as f32,
            shuffle: v["shuffle"].as_bool().unwrap_or(false),
            repeat: Repeat::from_number(v["repeat"].as_i64().unwrap_or(0)),
        })
    }

    pub fn load() -> Option<Session> {
        Session::parse(&crate::store::load(FILE)?)
    }

    pub fn save(text: String) {
        let _ = crate::store::save(FILE, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(n: usize) -> Item {
        Item {
            title: format!("Song {n}"),
            video_id: Some(format!("v{n}")),
            artists: vec![("Wisp".into(), Some("UC1".into()))],
            ..Item::default()
        }
    }

    #[test]
    fn a_session_comes_back_as_it_was_saved() {
        let queue: Vec<Item> = (0..5).map(song).collect();
        let text = Session {
            queue: queue.clone(),
            index: 2,
            position: 61.5,
            volume: 0.4,
            shuffle: true,
            repeat: Repeat::One,
        }
        .serialize();
        let back = Session::parse(&text).unwrap();
        assert!(back.shuffle);
        assert_eq!(back.repeat, Repeat::One);
        assert_eq!(back.index, 2);
        assert_eq!(back.position, 61.5);
        assert_eq!(back.volume, 0.4);
        assert_eq!(back.queue.len(), 5);
        assert_eq!(back.queue[2].title, "Song 2");
        assert_eq!(back.queue[2].artists, queue[2].artists);
    }

    #[test]
    fn a_long_queue_is_trimmed_around_the_playing_song() {
        let queue: Vec<Item> = (0..1000).map(song).collect();
        let saved = Session {
            queue,
            index: 500,
            ..Session::default()
        };
        let back = Session::parse(&saved.serialize()).unwrap();
        assert_eq!(back.queue.len(), BEFORE + 1 + AFTER);
        assert_eq!(back.queue[back.index].title, "Song 500");
    }

    #[test]
    fn a_broken_session_is_ignored() {
        assert!(Session::parse("not json").is_none());
        assert!(Session::parse(r#"{"queue":[],"index":0}"#).is_none());
    }
}
