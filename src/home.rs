//! The Home page, reshaped the way Spotify lays it out: where you left
//! off, mixes made for you, what you played lately and new music from the
//! artists you follow, then YouTube Music's own shelves.

use crate::history::{self, History};
use crate::innertube::model::{Item, Page, Shelf};
use crate::innertube::Client;
use std::collections::HashSet;

/// How many cards each of our own shelves shows.
const SHELF: usize = 12;

/// What the Home page is built from, fetched together off the UI thread.
pub struct Parts {
    home: Result<Page, String>,
    releases: Option<Page>,
    /// The account's play history. Only when signed in.
    played: Option<Page>,
    /// Folded names of the artists the account follows.
    followed: Vec<String>,
}

/// Fetches YouTube Music's home and the pages our shelves come from, side
/// by side. Blocks. Only the home itself has to arrive.
pub fn fetch(client: &Client) -> Parts {
    let signed_in = client.signed_in();
    std::thread::scope(|scope| {
        let releases = scope.spawn(|| client.browse("FEmusic_new_releases", None).ok());
        let played = scope.spawn(|| {
            signed_in
                .then(|| client.browse("FEmusic_history", None).ok())
                .flatten()
        });
        let followed = scope.spawn(|| {
            signed_in
                .then(|| client.browse("FEmusic_library_corpus_artists", None).ok())
                .flatten()
                .map(|page| {
                    page.shelves
                        .into_iter()
                        .flat_map(|s| s.items)
                        .map(|i| history::fold(&i.title))
                        .collect()
                })
                .unwrap_or_default()
        });
        let home = client.browse("FEmusic_home", None);
        Parts {
            home,
            releases: releases.join().ok().flatten(),
            played: played.join().ok().flatten(),
            followed: followed.join().unwrap_or_default(),
        }
    })
}

/// A mix YouTube Music made for the listener, such as "My Mix 1",
/// "Replay Mix" or "Discover Mix".
fn is_mix(item: &Item) -> bool {
    item.video_id.is_none()
        && item
            .title
            .to_lowercase()
            .split_whitespace()
            // "Remix" in an album's name is no mix made for you.
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            .any(|w| w == "mix" || w == "supermix")
}

/// Who made an item, folded: the linked artists, or the subtitle's.
fn makers(item: &Item) -> Vec<String> {
    let mut names: Vec<String> = item.artists.iter().map(|(n, _)| history::fold(n)).collect();
    let from_subtitle = history::fold(&history::artist_of(item));
    if !from_subtitle.is_empty() {
        names.push(from_subtitle);
    }
    names
}

/// Adds `item` unless one with the same key is there already.
fn add(list: &mut Vec<Item>, item: Item) {
    let key = history::key_of(&item);
    if key.is_none() || !list.iter().any(|i| history::key_of(i) == key) {
        list.push(item);
    }
}

fn shelf(title: &str, items: Vec<Item>) -> Option<Shelf> {
    (!items.is_empty()).then(|| Shelf {
        title: title.into(),
        carousel: true,
        items,
        more: None,
    })
}

/// Puts the Home page together from what `fetch` got and the listening
/// history kept on this computer.
pub fn compose(parts: Parts, taste: &History) -> Result<Page, String> {
    let mut home = parts.home?;

    // Made for you: every mix, pulled out of whichever shelf it was in.
    let mut mixes = Vec::new();
    for shelf in &mut home.shelves {
        let items = std::mem::take(&mut shelf.items);
        for item in items {
            if is_mix(&item) {
                add(&mut mixes, item);
            } else {
                shelf.items.push(item);
            }
        }
    }

    // Jump back in: what you opened here last, then YouTube Music's
    // "Listen again", which knows your other devices too.
    let mut jump = Vec::new();
    for item in taste.recent(false, SHELF) {
        add(&mut jump, item);
    }
    if let Some(at) = home
        .shelves
        .iter()
        .position(|s| s.title.eq_ignore_ascii_case("Listen again"))
    {
        for item in home.shelves.remove(at).items {
            add(&mut jump, item);
        }
    }
    jump.truncate(SHELF);
    home.shelves.retain(|s| !s.items.is_empty());

    // Recently played: the account's history, or this computer's.
    let mut played: Vec<Item> = parts
        .played
        .map(|page| {
            page.shelves
                .into_iter()
                .flat_map(|s| s.items)
                .filter(|i| i.video_id.is_some())
                .collect()
        })
        .unwrap_or_default();
    if played.is_empty() {
        played = taste.recent(true, SHELF);
    }
    let mut recent = Vec::new();
    for item in played {
        add(&mut recent, item);
    }
    recent.truncate(SHELF);

    // New releases, from the artists you follow or play most when any
    // of them has one. Otherwise just what is new.
    let followed = !parts.followed.is_empty();
    let yours: HashSet<String> = parts
        .followed
        .into_iter()
        .chain(taste.top_artists(60))
        .filter(|n| !n.is_empty())
        .collect();
    let new: Vec<Item> = parts
        .releases
        .map(|page| page.shelves.into_iter().flat_map(|s| s.items).collect())
        .unwrap_or_default();
    let (new_title, new) = {
        let mine: Vec<Item> = new
            .iter()
            .filter(|i| makers(i).iter().any(|m| yours.contains(m)))
            .cloned()
            .collect();
        if mine.is_empty() {
            ("New releases", new.into_iter().take(SHELF).collect())
        } else if followed {
            ("New from artists you follow", mine.into_iter().take(SHELF).collect())
        } else {
            ("New from artists you play", mine.into_iter().take(SHELF).collect())
        }
    };

    let ours = [
        shelf("Jump back in", jump),
        shelf("Made for you", mixes),
        shelf("Recently played", recent),
        shelf(new_title, new),
    ];
    let rest = std::mem::take(&mut home.shelves);
    home.shelves = ours.into_iter().flatten().chain(rest).collect();
    Ok(home)
}
