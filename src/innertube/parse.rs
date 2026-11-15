//! Turns InnerTube JSON into `Page`, `Shelf` and `Item`.
//!
//! YouTube Music nests the same few renderers in many different wrappers,
//! and the wrappers change often. We walk the whole tree and pick out the
//! renderers we know, so a new wrapper does not break parsing.

use super::model::*;
use serde_json::Value;

const HEADERS: [&str; 5] = [
    "musicResponsiveHeaderRenderer",
    "musicImmersiveHeaderRenderer",
    "musicDetailHeaderRenderer",
    "musicVisualHeaderRenderer",
    "musicHeaderRenderer",
];

pub fn text(v: &Value) -> String {
    if let Some(runs) = v.get("runs").and_then(Value::as_array) {
        runs.iter()
            .filter_map(|r| r.get("text")?.as_str())
            .collect()
    } else {
        v.get("simpleText")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }
}

fn clean(s: String) -> String {
    s.trim()
        .trim_end_matches('•')
        .trim_start_matches('•')
        .trim()
        .to_string()
}

/// Depth first search for the first value stored under `key`.
pub fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => {
            if let Some(hit) = map.get(key) {
                return Some(hit);
            }
            map.iter()
                .filter(|(k, _)| k.as_str() != "menu")
                .find_map(|(_, c)| find(c, key))
        }
        Value::Array(list) => list.iter().find_map(|c| find(c, key)),
        _ => None,
    }
}

fn thumb(v: &Value) -> String {
    let Some(list) = find(v, "thumbnails").and_then(Value::as_array) else {
        return String::new();
    };
    let width = |t: &Value| t.get("width").and_then(Value::as_u64).unwrap_or(0);
    list.iter()
        .find(|t| width(t) >= 120)
        .or(list.last())
        .and_then(|t| t.get("url")?.as_str())
        .unwrap_or("")
        .to_string()
}

fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn apply_endpoint(ep: &Value, item: &mut Item) {
    if let Some(w) = ep.get("watchEndpoint") {
        item.video_id = string(w, "videoId");
        item.playlist_id = string(w, "playlistId");
        item.kind = Kind::Song;
    } else if let Some(w) = ep.get("watchPlaylistEndpoint") {
        item.playlist_id = string(w, "playlistId");
        item.params = string(w, "params");
        item.kind = Kind::Playlist;
    } else if let Some(b) = ep.get("browseEndpoint") {
        item.browse_id = string(b, "browseId");
        item.params = string(b, "params");
        let page_type = find(b, "pageType").and_then(Value::as_str).unwrap_or("");
        item.kind = match page_type {
            "MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK" => Kind::Album,
            "MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL" => Kind::Artist,
            "MUSIC_PAGE_TYPE_PLAYLIST" => Kind::Playlist,
            _ => Kind::Other,
        };
    }
}

fn list_item(r: &Value) -> Item {
    let columns: Vec<String> = r
        .get("flexColumns")
        .and_then(Value::as_array)
        .map(|cols| {
            cols.iter()
                .filter_map(|c| {
                    c.get("musicResponsiveListItemFlexColumnRenderer")?
                        .get("text")
                })
                .map(|t| clean(text(t)))
                .collect()
        })
        .unwrap_or_default();
    let mut item = Item {
        title: columns.first().cloned().unwrap_or_default(),
        subtitle: columns
            .iter()
            .skip(1)
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" • "),
        thumb: r.get("thumbnail").map(thumb).unwrap_or_default(),
        ..Item::default()
    };
    if let Some(fixed) = r.get("fixedColumns").and_then(|f| find(f, "text")) {
        item.duration = text(fixed);
    }
    let title_endpoint = r
        .get("flexColumns")
        .and_then(|c| c.get(0))
        .and_then(|c| find(c, "navigationEndpoint"));
    if let Some(ep) = r.get("navigationEndpoint").or(title_endpoint) {
        apply_endpoint(ep, &mut item);
    }
    if let Some(id) = r.get("playlistItemData").and_then(|d| string(d, "videoId")) {
        item.video_id = Some(id);
        item.kind = Kind::Song;
    }
    item
}

fn two_row_item(r: &Value) -> Item {
    let mut item = Item {
        title: r.get("title").map(text).unwrap_or_default(),
        subtitle: clean(r.get("subtitle").map(text).unwrap_or_default()),
        thumb: r.get("thumbnailRenderer").map(thumb).unwrap_or_default(),
        ..Item::default()
    };
    if let Some(ep) = r.get("navigationEndpoint") {
        apply_endpoint(ep, &mut item);
    }
    item
}

