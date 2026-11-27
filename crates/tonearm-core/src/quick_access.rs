//! Quick access: albums, artists and playlists pinned to the side menu.
//!
//! They are kept as plain items in a JSON file in the data folder, oldest
//! first, so the list reads in the order it was built up. Nothing in it is
//! secret, unlike the sign in and the listening history.

use crate::innertube::model::{Header, Item, Kind, PageKind};
use serde_json::Value;
use std::path::PathBuf;

const FILE: &str = "quick-access.json";
/// What the file was called while Quick access was "favorites". Read once,
/// when the new file is not there yet, so nothing pinned is lost.
const OLD_FILE: &str = "favorites.json";

#[derive(Default)]
pub struct QuickAccess {
    items: Vec<Item>,
}

fn path() -> Option<PathBuf> {
    Some(crate::store::data_dir()?.join(FILE))
}

/// What names a pin: the page it opens.
fn key(item: &Item) -> Option<&str> {
    item.browse_id.as_deref()
}

/// Whether something can be pinned: an album, artist or playlist with a
/// page to open. Songs cannot, and neither can mixes with no page.
pub fn can_pin(item: &Item) -> bool {
    item.video_id.is_none()
        && key(item).is_some()
        && matches!(item.kind, Kind::Album | Kind::Artist | Kind::Playlist)
}

/// The pin an open page stands for, made from its hero and the id it was
/// opened with. None for pages that are not an album, artist or playlist.
pub fn from_page(header: &Header, id: &str, params: Option<&str>) -> Option<Item> {
    let kind = match header.kind {
        PageKind::Artist => Kind::Artist,
        PageKind::Album | PageKind::Single => Kind::Album,
        PageKind::Playlist => Kind::Playlist,
        _ => return None,
    };
    let item = Item {
        kind,
        title: header.title.clone(),
        subtitle: header.byline.clone(),
        thumb: header.thumb.clone(),
        browse_id: Some(id.to_string()),
        params: params.map(str::to_string),
        ..Item::default()
    };
    can_pin(&item).then_some(item)
}

/// Opens each pinned page again and takes its cover and title as they are
/// now. A pin keeps the cover link it was made with, and some links expire:
/// a playlist's own cover is a signed link that stops working after a few
/// days, leaving the tile blank. Blocks on the network, a page per pin, so
/// run it on a worker, once a start. Saves only when something changed,
/// into the list as it is by then, so a pin made or removed meanwhile
/// stays as it was. Returns whether anything changed.
pub fn refresh(client: &crate::innertube::Client) -> bool {
    let mut fresh: Vec<Item> = Vec::new();
    for pin in QuickAccess::load().items() {
        let Some(id) = key(pin) else { continue };
        let Ok(page) = client.browse(id, pin.params.as_deref()) else { continue };
        let Some(header) = page.header else { continue };
        let mut item = pin.clone();
        if !header.thumb.is_empty() {
            item.thumb = header.thumb;
        }
        if !header.title.is_empty() {
            item.title = header.title;
        }
        if item.thumb != pin.thumb || item.title != pin.title {
            fresh.push(item);
        }
    }
    if fresh.is_empty() {
        return false;
    }
    let mut list = QuickAccess::load();
    for item in fresh {
        if list.contains(&item) {
            list.put(item);
        }
    }
    QuickAccess::save(list.serialize());
    true
}

fn parse(text: &str) -> Vec<Item> {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(Item::from_json)
        .filter(can_pin)
        .collect()
}

impl QuickAccess {
    pub fn load() -> Self {
        let Some(path) = path() else {
            return QuickAccess::default();
        };
        if let Ok(text) = std::fs::read_to_string(&path) {
            return QuickAccess { items: parse(&text) };
        }
        // Pins saved under the old name move to the new one.
        let old = path.with_file_name(OLD_FILE);
        match std::fs::read_to_string(&old) {
            Ok(text) => {
                let list = QuickAccess { items: parse(&text) };
                QuickAccess::save(list.serialize());
                let _ = std::fs::remove_file(old);
                list
            }
            Err(_) => QuickAccess::default(),
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn contains(&self, item: &Item) -> bool {
        key(item).is_some_and(|k| self.items.iter().any(|i| key(i) == Some(k)))
    }

    /// Pins the item, or unpins it when it is pinned already. Returns true
    /// when it was pinned.
    pub fn toggle(&mut self, item: &Item) -> bool {
        let Some(k) = key(item) else { return false };
        if let Some(at) = self.items.iter().position(|i| key(i) == Some(k)) {
            self.items.remove(at);
            false
        } else {
            self.items.push(item.clone());
            true
        }
    }

    /// Adds the pin, or replaces the one for the same page.
    pub fn put(&mut self, item: Item) {
        match key(&item).and_then(|k| self.items.iter().position(|i| key(i) == Some(k))) {
            Some(at) => self.items[at] = item,
            None => self.items.push(item),
        }
    }

    /// Removes the pin for a page.
    pub fn remove(&mut self, id: &str) {
        self.items.retain(|i| key(i) != Some(id));
    }

    /// The list as text for `save`.
    pub fn serialize(&self) -> String {
        Value::Array(self.items.iter().map(Item::to_json).collect()).to_string()
    }

    /// Writes the file by way of a temporary one, so a crash mid-write
    /// leaves the old list rather than half of a new one.
    pub fn save(text: String) {
        let Some(path) = path() else { return };
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let temp = path.with_extension("tmp");
        if std::fs::write(&temp, text).is_ok() {
            let _ = std::fs::rename(&temp, &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, title: &str) -> Item {
        Item {
            kind: Kind::Album,
            title: title.into(),
            browse_id: Some(id.into()),
            ..Item::default()
        }
    }

    #[test]
    fn toggling_pins_then_unpins_and_keeps_order() {
        let mut pinned = QuickAccess::default();
        assert!(pinned.toggle(&album("a", "One")));
        assert!(pinned.toggle(&album("b", "Two")));
        assert!(pinned.contains(&album("a", "Renamed")));
        assert!(!pinned.toggle(&album("a", "One")));
        assert!(!pinned.contains(&album("a", "One")));
        assert_eq!(pinned.items().len(), 1);
        assert_eq!(pinned.items()[0].title, "Two");
    }

    #[test]
    fn songs_and_pageless_items_cannot_be_pinned() {
        let song = Item {
            kind: Kind::Song,
            video_id: Some("v".into()),
            browse_id: Some("b".into()),
            ..Item::default()
        };
        assert!(!can_pin(&song));
        assert!(!can_pin(&Item { kind: Kind::Album, ..Item::default() }));
        assert!(can_pin(&album("a", "One")));
    }

    #[test]
    fn survives_a_round_trip() {
        let mut pinned = QuickAccess::default();
        let liked = Item {
            kind: Kind::Playlist,
            title: "Liked Songs".into(),
            browse_id: Some("spotify:liked".into()),
            ..Item::default()
        };
        pinned.toggle(&liked);
        pinned.toggle(&album("a", "One"));
        let back = parse(&pinned.serialize());
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].browse_id.as_deref(), Some("spotify:liked"));
        assert_eq!(back[1].kind, Kind::Album);
    }
}
