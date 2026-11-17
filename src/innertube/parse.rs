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

/// The biggest picture on offer, for the page header.
fn thumb_best(v: &Value) -> String {
    let Some(list) = find(v, "thumbnails").and_then(Value::as_array) else {
        return String::new();
    };
    let width = |t: &Value| t.get("width").and_then(Value::as_u64).unwrap_or(0);
    list.iter()
        .max_by_key(|t| width(t))
        .and_then(|t| t.get("url")?.as_str())
        .unwrap_or("")
        .to_string()
}

fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The browse id a text run links to, with its page type when given.
fn run_link(run: &Value) -> Option<(String, String)> {
    let browse = run.get("navigationEndpoint")?.get("browseEndpoint")?;
    let id = string(browse, "browseId")?;
    let page_type = find(browse, "pageType")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((id, page_type))
}

fn is_artist_link(id: &str, page_type: &str) -> bool {
    matches!(
        page_type,
        "MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL"
    ) || (page_type.is_empty() && id.starts_with("UC"))
}

fn is_album_link(id: &str, page_type: &str) -> bool {
    matches!(
        page_type,
        "MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK"
    ) || (page_type.is_empty() && id.starts_with("MPREb_"))
}

/// Keeps the artist and album links from subtitle runs, so the names can
/// open their pages.
fn take_links(v: &Value, item: &mut Item) {
    let Some(runs) = v.get("runs").and_then(Value::as_array) else {
        return;
    };
    for run in runs {
        let Some((id, page_type)) = run_link(run) else {
            continue;
        };
        let name = run
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        if is_artist_link(&id, &page_type) {
            if !item.artists.iter().any(|(n, _)| *n == name) {
                item.artists.push((name, Some(id)));
            }
        } else if is_album_link(&id, &page_type) && item.album.is_none() {
            item.album = Some((name, id));
        }
    }
}

/// The texts of runs split on the " • " separators.
fn dot_parts(v: &Value) -> Vec<String> {
    text(v)
        .split('•')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
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
    if let Some(data) = r.get("playlistItemData") {
        if let Some(id) = string(data, "videoId") {
            item.video_id = Some(id);
            item.kind = Kind::Song;
        }
        item.set_video_id = string(data, "playlistSetVideoId");
    }
    let columns = r.get("flexColumns").and_then(Value::as_array);
    for column in columns.into_iter().flatten().skip(1) {
        if let Some(t) = column
            .get("musicResponsiveListItemFlexColumnRenderer")
            .and_then(|c| c.get("text"))
        {
            take_links(t, &mut item);
        }
    }
    if item.video_id.is_none() {
        item.library = library_toggle(r);
    }
    item
}