fn panel_item(r: &Value) -> Item {
    let byline = r.get("longBylineText").or(r.get("shortBylineText"));
    let mut item = Item {
        kind: Kind::Song,
        title: r.get("title").map(text).unwrap_or_default(),
        subtitle: clean(byline.map(text).unwrap_or_default()),
        thumb: r.get("thumbnail").map(thumb).unwrap_or_default(),
        duration: r.get("lengthText").map(text).unwrap_or_default(),
        ..Item::default()
    };
    if let Some(ep) = r.get("navigationEndpoint") {
        apply_endpoint(ep, &mut item);
    }
    if let Some(id) = string(r, "videoId") {
        item.video_id = Some(id);
    }
    item
}

fn card_item(r: &Value) -> Item {
    let mut item = Item {
        title: r.get("title").map(text).unwrap_or_default(),
        subtitle: clean(r.get("subtitle").map(text).unwrap_or_default()),
        thumb: r.get("thumbnail").map(thumb).unwrap_or_default(),
        ..Item::default()
    };
    let endpoint = r
        .get("onTap")
        .or_else(|| r.get("title").and_then(|t| find(t, "navigationEndpoint")));
    if let Some(ep) = endpoint {
        apply_endpoint(ep, &mut item);
    }
    item
}

#[derive(Default)]
struct Walker {
    page: Page,
    current: Option<usize>,
    loose: Option<usize>,
    has_header: bool,
}

impl Walker {
    fn push(&mut self, item: Item) {
        if item.title.is_empty() {
            return;
        }
        let index = match (self.current, self.loose) {
            (Some(i), _) => i,
            (None, Some(i)) => i,
            (None, None) => {
                self.page.shelves.push(Shelf::default());
                self.loose = Some(self.page.shelves.len() - 1);
                self.page.shelves.len() - 1
            }
        };
        self.page.shelves[index].items.push(item);
    }

    fn shelf(&mut self, v: &Value, carousel: bool) {
        let title = v
            .get("title")
            .or_else(|| v.get("header").and_then(|h| find(h, "title")))
            .map(text)
            .unwrap_or_default();
        self.page.shelves.push(Shelf {
            title,
            carousel,
            items: Vec::new(),
        });
        let outer = self.current.replace(self.page.shelves.len() - 1);
        self.loose = None;
        for key in ["contents", "items"] {
            if let Some(children) = v.get(key) {
                self.walk(children);
            }
        }
        if self.page.continuation.is_none() {
            if let Some(c) = v.get("continuations") {
                self.walk(c);
            }
        }
        self.current = outer;
    }

    fn header(&mut self, h: &Value) {
        if self.has_header {
            return;
        }
        self.has_header = true;
        self.page.title = h.get("title").map(text).unwrap_or_default();
        let lines = ["straplineTextOne", "subtitle", "secondSubtitle"];
        let parts: Vec<String> = lines
            .iter()
            .filter_map(|k| h.get(*k))
            .map(|t| clean(text(t)))
            .filter(|s| !s.is_empty())
            .collect();
        self.page.subtitle = parts.join(" • ");
        self.page.thumb = h.get("thumbnail").map(thumb).unwrap_or_default();
    }

