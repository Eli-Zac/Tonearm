//! What you listen to and open, kept on this computer to tailor search.
//!
//! Every song, artist, album and playlist carries an affinity score. Each
//! listen or visit adds to it and the score halves every few weeks, so
//! what you play now counts for more than what you played last year.
//! Artists also get a score from the songs of theirs you play.
//!
//! The file is encrypted with your Windows account, like the sign in.

use crate::innertube::model::{Item, Kind};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Days for a score to fall to half.
const HALF_LIFE_DAYS: f64 = 21.0;
/// Entries kept. The weakest go first.
const LIMIT: usize = 1500;
const FILE: &str = "history";

/// Kind labels and play counts YouTube Music mixes into subtitles.
const LABELS: [&str; 9] = [
    "Song", "Video", "Single", "Album", "EP", "Playlist", "Episode", "Podcast", "Profile",
];
const COUNTS: [&str; 6] = [" views", " plays", " likes", " audience", " subscribers", " songs"];

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A score as it stands at `now`, after decay since `at`.
fn decayed(score: f64, at: u64, now: u64) -> f64 {
    let days = now.saturating_sub(at) as f64 / 86_400.0;
    score * 0.5f64.powf(days / HALF_LIFE_DAYS)
}

pub fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// Who made an item: the first part of its subtitle that is not a kind
/// label or a count. For an artist, its own name.
pub fn artist_of(item: &Item) -> String {
    if item.kind == Kind::Artist {
        return item.title.clone();
    }
    item.subtitle
        .split(" • ")
        .map(str::trim)
        .find(|p| {
            !p.is_empty()
                && !LABELS.contains(p)
                && !COUNTS.iter().any(|c| p.ends_with(c))
                && !p.contains(':')
        })
        .unwrap_or("")
        .to_string()
}

/// The id that names an item, whatever kind it is.
pub fn key_of(item: &Item) -> Option<String> {
    item.video_id
        .clone()
        .or_else(|| item.browse_id.clone())
        .or_else(|| item.playlist_id.clone())
}

struct Entry {
    item: Item,
    score: f64,
    at: u64,
}

#[derive(Default)]
pub struct History {
    entries: HashMap<String, Entry>,
    /// Folded artist name to (score, at).
    artists: HashMap<String, (f64, u64)>,
}

impl History {
    pub fn load() -> Self {
        let mut history = History::default();
        let Some(text) = crate::store::load(FILE) else {
            return history;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            return history;
        };
        for e in v["entries"].as_array().into_iter().flatten() {
            let item = Item::from_json(e);
            if let Some(key) = key_of(&item) {
                let score = e["score"].as_f64().unwrap_or(0.0);
                let at = e["at"].as_u64().unwrap_or(0);
                history.entries.insert(key, Entry { item, score, at });
            }
        }
        if let Some(map) = v["artists"].as_object() {
            for (name, a) in map {
                let score = a[0].as_f64().unwrap_or(0.0);
                let at = a[1].as_u64().unwrap_or(0);
                history.artists.insert(name.clone(), (score, at));
            }
        }
        history
    }

    /// The history as text for `store::save`.
    pub fn serialize(&self) -> String {
        let entries: Vec<Value> = self
            .entries
            .values()
            .map(|e| {
                let mut v = e.item.to_json();
                v["score"] = json!(e.score);
                v["at"] = json!(e.at);
                v
            })
            .collect();
        let artists: serde_json::Map<String, Value> = self
            .artists
            .iter()
            .map(|(name, (score, at))| (name.clone(), json!([score, at])))
            .collect();
        json!({"entries": entries, "artists": artists}).to_string()
    }

    pub fn save(text: String) {
        let _ = crate::store::save(FILE, &text);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.artists.clear();
    }

    /// Deletes the saved file. Run it on the same thread as `save`, so a
    /// save still waiting cannot bring the history back.
    pub fn delete_file() {
        crate::store::clear(FILE);
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds `weight` to an item and to its artist. A listen is worth 1, the
    /// start of a song a little, opening a page a little more.
    pub fn record(&mut self, item: &Item, weight: f64) {
        let Some(key) = key_of(item) else { return };
        let now = now();
        let entry = self.entries.entry(key).or_insert_with(|| Entry {
            item: item.clone(),
            score: 0.0,
            at: now,
        });
        entry.score = decayed(entry.score, entry.at, now) + weight;
        entry.at = now;
        // Keep the freshest details, but never lose art we had.
        let thumb = std::mem::take(&mut entry.item.thumb);
        entry.item = item.clone();
        if entry.item.thumb.is_empty() {
            entry.item.thumb = thumb;
        }
        let artist = fold(&artist_of(item));
        if !artist.is_empty() {
            let (score, at) = self.artists.entry(artist).or_insert((0.0, now));
            *score = decayed(*score, *at, now) + weight;
            *at = now;
        }
        self.prune(now);
    }

    fn prune(&mut self, now: u64) {
        if self.entries.len() > LIMIT {
            let mut scores: Vec<(String, f64)> = self
                .entries
                .iter()
                .map(|(k, e)| (k.clone(), decayed(e.score, e.at, now)))
                .collect();
            scores.sort_by(|a, b| a.1.total_cmp(&b.1));
            for (key, _) in scores.into_iter().take(self.entries.len() - LIMIT) {
                self.entries.remove(&key);
            }
        }
        if self.artists.len() > LIMIT {
            let mut scores: Vec<(String, f64)> = self
                .artists
                .iter()
                .map(|(k, (s, at))| (k.clone(), decayed(*s, *at, now)))
                .collect();
            scores.sort_by(|a, b| a.1.total_cmp(&b.1));
            for (key, _) in scores.into_iter().take(self.artists.len() - LIMIT) {
                self.artists.remove(&key);
            }
        }
    }

    fn artist_score(&self, name: &str, now: u64) -> f64 {
        self.artists
            .get(&fold(name))
            .map(|(s, at)| decayed(*s, *at, now))
            .unwrap_or(0.0)
    }

    /// What was played or opened last, newest first. Songs when `songs`
    /// is true, otherwise albums, playlists and artists.
    pub fn recent(&self, songs: bool, limit: usize) -> Vec<Item> {
        let mut entries: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| e.item.video_id.is_some() == songs)
            .filter(|e| songs || e.item.browse_id.is_some())
            .collect();
        entries.sort_by(|a, b| b.at.cmp(&a.at));
        entries.into_iter().take(limit).map(|e| e.item.clone()).collect()
    }