/// The library toggle in an album's or playlist's menu: the playlist it
/// saves, and whether it is saved already. Songs use another kind of
/// toggle, so they give None.
fn library_toggle(r: &Value) -> Option<(String, bool)> {
    let entries = r.get("menu")?.get("menuRenderer")?.get("items")?.as_array()?;
    entries.iter().find_map(|entry| {
        let toggle = entry.get("toggleMenuServiceItemRenderer")?;
        let like = find(toggle.get("defaultServiceEndpoint")?, "likeEndpoint")?;
        let id = like.get("target").and_then(|t| string(t, "playlistId"))?;
        // A saved one's first action takes it out.
        let saved = like.get("status").and_then(Value::as_str) == Some("INDIFFERENT")
            || toggle.get("defaultText").map(text).is_some_and(|t| t.contains("Remove"));
        Some((id, saved))
    })
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
    if let Some(subtitle) = r.get("subtitle") {
        take_links(subtitle, &mut item);
    }
    if item.video_id.is_none() {
        item.library = library_toggle(r);
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
    if let Some(byline) = byline {
        take_links(byline, &mut item);
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
    if let Some(subtitle) = r.get("subtitle") {
        take_links(subtitle, &mut item);
    }
    item
}

/// The item a header button starts, such as an artist's shuffle.
fn button_item(button: Option<&Value>) -> Option<Item> {
    let ep = find(button?, "navigationEndpoint")?;
    let mut item = Item::default();
    apply_endpoint(ep, &mut item);
    if let Some(w) = ep.get("watchEndpoint") {
        item.params = string(w, "params");
    }
    (item.video_id.is_some() || item.playlist_id.is_some()).then_some(item)
}

fn page_kind(label: &str) -> PageKind {
    match label {
        "Album" | "Audiobook" => PageKind::Album,
        "Single" | "EP" => PageKind::Single,
        "Playlist" | "Mix" => PageKind::Playlist,
        "Podcast" | "Show" => PageKind::Podcast,
        "Profile" => PageKind::Profile,
        _ => PageKind::Other,
    }
}

/// Builds the detail header from one of the header renderers. The generic
/// headers of Home and the like give None, and keep the plain title strip.
fn detail_header(key: &str, h: &Value) -> Option<Header> {
    let description = h.get("description");
    let about = description
        .and_then(|d| d.get("musicDescriptionShelfRenderer"))
        .and_then(|m| m.get("description"))
        .or(description)
        .map(text)
        .unwrap_or_default();
    // Wikipedia's credit line is long and says little.
    let about = about
        .split("\n\nFrom Wikipedia")
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let mut header = Header {
        title: h.get("title").map(text).unwrap_or_default(),
        about,
        thumb: h.get("thumbnail").map(thumb_best).unwrap_or_default(),
        ..Header::default()
    };
    match key {
        "musicImmersiveHeaderRenderer" => {
            header.kind = PageKind::Artist;
            header.label = "Artist".into();
            header.banner = !header.thumb.is_empty();
            let subscribe = h
                .get("subscriptionButton")
                .and_then(|b| b.get("subscribeButtonRenderer"));
            let mut meta = Vec::new();
            if let Some(s) = subscribe {
                let count = s
                    .get("longSubscriberCountText")
                    .map(text)
                    .filter(|t| !t.is_empty())
                    .or_else(|| {
                        s.get("subscriberCountText")
                            .map(text)
                            .filter(|t| !t.is_empty())
                            .map(|t| format!("{t} subscribers"))
                    });
                meta.extend(count);
                header.subscribed = Some(s.get("subscribed").and_then(Value::as_bool) == Some(true));
                header.channel_id = string(s, "channelId");
            }
            if let Some(monthly) = h.get("monthlyListenerCount").map(text) {
                if !monthly.is_empty() {
                    meta.push(monthly);
                }
            }
            header.meta = meta.join(" • ");
            header.shuffle = button_item(h.get("playButton"));
            header.radio = button_item(h.get("startRadioButton"));
        }
        "musicResponsiveHeaderRenderer" | "musicDetailHeaderRenderer" => {
            let subtitle = h.get("subtitle");
            let parts = subtitle.map(dot_parts).unwrap_or_default();
            header.label = parts.first().cloned().unwrap_or_default();
            header.kind = page_kind(&header.label);
            // The artist or owner: the strapline on albums, the avatar stack
            // on playlists, a run of the subtitle on the older header.
            let strapline = h.get("straplineTextOne");
            let runs = strapline
                .or(subtitle)
                .and_then(|s| s.get("runs"))
                .and_then(Value::as_array);
            let linked = runs
                .into_iter()
                .flatten()
                .find_map(|r| run_link(r).map(|(id, _)| (r, id)));
            if let Some(strapline) = strapline {
                header.byline = clean(text(strapline));
                header.byline_browse_id = linked.map(|(_, id)| id);
            } else if let Some((run, id)) = linked {
                header.byline = run.get("text").map(|t| clean(t.as_str().unwrap_or("").into())).unwrap_or_default();
                header.byline_browse_id = Some(id);
            } else if let Some(stack) = h.get("facepile").and_then(|f| f.get("avatarStackViewModel")) {
                header.byline = stack
                    .get("text")
                    .and_then(|t| string(t, "content"))
                    .unwrap_or_default();
                header.byline_browse_id = find(stack, "browseEndpoint").and_then(|b| string(b, "browseId"));
            }
            let year = parts
                .iter()
                .skip(1)
                .find(|p| p.len() == 4 && p.chars().all(|c| c.is_ascii_digit()));
            let mut meta: Vec<String> = year.cloned().into_iter().collect();
            meta.extend(h.get("secondSubtitle").map(dot_parts).unwrap_or_default());
            header.meta = meta.join(" • ");
            // The bookmark button saves the album or playlist to the library.
            let buttons = h.get("buttons").and_then(Value::as_array);
            for button in buttons.into_iter().flatten() {
                let Some(toggle) = button.get("toggleButtonRenderer") else {
                    continue;
                };
                let Some(like) = find(toggle, "likeEndpoint") else {
                    continue;
                };
                header.playlist_id = like.get("target").and_then(|t| string(t, "playlistId"));
                header.liked = Some(toggle.get("isToggled").and_then(Value::as_bool) == Some(true));
            }
        }
        _ => return None,
    }
    Some(header)
}

/// The browse endpoint behind a shelf's "Show all" or "More" button.
fn shelf_more(v: &Value) -> Option<(String, Option<String>)> {
    let endpoint = v
        .get("bottomEndpoint")
        .or_else(|| find(v.get("header")?, "moreContentButton"))?;
    let browse = find(endpoint, "browseEndpoint")?;
    Some((string(browse, "browseId")?, string(browse, "params")))
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
            more: shelf_more(v),
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

    fn header(&mut self, key: &str, h: &Value) {
        if self.has_header {
            return;
        }
        self.has_header = true;
        self.page.header = detail_header(key, h);
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
                        // The user's own playlist wraps the usual header.
                        "musicEditablePlaylistDetailHeaderRenderer" => {
                            self.walk(child);
                            if let Some(h) = self.page.header.as_mut() {
                                h.editable = true;
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
                                // The songs under the card read "Song • 2:38 •
                                // 35M plays", with no artist. They are the
                                // card's artist's, so lend them that one.
                                let artist = std::iter::once(&item)
                                    .chain(shelf.items.iter())
                                    .find(|i| i.kind == Kind::Artist && !i.title.is_empty())
                                    .map(|a| (a.title.clone(), a.browse_id.clone()))
                                    .or_else(|| item.artists.first().cloned());
                                if let Some(artist) = artist {
                                    for song in shelf.items.iter_mut() {
                                        if song.kind == Kind::Song && song.artists.is_empty() {
                                            song.artists.push(artist.clone());
                                        }
                                    }
                                }
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
                        k if HEADERS.contains(&k) => self.header(k, child),
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
    // An artist's top songs lead the page as "Popular", five of them, with
    // the rest behind "Show all".
    if page.header.as_ref().is_some_and(|h| h.kind == PageKind::Artist) {
        if let Some(songs) = page
            .shelves
            .iter_mut()
            .find(|s| !s.carousel && (s.title == "Songs" || s.title == "Top songs"))
        {
            songs.title = "Popular".into();
            if songs.more.is_some() {
                songs.items.truncate(5);
            }
        }
    }
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

/// Completed queries and items from a `get_search_suggestions` response.
/// Past searches come back as history suggestions and count as queries.
pub fn suggestions(v: &Value) -> (Vec<String>, Vec<Item>) {
    fn queries(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "searchSuggestionRenderer" || key == "historySuggestionRenderer" {
                        let query = find(child, "searchEndpoint")
                            .and_then(|e| string(e, "query"))
                            .or_else(|| child.get("suggestion").map(text))
                            .unwrap_or_default();
                        if !query.is_empty() && !out.contains(&query) {
                            out.push(query);
                        }
                    } else {
                        queries(child, out);
                    }
                }
            }
            Value::Array(list) => list.iter().for_each(|c| queries(c, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    queries(v, &mut found);
    let items = page(v).shelves.into_iter().flat_map(|s| s.items).collect();
    (found, items)
}

/// The signed in account: its name, picture and handle.
pub struct Account {
    pub name: String,
    /// A link to its picture. Empty when it has none.
    pub photo: String,
    /// Its @handle. Empty when it has none.
    pub handle: String,
}

pub fn account(v: &Value) -> Option<Account> {
    let header = find(v, "activeAccountHeaderRenderer")?;
    let name = text(header.get("accountName")?);
    // The biggest on offer, as the profile page shows it large.
    let photo = header.get("accountPhoto").map(thumb_best).unwrap_or_default();
    let handle = header.get("channelHandle").map(text).unwrap_or_default();
    Some(Account { name, photo, handle })
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

    fn artist_run(name: &str, id: &str) -> Value {
        json!({"text": name, "navigationEndpoint": {"browseEndpoint": {"browseId": id,
            "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig":
                {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}})
    }

    #[test]
    fn reads_an_album_header() {
        // The shape music.youtube.com sent for Random Access Memories.
        let v = json!({"contents": [{"musicResponsiveHeaderRenderer": {
            "title": {"runs": [{"text": "Random Access Memories"}]},
            "subtitle": {"runs": [{"text": "Album"}, {"text": " • "}, {"text": "2013"}]},
            "straplineTextOne": {"runs": [artist_run("Daft Punk", "UC1")]},
            "secondSubtitle": {"runs": [{"text": "13 songs"}, {"text": " • "}, {"text": "1 hour, 14 minutes"}]},
            "description": {"musicDescriptionShelfRenderer": {"description": {"runs": [
                {"text": "The fourth album.\n\nFrom Wikipedia (link)"}]}}},
            "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [
                {"url": "small", "width": 60}, {"url": "big", "width": 544}]}}},
            "buttons": [{"toggleButtonRenderer": {"isToggled": false,
                "defaultServiceEndpoint": {"likeEndpoint": {"status": "LIKE", "target": {"playlistId": "OLAK1"}}}}}]
        }}]});
        let header = page(&v).header.expect("a header");
        assert_eq!(header.kind, PageKind::Album);
        assert_eq!(header.label, "Album");
        assert_eq!(header.byline, "Daft Punk");
        assert_eq!(header.byline_browse_id.as_deref(), Some("UC1"));
        assert_eq!(header.meta, "2013 • 13 songs • 1 hour, 14 minutes");
        assert_eq!(header.about, "The fourth album.");
        assert_eq!(header.thumb, "big");
        assert_eq!(header.liked, Some(false));
        assert_eq!(header.playlist_id.as_deref(), Some("OLAK1"));
    }

    #[test]
    fn reads_an_artist_header() {
        let v = json!({"header": {"musicImmersiveHeaderRenderer": {
            "title": {"runs": [{"text": "Daft Punk"}]},
            "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [
                {"url": "banner", "width": 1080}]}}},
            "monthlyListenerCount": {"runs": [{"text": "65.9M monthly audience"}]},
            "subscriptionButton": {"subscribeButtonRenderer": {"subscribed": true, "channelId": "UC2",
                "longSubscriberCountText": {"runs": [{"text": "7.22M subscribers"}]}}},
            "playButton": {"buttonRenderer": {"navigationEndpoint": {"watchEndpoint":
                {"videoId": "v1", "playlistId": "RDAO1", "params": "shuffle"}}}}
        }}, "contents": {"musicShelfRenderer": {
            "title": {"runs": [{"text": "Top songs"}]},
            "bottomEndpoint": {"browseEndpoint": {"browseId": "VLOLAK2", "params": "p"}},
            "contents": (0..7).map(|i| json!({"musicResponsiveListItemRenderer": {
                "flexColumns": [{"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": format!("song {i}")}]}}}],
                "playlistItemData": {"videoId": format!("v{i}")}}})).collect::<Vec<_>>()
        }}});
        let page = page(&v);
        let header = page.header.expect("a header");
        assert_eq!(header.kind, PageKind::Artist);
        assert!(header.banner);
        assert_eq!(header.subscribed, Some(true));
        assert_eq!(header.channel_id.as_deref(), Some("UC2"));
        assert_eq!(header.meta, "7.22M subscribers • 65.9M monthly audience");
        let shuffle = header.shuffle.expect("a shuffle button");
        assert_eq!(shuffle.params.as_deref(), Some("shuffle"));
        assert_eq!(page.shelves[0].title, "Popular");
        assert_eq!(page.shelves[0].items.len(), 5);
        assert_eq!(
            page.shelves[0].more,
            Some(("VLOLAK2".to_string(), Some("p".to_string())))
        );
    }

    #[test]
    fn keeps_artist_and_album_links_of_a_playlist_row() {
        let v = json!({"musicPlaylistShelfRenderer": {"contents": [{"musicResponsiveListItemRenderer": {
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Get Lucky"}]}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    artist_run("Daft Punk", "UC1"), {"text": " & "}, artist_run("Pharrell Williams", "UC3")]}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Random Access Memories",
                    "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_1",
                        "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig":
                            {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}}]}}}
            ],
            "playlistItemData": {"videoId": "v1"}
        }}]}});
        let song = &page(&v).shelves[0].items[0];
        assert_eq!(song.subtitle, "Daft Punk & Pharrell Williams • Random Access Memories");
        assert_eq!(
            song.artists,
            vec![
                ("Daft Punk".to_string(), Some("UC1".to_string())),
                ("Pharrell Williams".to_string(), Some("UC3".to_string()))
            ]
        );
        assert_eq!(
            song.album,
            Some(("Random Access Memories".to_string(), "MPREb_1".to_string()))
        );
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