    fn walk(&mut self, v: &Value) {
        match v {
            Value::Array(list) => list.iter().for_each(|c| self.walk(c)),
            Value::Object(map) => {
                for (key, child) in map {
                    match key.as_str() {
                        "musicResponsiveListItemRenderer" => self.push(list_item(child)),
                        "musicTwoRowItemRenderer" => self.push(two_row_item(child)),
                        "playlistPanelVideoRenderer" => self.push(panel_item(child)),
                        "playlistPanelVideoWrapperRenderer" => {
                            if let Some(primary) = child.get("primaryRenderer") {
                                self.walk(primary);
                            }
                        }
                        "musicCarouselShelfRenderer" => self.shelf(child, true),
                        "gridRenderer" => self.shelf(child, true),
                        "musicShelfRenderer"
                        | "musicPlaylistShelfRenderer"
                        | "playlistPanelRenderer" => self.shelf(child, false),
                        "musicCardShelfRenderer" => {
                            let item = card_item(child);
                            self.shelf(child, false);
                            if let Some(shelf) = self.page.shelves.last_mut() {
                                shelf.title = "Top result".into();
                                if !item.title.is_empty() {
                                    shelf.items.insert(0, item);
                                }
                            }
                        }
                        "nextContinuationData" => {
                            if self.page.continuation.is_none() {
                                self.page.continuation = string(child, "continuation");
                            }
                        }
                        "continuationItemRenderer" => {
                            if self.page.continuation.is_none() {
                                self.page.continuation = find(child, "continuationCommand")
                                    .and_then(|c| string(c, "token"));
                            }
                        }
                        "menu" | "frameworkUpdates" | "responseContext" => {}
                        k if HEADERS.contains(&k) => self.header(child),
                        _ => self.walk(child),
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn page(v: &Value) -> Page {
    let mut walker = Walker::default();
    walker.walk(v);
    let mut page = walker.page;
    page.shelves.retain(|s| !s.items.is_empty());
    // Album tracks carry no artwork of their own. Use the album cover.
    if !page.thumb.is_empty() {
        for item in page.shelves.iter_mut().flat_map(|s| s.items.iter_mut()) {
            if item.thumb.is_empty() {
                item.thumb = page.thumb.clone();
            }
        }
    }
    page
}

/// Browse id of the lyrics tab inside a `next` response.
pub fn lyrics_browse_id(v: &Value) -> Option<String> {
    let tabs = find(v, "watchNextTabbedResultsRenderer")?
        .get("tabs")?
        .as_array()?;
    tabs.iter().find_map(|tab| {
        let endpoint = tab
            .get("tabRenderer")?
            .get("endpoint")?
            .get("browseEndpoint")?;
        let page_type = find(endpoint, "pageType")?.as_str()?;
        (page_type == "MUSIC_PAGE_TYPE_TRACK_LYRICS")
            .then(|| string(endpoint, "browseId"))
            .flatten()
    })
}

pub fn lyrics(v: &Value) -> Option<String> {
    let shelf = find(v, "musicDescriptionShelfRenderer")?;
    let body = text(shelf.get("description")?);
    let footer = shelf.get("footer").map(text).unwrap_or_default();
    Some(if footer.is_empty() {
        body
    } else {
        format!("{body}\n\n{footer}")
    })
}

pub fn account_name(v: &Value) -> Option<String> {
    let header = find(v, "activeAccountHeaderRenderer")?;
    Some(text(header.get("accountName")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_shelf_with_a_song() {
        let v = json!({"contents": {"musicShelfRenderer": {
            "title": {"runs": [{"text": "Songs"}]},
            "contents": [{"musicResponsiveListItemRenderer": {
                "flexColumns": [
                    {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "One More Time"}]}}},
                    {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Daft Punk"}, {"text": " • "}]}}}
                ],
                "fixedColumns": [{"musicResponsiveListItemFixedColumnRenderer": {"text": {"runs": [{"text": "5:21"}]}}}],
                "playlistItemData": {"videoId": "abc"}
            }}],
            "continuations": [{"nextContinuationData": {"continuation": "tok"}}]
        }}});
        let page = page(&v);
        assert_eq!(page.shelves.len(), 1);
        assert_eq!(page.shelves[0].title, "Songs");
        let song = &page.shelves[0].items[0];
        assert_eq!(song.kind, Kind::Song);
        assert_eq!(song.title, "One More Time");
        assert_eq!(song.subtitle, "Daft Punk");
        assert_eq!(song.video_id.as_deref(), Some("abc"));
        assert_eq!(song.duration_secs(), 321.0);
        assert_eq!(page.continuation.as_deref(), Some("tok"));
    }

    #[test]
    fn loose_items_do_not_join_the_previous_shelf() {
        let row = |title: &str| {
            json!({"musicResponsiveListItemRenderer": {"flexColumns": [
            {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": title}]}}}]}})
        };
        let v = json!([
            {"musicShelfRenderer": {"title": {"runs": [{"text": "A"}]}, "contents": [row("one")]}},
            {"itemSectionRenderer": {"contents": [row("two")]}},
            {"itemSectionRenderer": {"contents": [row("three")]}}
        ]);
        let page = page(&v);
        assert_eq!(page.shelves.len(), 2);
        assert_eq!(page.shelves[1].items.len(), 2);
    }
}