    /// The artists you play most, best first, as folded names.
    pub fn top_artists(&self, limit: usize) -> Vec<String> {
        let now = now();
        let mut artists: Vec<(&String, f64)> = self
            .artists
            .iter()
            .map(|(name, (score, at))| (name, decayed(*score, *at, now)))
            .collect();
        artists.sort_by(|a, b| b.1.total_cmp(&a.1));
        artists.into_iter().take(limit).map(|(name, _)| name.clone()).collect()
    }

    /// How much an item from YouTube Music matches your taste: its own
    /// score if you have played or opened it, plus part of its artist's.
    pub fn affinity(&self, item: &Item) -> f64 {
        let now = now();
        let own = key_of(item)
            .and_then(|k| self.entries.get(&k))
            .map(|e| decayed(e.score, e.at, now))
            .unwrap_or(0.0);
        let artist = artist_of(item);
        let by = if artist.is_empty() {
            0.0
        } else {
            self.artist_score(&artist, now)
        };
        // For an artist the artist score is its own score.
        if item.kind == Kind::Artist {
            own.max(by)
        } else {
            own + by * 0.35
        }
    }

    /// Items from your history that match what was typed, best first, with
    /// a rank that mixes how well they match and how much you play them.
    pub fn matches(&self, query: &str, limit: usize) -> Vec<(Item, f64)> {
        let words: Vec<String> = fold(query).split_whitespace().map(str::to_string).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let now = now();
        let mut found: Vec<(Item, f64)> = self
            .entries
            .values()
            .filter_map(|e| {
                let quality = match_quality(&words, &e.item)?;
                let score = decayed(e.score, e.at, now);
                (score >= 0.2).then(|| (e.item.clone(), quality + (1.0 + score).ln() * 1.5))
            })
            .collect();
        found.sort_by(|a, b| b.1.total_cmp(&a.1));
        found.truncate(limit);
        found
    }
}

/// How well typed words match an item, or None when some word does not
/// start any word of its title or artist. 3 when the title starts with
/// the whole query, 2 when the title alone matches, 1 through the artist.
fn match_quality(words: &[String], item: &Item) -> Option<f64> {
    let title = fold(&item.title);
    let artist = fold(&artist_of(item));
    let starts = |text: &str, w: &str| text.split_whitespace().any(|t| t.starts_with(w));
    if !words.iter().all(|w| starts(&title, w) || starts(&artist, w)) {
        return None;
    }
    let query = words.join(" ");
    if title.starts_with(&query) {
        Some(3.0)
    } else if words.iter().all(|w| starts(&title, w)) {
        Some(2.0)
    } else {
        Some(1.0)
    }
}

/// Moves items you are likely after up a list, keeping YouTube Music's
/// order otherwise. A step down the list costs about what one recent
/// listen of the artist is worth, so taste only reorders close calls and
/// lifts things you clearly play a lot.
pub fn rerank(history: &History, items: &mut Vec<Item>) {
    if history.is_empty() || items.len() < 2 {
        return;
    }
    let mut scored: Vec<(f64, Item)> = std::mem::take(items)
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let lift = (1.0 + history.affinity(&item)).ln() * 1.6;
            (lift - i as f64 * 0.6, item)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    *items = scored.into_iter().map(|(_, item)| item).collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, title: &str, artist: &str) -> Item {
        Item {
            kind: Kind::Song,
            title: title.into(),
            subtitle: format!("Song • {artist} • 1.2M plays"),
            video_id: Some(id.into()),
            ..Item::default()
        }
    }

    #[test]
    fn artist_skips_labels_and_counts() {
        assert_eq!(artist_of(&song("a", "One", "Daft Punk")), "Daft Punk");
    }

    #[test]
    fn played_songs_match_by_title_or_artist() {
        let mut h = History::default();
        h.record(&song("a", "No Pole", "Don Toliver"), 1.0);
        h.record(&song("b", "Something Else", "Other"), 1.0);
        let hits = h.matches("don", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.title, "No Pole");
        assert_eq!(h.matches("no p", 5)[0].0.title, "No Pole");
        assert!(h.matches("zzz", 5).is_empty());
    }

    #[test]
    fn rerank_lifts_an_artist_you_play() {
        let mut h = History::default();
        for _ in 0..4 {
            h.record(&song("x", "Hit", "Loved"), 1.0);
        }
        let mut items = vec![song("1", "A", "Stranger"), song("2", "B", "Loved")];
        rerank(&h, &mut items);
        assert_eq!(items[0].title, "B");
    }

    #[test]
    fn decay_halves_over_the_half_life() {
        let at = 1_000_000;
        let later = at + (HALF_LIFE_DAYS * 86_400.0) as u64;
        assert!((decayed(2.0, at, later) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn survives_a_round_trip() {
        let mut h = History::default();
        h.record(&song("a", "No Pole", "Don Toliver"), 1.0);
        let text = h.serialize();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["entries"].as_array().unwrap().len(), 1);
        assert!(v["artists"]["don toliver"].is_array());
    }
}
