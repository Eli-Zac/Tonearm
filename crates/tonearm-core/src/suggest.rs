//! What the search box offers as you type, the same on every app: up to
//! five suggested searches with the part you typed in bold, then the first
//! few results of the real search, and your recent searches while the box
//! is empty.
//!
//! The suggested searches and the search itself are asked for side by
//! side once typing pauses (`DELAY`), and kept a few minutes by what was
//! typed, so deleting a letter shows its results at once and pressing
//! Enter opens the results page without asking again.

use crate::history;
use crate::innertube::model::{Item, Kind, Page};
use crate::innertube::Client;
use crate::store;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long typing has to pause before suggestions are asked for.
pub const DELAY: Duration = Duration::from_millis(140);
/// How long fetched suggestions stay good enough to show again.
pub const FRESH: Duration = Duration::from_secs(300);
/// Suggested searches shown, at most.
pub const QUERIES: usize = 5;
/// Search results shown under them, at most.
pub const ITEMS: usize = 4;
/// Replies kept before the cache starts over.
const CACHE_KEPT: usize = 300;
/// Where recent searches are kept, and how many.
const RECENT: &str = "searches";
const RECENT_KEPT: usize = 8;

/// One reply: the suggested searches and the real search's results.
#[derive(Clone)]
pub struct Suggested {
    pub queries: Vec<String>,
    pub page: Page,
    pub at: Instant,
}

/// Asks for the suggested searches and the search's results at once.
/// The suggestion call's own items are YouTube Music's guesses and often
/// differ from what the search returns, so they are not used. Fails only
/// when both fail. Blocks, so call it from a worker.
pub fn fetch(client: &Client, query: &str) -> Result<Suggested, String> {
    let (queries, page) = std::thread::scope(|scope| {
        let queries = scope.spawn(|| client.suggest(query));
        let page = client.search(query);
        let queries = queries.join().unwrap_or_else(|_| Err(String::new()));
        (queries, page)
    });
    match (queries, page) {
        (Err(e), Err(_)) => Err(e),
        (queries, page) => Ok(Suggested {
            queries: queries.map(|(q, _)| q).unwrap_or_default(),
            page: page.unwrap_or_default(),
            at: Instant::now(),
        }),
    }
}

/// Replies by what was typed, lower case.
#[derive(Default)]
pub struct Cache {
    replies: HashMap<String, Suggested>,
}

impl Cache {
    /// The reply for `query`, while it is fresh.
    pub fn get(&self, query: &str) -> Option<Suggested> {
        self.replies
            .get(&query.trim().to_lowercase())
            .filter(|s| s.at.elapsed() < FRESH)
            .cloned()
    }

    pub fn put(&mut self, query: &str, found: Suggested) {
        if self.replies.len() > CACHE_KEPT {
            self.replies.clear();
        }
        self.replies.insert(query.trim().to_lowercase(), found);
    }
}

/// Splits a suggested search into the part that matches what was typed,
/// shown in bold, and the rest. When it does not start with the typed
/// text, nothing is bold.
pub fn split_typed(suggestion: &str, typed: &str) -> (String, String) {
    let mut end = 0;
    let mut typed_chars = typed.chars();
    for (index, c) in suggestion.char_indices() {
        match typed_chars.next() {
            Some(t) if t.to_lowercase().eq(c.to_lowercase()) => end = index + c.len_utf8(),
            Some(_) => return (String::new(), suggestion.to_string()),
            None => break,
        }
    }
    if typed_chars.next().is_some() {
        return (String::new(), suggestion.to_string());
    }
    (suggestion[..end].to_string(), suggestion[end..].to_string())
}

/// The suggested searches to show.
pub fn queries(found: &Suggested) -> Vec<String> {
    found.queries.iter().take(QUERIES).cloned().collect()
}

/// The first results of `page`, each thing once, at most `ITEMS`. The app
/// tailors the page first, as its results page would be, so the list is a
/// preview of that page.
pub fn items(page: &Page) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    for item in page.shelves.iter().flat_map(|s| &s.items) {
        let key = history::key_of(item);
        if key.is_some() && !items.iter().any(|i| history::key_of(i) == key) {
            items.push(item.clone());
            if items.len() == ITEMS {
                break;
            }
        }
    }
    items
}

/// "Artist", "Song • Journey", "Album • Daft Punk": what an item is and
/// who made it, without the play counts YouTube Music adds.
pub fn subtitle(item: &Item) -> String {
    if item.kind == Kind::Artist {
        return "Artist".into();
    }
    let labels = [
        "Song", "Video", "Single", "Album", "EP", "Playlist", "Episode", "Podcast", "Profile",
    ];
    let counts = [" views", " plays", " likes", " audience", " subscribers", " songs"];
    let parts: Vec<&str> = item
        .subtitle
        .split(" • ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let label = parts
        .first()
        .filter(|p| labels.contains(p))
        .copied()
        .unwrap_or(match item.kind {
            Kind::Song => "Song",
            Kind::Album => "Album",
            Kind::Playlist => "Playlist",
            _ => "",
        });
    let by = parts
        .iter()
        .find(|p| !labels.contains(p) && !counts.iter().any(|c| p.ends_with(c)));
    match by {
        Some(by) if !label.is_empty() => format!("{label} • {by}"),
        Some(by) => by.to_string(),
        None => label.to_string(),
    }
}

/// The recent searches saved on this device, newest first.
pub fn load_recent() -> Vec<String> {
    store::load(RECENT)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(RECENT_KEPT)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Puts `query` at the top of `recent`. Returns what to save with
/// `save_recent`, which the app may do off its own thread.
pub fn remember(recent: &mut Vec<String>, query: &str) -> String {
    let query = query.trim();
    recent.retain(|q| !q.eq_ignore_ascii_case(query));
    if !query.is_empty() {
        recent.insert(0, query.to_string());
    }
    recent.truncate(RECENT_KEPT);
    recent.join("\n")
}

pub fn save_recent(text: &str) {
    let _ = store::save(RECENT, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_part_is_bold() {
        assert_eq!(split_typed("Don Toliver", "don t"), ("Don T".into(), "oliver".into()));
        assert_eq!(split_typed("Don Toliver", "tol"), (String::new(), "Don Toliver".into()));
        assert_eq!(split_typed("Don", "Don Toliver"), (String::new(), "Don".into()));
        assert_eq!(split_typed("Ébauche", "é"), ("É".into(), "bauche".into()));
    }

    #[test]
    fn recent_keeps_newest_once() {
        let mut recent = vec!["a".to_string(), "B".to_string()];
        assert_eq!(remember(&mut recent, "b"), "b\na");
        for i in 0..20 {
            remember(&mut recent, &i.to_string());
        }
        assert_eq!(recent.len(), RECENT_KEPT);
        assert_eq!(recent[0], "19");
    }
}
