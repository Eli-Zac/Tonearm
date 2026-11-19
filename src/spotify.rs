//! Your Spotify library in Tonearm: your playlists and Liked Songs.
//!
//! They are read with the web player token that Canvas already gets from
//! the `sp_dc` cookie (see `canvas.rs`), so they are whatever your Spotify
//! account sees, private playlists and Spotify's own mixes included. The
//! public Web API answers that token with 429 at once, so we ask what the
//! web player itself asks: its GraphQL endpoint, "pathfinder", where each
//! query is named and known by a hash. When Spotify changes a query its
//! hash stops working, the library says so, and the hashes below need
//! updating from the web player. (BitChord, another YouTube Music client,
//! showed the way.)
//!
//! Tonearm plays from YouTube Music, so each song is then matched to its
//! YouTube Music song (see `find`), and the matches are kept on disk so a
//! song is only looked up once.

use crate::innertube::model::{Header, Item, Kind, Page, PageKind, Shelf};
use crate::innertube::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PATHFINDER: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
/// The web player's queries: your library's playlists, a playlist's
/// songs, and Liked Songs.
const LIBRARY: (&str, &str) = ("libraryV3", "973e511ca44261fda7eebac8b653155e7caee3675abb4fb110cc1b8c78b091c3");
const PLAYLIST: (&str, &str) = ("fetchPlaylist", "346811f856fb0b7e4f6c59f8ebea78dd081c6e2fb01b77c954b26259d5fc6763");
const LIKED_SONGS: (&str, &str) = ("fetchLibraryTracks", "087278b20b743578a6262c2b0b4bcd20d879c503cc359a2285baf083ef944240");
/// Spotify's own Liked Songs picture, the purple heart.
pub const LIKED_COVER: &str = "https://misc.scdn.co/liked-songs/liked-songs-640.png";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
/// The browse ids Spotify pages open with, so they go through the same
/// routes, history and play buttons as YouTube Music pages.
pub const PREFIX: &str = "spotify:";
pub const LIKED: &str = "spotify:liked";

/// A playlist in your library.
#[derive(Clone, Debug, Default)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub thumb: String,
    pub count: u64,
    pub about: String,
    /// Whether this account may add songs to it and take them out: its
    /// own playlists and ones it collaborates on, not Spotify's mixes.
    pub editable: bool,
}

/// A song in a playlist or in Liked Songs.
#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    pub thumb: String,
    pub duration_ms: u64,
    pub explicit: bool,
    /// Its place in a playlist, which taking it out needs. Empty in Liked
    /// Songs and search results.
    pub uid: String,
}

impl Track {
    pub fn duration_secs(&self) -> f32 {
        self.duration_ms as f32 / 1000.0
    }
}

// --------------------------------------------------------------------- api

/// One pathfinder query. Spotify sometimes asks for a short wait first.
fn query(http: &ureq::Agent, token: &str, (name, hash): (&str, &str), variables: Value) -> Result<Value, String> {
    let body = json!({
        "variables": variables,
        "operationName": name,
        "extensions": { "persistedQuery": { "version": 1, "sha256Hash": hash } },
    })
    .to_string();
    let call = || {
        http.post(PATHFINDER)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/json")
            .set("Content-Type", "application/json;charset=UTF-8")
            .set("App-Platform", "WebPlayer")
            .set("Origin", "https://open.spotify.com")
            .set("Referer", "https://open.spotify.com/")
            .set("User-Agent", USER_AGENT)
            .send_string(&body)
    };
    let mut response = call();
    for _ in 0..3 {
        let wait = match &response {
            Err(ureq::Error::Status(429, r)) => r
                .header("Retry-After")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(2),
            _ => break,
        };
        if wait > 10 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(wait.max(1)));
        response = call();
    }
    let root: Value = match response {
        Ok(r) => r.into_json().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(401, _)) => {
            crate::canvas::forget_token();
            return Err("Spotify signed Tonearm out. Copy a fresh sp_dc cookie in Settings.".into());
        }
        Err(ureq::Error::Status(429, _)) => return Err("Spotify is busy. Try again in a minute.".into()),
        Err(ureq::Error::Status(code, _)) => return Err(format!("Spotify's {name} answered {code}")),
        Err(e) => return Err(format!("Could not reach Spotify: {e}")),
    };
    if let Some(message) = root.pointer("/errors/0/message").and_then(Value::as_str) {
        // A changed query comes back as an unknown hash.
        return Err(format!("Spotify's {name} failed: {message}"));
    }
    Ok(root)
}

/// Pages through a query, `limit` at a time. `list` points at the paged
/// object in a response, which has `items` and `totalCount`. `first` gets
/// the first whole response, even when it has no items, and `each` gets
/// the whole response and one item.
#[allow(clippy::too_many_arguments)]
fn paged(
    http: &ureq::Agent,
    token: &str,
    op: (&str, &str),
    limit: u64,
    vars: Value,
    list: &str,
    mut first: impl FnMut(&Value),
    mut each: impl FnMut(&Value, &Value),
) -> Result<(), String> {
    let mut offset = 0;
    // A guard against a list that never ends: 10,000 songs at 100 a page.
    for page in 0..100 {
        let mut v = vars.clone();
        v["offset"] = json!(offset);
        v["limit"] = json!(limit);
        let root = query(http, token, op, v)?;
        if page == 0 {
            first(&root);
        }
        let paged = root
            .pointer(list)
            .ok_or_else(|| format!("Spotify's {} sent nothing", op.0))?;
        let items = paged.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
        for item in &items {
            each(&root, item);
        }
        offset += items.len() as u64;
        let total = paged.get("totalCount").and_then(Value::as_u64).unwrap_or(0);
        if items.is_empty() || offset >= total {
            break;
        }
    }
    Ok(())
}

fn text(v: &Value, pointer: &str) -> String {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// The source nearest `want` pixels wide from an image's sources, which
/// Spotify lists in no fixed order.
fn source(sources: Option<&Value>, want: u64) -> String {
    let list = sources.and_then(Value::as_array).cloned().unwrap_or_default();
    let width = |v: &Value| v.get("width").and_then(Value::as_u64).unwrap_or(want);
    list.iter()
        .min_by_key(|v| width(v).abs_diff(want))
        .and_then(|v| v.get("url").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// Descriptions come as HTML, with links and entities in them.
fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// A song's `data` object, from a playlist or Liked Songs. Podcast
/// episodes and local files have no track uri and are left out: neither
/// can be found on YouTube Music.
fn track_of(data: &Value, uri: &str) -> Option<Track> {
    let id = uri.strip_prefix("spotify:track:")?.to_string();
    let title = text(data, "/name");
    if title.is_empty() {
        return None;
    }
    let artists = data
        .pointer("/artists/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|a| text(a, "/profile/name"))
        .filter(|n| !n.is_empty())
        .collect();
    let duration = data
        .pointer("/trackDuration/totalMilliseconds")
        .or_else(|| data.pointer("/duration/totalMilliseconds"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(Track {
        id,
        title,
        artists,
        album: text(data, "/albumOfTrack/name"),
        thumb: source(data.pointer("/albumOfTrack/coverArt/sources"), 300),
        duration_ms: duration,
        explicit: text(data, "/contentRating/label") == "EXPLICIT",
        uid: String::new(),
    })
}

/// Your playlists, the ones you made and the ones you follow, in your
/// library's order. Liked Songs is not among them: see `liked`.
pub fn playlists(http: &ureq::Agent, cookie: &str) -> Result<Vec<Playlist>, String> {
    let token = crate::canvas::web_token(http, cookie)?;
    let vars = json!({
        "filters": ["Playlists"],
        "order": null,
        "textFilter": "",
        "features": ["LIKED_SONGS", "YOUR_EPISODES_V2", "PRERELEASES", "EVENTS"],
        "flatten": true,
        "expandedFolders": [],
        "folderUri": null,
        "includeFoldersWhenFlattening": false,
    });
    let mut lists = Vec::new();
    paged(http, &token, LIBRARY, 50, vars, "/data/me/libraryV3", |_| {}, |_, entry| {
        let item = &entry["item"];
        let data = &item["data"];
        if data.get("__typename").and_then(Value::as_str) != Some("Playlist") {
            return;
        }
        let uri = text(item, "/_uri");
        let Some(id) = uri.strip_prefix("spotify:playlist:") else { return };
        let name = text(data, "/name");
        if name.is_empty() {
            return;
        }
        lists.push(Playlist {
            id: id.to_string(),
            name,
            owner: text(data, "/ownerV2/data/name"),
            thumb: source(data.pointer("/images/items/0/sources"), 300),
            count: 0,
            about: html_text(&text(data, "/description")),
            // Only a playlist's own page says; see `playlist`.
            editable: false,
        });
    })?;
    Ok(lists)
}

/// Your Liked Songs, newest first.
pub fn liked(http: &ureq::Agent, cookie: &str) -> Result<Vec<Track>, String> {
    let token = crate::canvas::web_token(http, cookie)?;
    let mut tracks = Vec::new();
    paged(http, &token, LIKED_SONGS, 100, json!({}), "/data/me/library/tracks", |_| {}, |_, entry| {
        let wrapper = &entry["track"];
        let uri = match wrapper.get("_uri").and_then(Value::as_str) {
            Some(uri) => uri.to_string(),
            None => text(wrapper, "/data/uri"),
        };
        tracks.extend(track_of(&wrapper["data"], &uri));
    })?;
    Ok(tracks)
}

/// A playlist and its songs.
pub fn playlist(http: &ureq::Agent, cookie: &str, id: &str) -> Result<(Playlist, Vec<Track>), String> {
    let token = crate::canvas::web_token(http, cookie)?;
    let vars = json!({ "uri": format!("spotify:playlist:{id}"), "enableWatchFeedEntrypoint": false });
    let mut head: Option<Playlist> = None;
    let mut tracks = Vec::new();
    // From the first page, songs or not: an empty playlist still has its
    // name, and may still be yours to fill.
    let first = |root: &Value| {
        let p = &root["data"]["playlistV2"];
        head = Some(Playlist {
            id: id.to_string(),
            name: text(p, "/name"),
            owner: text(p, "/ownerV2/data/name"),
            thumb: source(p.pointer("/images/items/0/sources"), 640),
            count: p.pointer("/content/totalCount").and_then(Value::as_u64).unwrap_or(0),
            about: html_text(&text(p, "/description")),
            editable: p
                .pointer("/currentUserCapabilities/canEditItems")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    };
    paged(http, &token, PLAYLIST, 100, vars, "/data/playlistV2/content", first, |_, entry| {
        let item = &entry["itemV2"];
        let data = &item["data"];
        let uri = match data.get("uri").and_then(Value::as_str) {
            Some(uri) => uri.to_string(),
            None => text(item, "/_uri"),
        };
        if let Some(mut track) = track_of(data, &uri) {
            track.uid = text(entry, "/uid");
            tracks.push(track);
        }
    })?;
    let head = head.unwrap_or(Playlist { id: id.to_string(), ..Playlist::default() });
    Ok((head, tracks))
}

/// A cheap fingerprint of a playlist or of Liked Songs, which changes
/// whenever a song is added or taken out: a playlist's revision, or Liked
/// Songs' count and newest song.
pub fn fingerprint(http: &ureq::Agent, cookie: &str, id: &str) -> Result<String, String> {
    let token = crate::canvas::web_token(http, cookie)?;
    if id == LIKED {
        let root = query(http, &token, LIKED_SONGS, json!({ "offset": 0, "limit": 1 }))?;
        let list = root.pointer("/data/me/library/tracks").ok_or("Spotify sent nothing")?;
        let count = list.get("totalCount").and_then(Value::as_u64).unwrap_or(0);
        return Ok(format!("{count}|{}", text(list, "/items/0/track/_uri")));
    }
    let playlist_id = id
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_prefix("playlist:"))
        .ok_or("Not a Spotify playlist")?;
    let vars = json!({
        "uri": format!("spotify:playlist:{playlist_id}"),
        "offset": 0,
        "limit": 1,
        "enableWatchFeedEntrypoint": false,
    });
    let root = query(http, &token, PLAYLIST, vars)?;
    let p = &root["data"]["playlistV2"];
    let count = p.pointer("/content/totalCount").and_then(Value::as_u64).unwrap_or(0);
    Ok(format!("{}|{count}", text(p, "/revisionId")))
}

/// Your playlists, kept for a few minutes so going back to Library does
/// not ask Spotify again.
static LISTS: Mutex<Option<(Instant, Vec<Playlist>)>> = Mutex::new(None);
const LISTS_FRESH: Duration = Duration::from_secs(5 * 60);

pub fn playlists_cached(http: &ureq::Agent, cookie: &str) -> Result<Vec<Playlist>, String> {
    if let Some((at, lists)) = LISTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if at.elapsed() < LISTS_FRESH {
            return Ok(lists.clone());
        }
    }
    let lists = playlists(http, cookie)?;
    *LISTS.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), lists.clone()));
    Ok(lists)
}

// ------------------------------------------------------------------- edits
//
// What two-way sync changes on Spotify, done as the web player does it:
// its own mutations through pathfinder, and its playlist service for a new
// playlist. None of this is a published API, so a change on Spotify's side
// shows up as an error here rather than as a wrong edit.

const ADD_TO_PLAYLIST: (&str, &str) = ("addToPlaylist", "b907ad6b088d5bf7680fa81198996d58398c2d55dfcdf59fcc959e0be98a418a");
const REMOVE_FROM_PLAYLIST: (&str, &str) =
    ("removeFromPlaylist", "b907ad6b088d5bf7680fa81198996d58398c2d55dfcdf59fcc959e0be98a418a");
const ADD_TO_LIBRARY: (&str, &str) = ("addToLibrary", "896ebcb47815681340860d121cb5d494e157e2a78d3950385cd54e0393c67148");
const REMOVE_FROM_LIBRARY: (&str, &str) =
    ("removeFromLibrary", "896ebcb47815681340860d121cb5d494e157e2a78d3950385cd54e0393c67148");
const PROFILE: (&str, &str) = ("profileAttributes", "08ffb4730af3746e04a8301396f20875dbbce10c75243803091a9274eacc8ac0");
const SEARCH_TRACKS: (&str, &str) = ("searchTracks", "b02683192a98dde7966b5e6655a79eeb62713eab703eda9902c932818dd52751");
const PLAYLIST_SERVICE: &str = "https://spclient.wg.spotify.com/playlist/v2";

/// A mutation's answer can carry its own error without a failing status.
fn mutate(http: &ureq::Agent, token: &str, op: (&str, &str), variables: Value) -> Result<(), String> {
    let root = query(http, token, op, variables)?;
    let failed = root
        .pointer("/data")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|data| data.values())
        .find_map(|v| {
            let kind = v.get("__typename").and_then(Value::as_str)?;
            kind.ends_with("Error").then(|| text(v, "/message"))
        });
    match failed {
        Some(message) => Err(format!("Spotify's {} failed: {message}", op.0)),
        None => Ok(()),
    }
}

fn track_uris(ids: &[String]) -> Vec<String> {
    ids.iter().map(|id| format!("spotify:track:{id}")).collect()
}

/// Adds songs, by track id, to the end of a playlist, in order.
pub fn add_tracks(http: &ureq::Agent, cookie: &str, playlist_id: &str, ids: &[String]) -> Result<(), String> {
    let token = crate::canvas::web_token(http, cookie)?;
    for batch in ids.chunks(100) {
        mutate(
            http,
            &token,
            ADD_TO_PLAYLIST,
            json!({
                "playlistItemUris": track_uris(batch),
                "playlistUri": format!("spotify:playlist:{playlist_id}"),
                "newPosition": { "moveType": "BOTTOM_OF_PLAYLIST", "fromUid": null },
                "interactionId": null,
            }),
        )?;
    }
    Ok(())
}

/// Takes songs out of a playlist, by their places in it (`Track::uid`).
pub fn remove_tracks(http: &ureq::Agent, cookie: &str, playlist_id: &str, uids: &[String]) -> Result<(), String> {
    let token = crate::canvas::web_token(http, cookie)?;
    for batch in uids.chunks(100) {
        mutate(
            http,
            &token,
            REMOVE_FROM_PLAYLIST,
            json!({ "playlistUri": format!("spotify:playlist:{playlist_id}"), "uids": batch }),
        )?;
    }
    Ok(())
}

/// Likes songs, or takes them out of Liked Songs.
pub fn like_tracks(http: &ureq::Agent, cookie: &str, ids: &[String], like: bool) -> Result<(), String> {
    let token = crate::canvas::web_token(http, cookie)?;
    for batch in ids.chunks(50) {
        let (op, vars) = if like {
            (ADD_TO_LIBRARY, json!({ "libraryItemUris": track_uris(batch), "interactionId": null }))
        } else {
            (REMOVE_FROM_LIBRARY, json!({ "libraryItemUris": track_uris(batch) }))
        };
        mutate(http, &token, op, vars)?;
    }
    Ok(())
}

/// The signed in account's username, which a new playlist is filed under.
fn username(http: &ureq::Agent, token: &str) -> Result<String, String> {
    let root = query(http, token, PROFILE, json!({}))?;
    let name = text(&root, "/data/me/profile/username");
    if name.is_empty() {
        return Err("Spotify did not say whose account this is".into());
    }
    Ok(name)
}

/// One change to the playlist service, in the web player's own form:
/// protobuf as JSON, which turns away any field it does not know, so the
/// bodies carry only what the web player's own encoder writes. `step`
/// names the change in an error.
fn playlist_service(http: &ureq::Agent, token: &str, step: &str, path: &str, body: Value) -> Result<Value, String> {
    let response = http
        .post(&format!("{PLAYLIST_SERVICE}{path}"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .set("Content-Type", "application/json;charset=UTF-8")
        .set("App-Platform", "WebPlayer")
        .set("Origin", "https://open.spotify.com")
        .set("Referer", "https://open.spotify.com/")
        .set("User-Agent", USER_AGENT)
        .send_string(&body.to_string());
    match response {
        Ok(r) => Ok(r.into_json().unwrap_or(Value::Null)),
        Err(ureq::Error::Status(401, _)) => {
            crate::canvas::forget_token();
            Err("Spotify signed Tonearm out. Copy a fresh sp_dc cookie in Settings.".into())
        }
        Err(ureq::Error::Status(code, r)) => {
            // Its reason, when it gives one, says what it wanted instead.
            let why: String = r.into_string().unwrap_or_default().chars().take(160).collect();
            eprintln!("spotify {step}: {code} {why}");
            Err(format!("Spotify would not {step} ({code})"))
        }
        Err(e) => Err(format!("Could not reach Spotify: {e}")),
    }
}

/// Makes a new, empty playlist in your library, at the top as Spotify
/// puts new ones, and gives its id.
pub fn create_playlist(http: &ureq::Agent, cookie: &str, name: &str) -> Result<String, String> {
    let token = crate::canvas::web_token(http, cookie)?;
    // A new playlist is one change to an empty one: its name. Only the
    // list of changes goes, with no "info" beside it, unlike the library
    // change below.
    let made = playlist_service(
        http,
        &token,
        "make the playlist",
        "/playlist",
        json!({
            "ops": [{
                "kind": "UPDATE_LIST_ATTRIBUTES",
                "updateListAttributes": { "newAttributes": { "values": { "name": name } } }
            }],
        }),
    )?;
    let uri = made.get("uri").and_then(Value::as_str).unwrap_or_default().to_string();
    let id = uri
        .strip_prefix("spotify:playlist:")
        .ok_or("Spotify did not make the playlist")?
        .to_string();
    // Made, but only in the library once it is in your list of playlists.
    let user = username(http, &token)?;
    let now = unix_now() * 1000;
    playlist_service(
        http,
        &token,
        "add the new playlist to your library",
        &format!("/user/{user}/rootlist/changes"),
        json!({
            "deltas": [{
                "ops": [{
                    "kind": "ADD",
                    "add": {
                        "addFirst": true,
                        "items": [{ "uri": uri, "attributes": { "timestamp": now.to_string() } }]
                    }
                }],
                "info": { "source": { "client": "WEBPLAYER" } },
            }],
        }),
    )?;
    *LISTS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(id)
}

// --------------------------------------------------------- the other way
//
// Two-way sync also needs a YouTube Music song's Spotify song. It is found
// by the same scoring as the other way, with the two swapped round.

/// A YouTube Music song as a Spotify song would be: its name, artists,
/// album and length, read from its title and subtitle.
fn as_track(item: &Item) -> Track {
    let artists: Vec<String> = if item.artists.is_empty() {
        vec![item.subtitle.split(" • ").next().unwrap_or("").to_string()]
    } else {
        item.artists.iter().map(|(n, _)| n.clone()).collect()
    };
    let secs = if item.duration.is_empty() { seconds_in(&item.subtitle).unwrap_or(0.0) } else { item.duration_secs() };
    Track {
        id: item.video_id.clone().unwrap_or_default(),
        title: video_title(&item.title, &artists),
        album: item.album.as_ref().map(|(n, _)| n.clone()).unwrap_or_default(),
        artists,
        duration_ms: (secs * 1000.0) as u64,
        ..Track::default()
    }
}

/// A Spotify song as a YouTube Music search result, for `score`.
fn as_result(track: &Track) -> Item {
    Item {
        kind: Kind::Song,
        title: track.title.clone(),
        subtitle: format!("{} • {} • {}", track.artists.join(", "), track.album, clock(track.duration_ms)),
        artists: track.artists.iter().map(|a| (a.clone(), None)).collect(),
        ..Item::default()
    }
}

/// Songs found on Spotify for a search.
pub fn search_tracks(http: &ureq::Agent, token: &str, words: &str, limit: u64) -> Result<Vec<Track>, String> {
    let root = query(
        http,
        token,
        SEARCH_TRACKS,
        json!({
            "searchTerm": words,
            "offset": 0,
            "limit": limit,
            "numberOfTopResults": 5,
            "includeAudiobooks": false,
            "includePreReleases": false,
            "includeAuthors": false,
        }),
    )?;
    Ok(root
        .pointer("/data/searchV2/tracksV2/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let data = &entry["item"]["data"];
            track_of(data, data.get("uri").and_then(Value::as_str)?)
        })
        .collect())
}

/// The Spotify song that is this YouTube Music song, or None. Remembered
/// either way, beside the matches the other way.
pub fn find_on_spotify(http: &ureq::Agent, token: &str, item: &Item) -> Result<Option<Track>, String> {
    let Some(video) = item.video_id.as_deref() else { return Ok(None) };
    if let Some(known) = known_track(video) {
        return Ok(known);
    }
    let want = as_track(item);
    let artist = want.artists.first().cloned().unwrap_or_default();
    let tries = [format!("{} {artist}", want.title), format!("{} {}", core_title_words(&want.title), want.artists.join(" "))];
    let mut best: Option<(f32, Track)> = None;
    for words in tries {
        if best.as_ref().is_some_and(|(s, _)| *s >= ENOUGH) {
            break;
        }
        // Spotify's order is its own guess at the best: it breaks ties.
        let found = search_tracks(http, token, &words, 10)?
            .into_iter()
            .enumerate()
            .filter_map(|(n, t)| Some((score(&want, &as_result(&t))? - 0.01 * n as f32, t)))
            .max_by(|a, b| a.0.total_cmp(&b.0));
        if found.as_ref().map(|f| f.0) > best.as_ref().map(|b| b.0) {
            best = found;
        }
    }
    let found = best.filter(|(s, _)| *s >= ENOUGH).map(|(_, t)| t);
    remember_track(video, found.as_ref());
    Ok(found)
}

/// A title's words without its brackets and " - " part, for a second
/// search: "Uprising (Live)" is "Uprising".
/// Which of `tracks` a YouTube Music song is, if one fits well enough:
/// the same song in another version or upload still counts as the one
/// already there, so sync does not bring in a second copy.
pub fn same_song_in(tracks: &[Track], item: &Item) -> Option<usize> {
    let as_song = Item {
        title: video_title(&item.title, &as_track(item).artists),
        ..item.clone()
    };
    tracks
        .iter()
        .enumerate()
        .filter_map(|(n, t)| Some((n, score(t, &as_song)?)))
        .filter(|(_, s)| *s >= ENOUGH)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(n, _)| n)
}

fn core_title_words(title: &str) -> String {
    let base = title.split(" - ").next().unwrap_or(title);
    let mut out = String::new();
    let mut depth = 0i32;
    for c in base.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn track_json(t: &Track) -> Value {
    json!({
        "id": t.id, "title": t.title, "artists": t.artists, "album": t.album,
        "thumb": t.thumb, "ms": t.duration_ms, "explicit": t.explicit,
    })
}

fn track_from_json(v: &Value) -> Option<Track> {
    Some(Track {
        id: v.get("id")?.as_str()?.to_string(),
        title: text(v, "/title"),
        artists: v
            .get("artists")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str().map(str::to_string))
            .collect(),
        album: text(v, "/album"),
        thumb: text(v, "/thumb"),
        duration_ms: v.get("ms").and_then(Value::as_u64).unwrap_or(0),
        explicit: v.get("explicit").and_then(Value::as_bool).unwrap_or(false),
        uid: String::new(),
    })
}

/// What we know of a YouTube Music song's Spotify song, as `known` does
/// the other way. Kept in the same file, under "yt:" and the video id.
pub fn known_track(video_id: &str) -> Option<Option<Track>> {
    with_matches(|map| {
        let entry = map.get(&format!("yt:{video_id}"))?;
        if let Some(track) = entry.get("track") {
            return track_from_json(track).map(Some);
        }
        let at = entry.get("none").and_then(Value::as_u64).unwrap_or(0);
        (unix_now().saturating_sub(at) < RETRY_MISSING).then_some(None)
    })
}

fn remember_track(video_id: &str, found: Option<&Track>) {
    let entry = match found {
        Some(t) => json!({ "track": track_json(t) }),
        None => json!({ "none": unix_now() }),
    };
    with_matches(|map| map.insert(format!("yt:{video_id}"), entry));
    if UNSAVED.fetch_add(1, Ordering::SeqCst) + 1 >= 25 {
        save_matches();
    }
}

// ------------------------------------------------------------------- pages

fn card(id: &str, title: &str, subtitle: String, thumb: &str) -> Item {
    Item {
        kind: Kind::Playlist,
        title: title.to_string(),
        subtitle,
        thumb: thumb.to_string(),
        browse_id: Some(id.to_string()),
        ..Item::default()
    }
}

/// Liked Songs, then your playlists, as cards.
fn cards(lists: &[Playlist]) -> Vec<Item> {
    let mut cards = vec![card(LIKED, "Liked Songs", "Playlist • Spotify".into(), LIKED_COVER)];
    for list in lists {
        let subtitle = match list.owner.as_str() {
            "" => "Playlist".to_string(),
            owner => format!("Playlist • {owner}"),
        };
        cards.push(card(&format!("{PREFIX}playlist:{}", list.id), &list.name, subtitle, &list.thumb));
    }
    cards
}

/// The row of your Spotify playlists over Library, with "Show all".
pub fn library_shelf(http: &ureq::Agent, cookie: &str) -> Result<Shelf, String> {
    let lists = playlists_cached(http, cookie)?;
    Ok(Shelf {
        title: "Spotify".into(),
        carousel: true,
        items: cards(&lists),
        more: Some((format!("{PREFIX}library"), None)),
        ..Shelf::default()
    })
}

/// The card Library shows before Spotify is connected, which opens the
/// steps in Settings.
pub const CONNECT: &str = "spotify:connect";

pub fn connect_shelf() -> Shelf {
    Shelf {
        title: "Spotify".into(),
        carousel: true,
        items: vec![card(CONNECT, "Connect Spotify", "See your Spotify playlists here and sync them with YouTube Music".into(), "")],
        ..Shelf::default()
    }
}

fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// "808 songs • 52 hours, 3 minutes", as YouTube Music words a playlist.
fn meta(tracks: &[Track]) -> String {
    let minutes = tracks.iter().map(|t| t.duration_ms).sum::<u64>() / 60_000;
    let songs = match tracks.len() {
        1 => "1 song".to_string(),
        n => format!("{n} songs"),
    };
    let plural = |n: u64, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{songs} • {}", plural(m, "minute")),
        (h, 0) => format!("{songs} • {}", plural(h, "hour")),
        (h, m) => format!("{songs} • {}, {}", plural(h, "hour"), plural(m, "minute")),
    }
}

/// A Spotify song as a row. It plays once `found` gives it the YouTube
/// Music song; until then it has no video id. The names stay Spotify's,
/// which is what the playlist shows on Spotify, with YouTube Music's links
/// to the artists and album where the names agree.
pub fn song_item(track: &Track, found: Option<&Item>) -> Item {
    let artists = track
        .artists
        .iter()
        .map(|name| {
            let link = found.and_then(|f| {
                f.artists
                    .iter()
                    .find(|(n, id)| id.is_some() && plain(n) == plain(name))
                    .and_then(|(_, id)| id.clone())
            });
            (name.clone(), link)
        })
        .collect();
    let mut subtitle = track.artists.join(", ");
    if !track.album.is_empty() {
        subtitle = format!("{subtitle} • {}", track.album);
    }
    Item {
        kind: Kind::Song,
        title: track.title.clone(),
        subtitle,
        thumb: track.thumb.clone(),
        duration: clock(track.duration_ms),
        video_id: found.and_then(|f| f.video_id.clone()),
        artists,
        album: found
            .and_then(|f| f.album.as_ref())
            .map(|(_, id)| (track.album.clone(), id.clone())),
        explicit: track.explicit,
        ..Item::default()
    }
}

/// A playlist or Liked Songs by its browse id, with its songs.
pub fn load(http: &ureq::Agent, cookie: &str, id: &str) -> Result<(Playlist, Vec<Track>), String> {
    if cookie.is_empty() {
        return Err("Add your Spotify sp_dc cookie in Settings to open your Spotify library.".into());
    }
    if id == LIKED {
        let tracks = liked(http, cookie)?;
        let list = Playlist {
            name: "Liked Songs".into(),
            owner: "Spotify".into(),
            thumb: LIKED_COVER.into(),
            editable: true,
            ..Playlist::default()
        };
        return Ok((list, tracks));
    }
    let playlist_id = id
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_prefix("playlist:"))
        .ok_or("Tonearm cannot open that Spotify link")?;
    playlist(http, cookie, playlist_id)
}

/// Whether a browse id is a Spotify playlist or Liked Songs: something
/// with songs, which can be copied to YouTube Music.
pub fn is_playlist(id: &str) -> bool {
    id == LIKED || id.starts_with(&format!("{PREFIX}playlist:"))
}

/// A Spotify page by its browse id: your library as a grid, or a
/// playlist or Liked Songs with its songs. Songs matched before come back
/// ready to play; the tracks go back too, for matching the rest.
pub fn page(http: &ureq::Agent, cookie: &str, id: &str) -> Result<(Page, Vec<Track>), String> {
    if cookie.is_empty() {
        return Err("Add your Spotify sp_dc cookie in Settings to open your Spotify library.".into());
    }
    if id == format!("{PREFIX}library") {
        let lists = playlists_cached(http, cookie)?;
        let page = Page {
            title: "Spotify".into(),
            shelves: vec![Shelf {
                carousel: true,
                items: cards(&lists),
                ..Shelf::default()
            }],
            ..Page::default()
        };
        return Ok((page, Vec::new()));
    }
    let (list, tracks) = load(http, cookie, id)?;
    let items: Vec<Item> = tracks.iter().map(|t| song_item(t, known(t).flatten().as_ref())).collect();
    let header = Header {
        kind: PageKind::Playlist,
        label: "Spotify playlist".into(),
        title: list.name.clone(),
        byline: list.owner.clone(),
        meta: meta(&tracks),
        about: list.about.clone(),
        thumb: list.thumb.clone(),
        ..Header::default()
    };
    let page = Page {
        title: list.name,
        shelves: vec![Shelf {
            items,
            ..Shelf::default()
        }],
        header: Some(header),
        ..Page::default()
    };
    Ok((page, tracks))
}

// ---------------------------------------------------------------- matching

/// Matches made, by Spotify track id: the YouTube Music song, or when
/// none was found, when we last looked. Kept on disk.
static MATCHES: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);
/// Matches made since the file was last written.
static UNSAVED: AtomicUsize = AtomicUsize::new(0);
/// A song not found is looked for again after this long: YouTube Music
/// may have it by then.
const RETRY_MISSING: u64 = 7 * 24 * 3600;

fn matches_file() -> Option<PathBuf> {
    Some(crate::store::data_dir()?.join("spotify-matches.json"))
}

fn with_matches<T>(f: impl FnOnce(&mut HashMap<String, Value>) -> T) -> T {
    let mut guard = MATCHES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(|| {
        matches_file()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    });
    f(map)
}

/// Writes the matches to disk, by way of a temporary file so a crash
/// mid-write loses nothing.
pub fn save_matches() {
    UNSAVED.store(0, Ordering::SeqCst);
    let Some(path) = matches_file() else { return };
    let text = with_matches(|map| serde_json::to_string(map).unwrap_or_default());
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let temp = path.with_extension("tmp");
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What we know of a song: Some(Some) its YouTube Music song, Some(None)
/// looked for lately and not found, None not looked for yet.
pub fn known(track: &Track) -> Option<Option<Item>> {
    with_matches(|map| {
        let entry = map.get(&track.id)?;
        if let Some(item) = entry.get("item") {
            return Some(Some(Item::from_json(item)));
        }
        // A miss from an older way of looking is looked for again: the
        // search may find it now.
        if entry.get("v").and_then(Value::as_u64) != Some(MATCHING_VERSION) {
            return None;
        }
        let at = entry.get("none").and_then(Value::as_u64).unwrap_or(0);
        (unix_now().saturating_sub(at) < RETRY_MISSING).then_some(None)
    })
}

/// Raised when matching learns to find songs it missed before, so those
/// misses are retried. 2: videos by the artist count.
const MATCHING_VERSION: u64 = 2;

fn remember(track: &Track, found: Option<&Item>) {
    let entry = match found {
        Some(item) => json!({ "item": item.to_json() }),
        None => json!({ "none": unix_now(), "v": MATCHING_VERSION }),
    };
    with_matches(|map| {
        map.insert(track.id.clone(), entry);
        // The same pair the other way, so two-way sync sees one song where
        // there is one, not a second to copy back.
        if let Some(video) = found.and_then(|i| i.video_id.as_deref()) {
            let key = format!("yt:{video}");
            if map.get(&key).is_none_or(|e| e.get("track").is_none()) {
                map.insert(key, json!({ "track": track_json(track) }));
            }
        }
    });
    if UNSAVED.fetch_add(1, Ordering::SeqCst) + 1 >= 25 {
        save_matches();
    }
}

/// Letters and digits, lowercased, with the common accents dropped, so
/// "Beyoncé" and "Beyonce" compare equal.
fn plain(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// The title without what versions add to it: Spotify's " - Remastered
/// 2009" and YouTube Music's "(Remastered 2009)", "(feat. X)" and the like.
fn core_title(title: &str) -> String {
    let base = title.split(" - ").next().unwrap_or(title);
    let mut out = String::new();
    let mut depth = 0i32;
    for c in base.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let core = plain(&out);
    // A title that is all brackets keeps them.
    if core.is_empty() { plain(title) } else { core }
}

/// Words that make a different recording of the same song. A match must
/// agree on these: a remix is not the original.
fn versions(title: &str) -> Vec<&'static str> {
    // These name the usual cut, not another one.
    let lower = title
        .to_lowercase()
        .replace("original mix", "")
        .replace("radio mix", "")
        .replace("album mix", "");
    [
        "remix", "live", "acoustic", "instrumental", "karaoke", "sped up", "slowed", "cover", "demo",
        "reprise", "stem", "stems", "nightcore", "8d", "extended", "a cappella", "acapella", "unplugged",
        "bass boosted", "reverb", "re-recorded", "rerecorded", "re-record", "mix",
    ]
    .into_iter()
        .filter(|word| {
            lower.match_indices(word).any(|(at, _)| {
                let before = lower[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
                let after = lower[at + word.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
                before && after
            })
        })
        .collect()
}

/// A search result's length, from its subtitle: "Artist • Album • 3:04".
fn seconds_in(subtitle: &str) -> Option<f32> {
    subtitle.split(" • ").find_map(|part| {
        let pieces: Vec<&str> = part.trim().split(':').collect();
        if pieces.len() < 2 || pieces.len() > 3 {
            return None;
        }
        let mut total = 0u32;
        for piece in pieces {
            total = total * 60 + piece.parse::<u32>().ok()?;
        }
        Some(total as f32)
    })
}

/// How well a search result fits the song, or None when it is a
/// different song. Title, artist, length and version all count.
fn score(track: &Track, candidate: &Item) -> Option<f32> {
    let (want, have) = (core_title(&track.title), core_title(&candidate.title));
    let mut score = if want == have {
        3.0
    } else if want.len().min(have.len()) >= 3 && (have.contains(&want) || want.contains(&have)) {
        1.5
    } else {
        return None;
    };
    let theirs: Vec<String> = if candidate.artists.is_empty() {
        vec![plain(candidate.subtitle.split(" • ").next().unwrap_or(""))]
    } else {
        candidate.artists.iter().map(|(n, _)| plain(n)).collect()
    };
    let close = |a: &str, b: &str| {
        !a.is_empty() && !b.is_empty() && (a == b || (a.len().min(b.len()) >= 3 && (a.contains(b) || b.contains(a))))
    };
    let same_artist = track.artists.iter().any(|ours| theirs.iter().any(|t| close(&plain(ours), t)));
    // Someone else's song of the same name and length is a cover, never
    // the song, so a different artist weighs more than any length can.
    score += if same_artist { 2.0 } else { -3.0 };
    if let Some(length) = seconds_in(&candidate.subtitle).filter(|_| track.duration_ms > 0) {
        let off = (length - track.duration_secs()).abs();
        score += if off <= 2.5 {
            2.0
        } else if off <= 6.0 {
            1.0
        } else if off <= 15.0 {
            0.0
        } else if off <= 30.0 {
            // The artist's own song under the same title, in another edit:
            // better than nothing, which is what a bigger cost would leave.
            -1.5
        } else {
            // Not the same recording: an extended cut, a loop, a snippet.
            return None;
        };
    }
    // The same album breaks a tie with a compilation or a reissue.
    let their_album = candidate.subtitle.split(" • ").nth(1).unwrap_or("");
    if !track.album.is_empty() && core_title(&track.album) == core_title(their_album) {
        score += 0.5;
    }
    // Another recording, however well the rest fits.
    if versions(&track.title) != versions(&candidate.title) {
        score -= 5.0;
    }
    Some(score)
}

/// Good enough to play: the title and artist agree, or the title and the
/// length do.
const ENOUGH: f32 = 3.5;

/// A search result's plays, from its subtitle: "… • 1.9M plays", or a
/// video's "… • 182K views".
fn plays_in(subtitle: &str) -> f64 {
    subtitle
        .split(" • ")
        .find_map(|part| {
            let part = part.trim();
            let number = ["plays", "play", "views", "view"]
                .iter()
                .find_map(|word| part.strip_suffix(word))?
                .trim_end();
            let (digits, scale) = match number.chars().last()? {
                'K' => (&number[..number.len() - 1], 1e3),
                'M' => (&number[..number.len() - 1], 1e6),
                'B' => (&number[..number.len() - 1], 1e9),
                _ => (number, 1.0),
            };
            Some(digits.replace(',', "").parse::<f64>().ok()? * scale)
        })
        .unwrap_or(0.0)
}

/// A video's title as a song title: "hannah ponce - beautiful places
/// (official audio)" is "beautiful places", once the artist in front is
/// taken off. `core_title` then drops the bracketed part.
fn video_title(title: &str, artists: &[String]) -> String {
    if let Some((front, rest)) = title.split_once(" - ") {
        if artists.iter().any(|a| plain(a) == plain(front)) {
            return rest.trim().to_string();
        }
    }
    title.to_string()
}

/// Where to search: YouTube Music's songs, or its videos for a song only
/// uploaded as one.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Songs,
    Videos,
}

/// The best result for the song with its score. Between results that fit
/// as well, the one played most wins: the original over a re-upload.
fn best_of(client: &Client, track: &Track, query: &str, source: Source) -> Option<(f32, Item)> {
    let tab = if source == Source::Videos { 6 } else { 1 };
    let page = client.search_in(query, tab).ok()?;
    let rank = |(s, i): &(f32, Item)| *s as f64 + 0.1 * (1.0 + plays_in(&i.subtitle)).log10();
    page.shelves
        .into_iter()
        .flat_map(|s| s.items)
        .filter(|i| i.video_id.is_some())
        .filter_map(|i| {
            let judged = match source {
                Source::Songs => score(track, &i)?,
                Source::Videos => {
                    let as_song = Item {
                        title: video_title(&i.title, &track.artists),
                        ..i.clone()
                    };
                    score(track, &as_song)?
                }
            };
            Some((judged, i))
        })
        .max_by(|a, b| rank(a).total_cmp(&rank(b)))
}

/// The YouTube Music song that is this Spotify song, searched for by its
/// title and artist, then by its bare title and all its artists if that
/// finds nothing good. Last of all, a video: some songs are only on
/// YouTube as the artist's own upload, "Artist - Song (Official Audio)".
/// The row keeps Spotify's names and cover either way; only the sound
/// comes from the video. Remembered either way.
pub fn find(client: &Client, track: &Track) -> Option<Item> {
    let artist = track.artists.first().map(String::as_str).unwrap_or("");
    let bare = track.title.split(" - ").next().unwrap_or(&track.title);
    let tries = [
        (format!("{} {artist}", track.title), Source::Songs),
        (format!("{bare} {}", track.artists.join(" ")), Source::Songs),
        (format!("{bare} {artist}"), Source::Videos),
    ];
    let mut best: Option<(f32, Item)> = None;
    for (query, source) in tries {
        if best.as_ref().is_some_and(|(s, _)| *s >= ENOUGH) {
            break;
        }
        let found = best_of(client, track, &query, source);
        if found.as_ref().map(|f| f.0) > best.as_ref().map(|b| b.0) {
            best = found;
        }
    }
    let found = best.filter(|(s, _)| *s >= ENOUGH).map(|(_, item)| item);
    remember(track, found.as_ref());
    found
}

/// Every song's YouTube Music song, or None where there is none, in the
/// playlist's order: remembered matches at once, the rest searched four
/// at a time. Blocks. `progress` hears how many are answered so far.
pub fn match_all(client: &Client, tracks: &[Track], progress: impl Fn(usize) + Sync) -> Vec<Option<Item>> {
    let answers: Vec<Mutex<Option<Option<Item>>>> = tracks.iter().map(|t| Mutex::new(known(t))).collect();
    let pending: Vec<usize> = (0..tracks.len())
        .filter(|&i| answers[i].lock().unwrap_or_else(|e| e.into_inner()).is_none())
        .collect();
    let done = AtomicUsize::new(tracks.len() - pending.len());
    let next = AtomicUsize::new(0);
    progress(done.load(Ordering::SeqCst));
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| loop {
                let n = next.fetch_add(1, Ordering::SeqCst);
                let Some(&i) = pending.get(n) else { return };
                let found = find(client, &tracks[i]);
                *answers[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(found);
                progress(done.fetch_add(1, Ordering::SeqCst) + 1);
            });
        }
    });
    save_matches();
    answers
        .into_iter()
        .map(|a| a.into_inner().unwrap_or_else(|e| e.into_inner()).flatten())
        .collect()
}

/// Whether a match is worth a second look: its title or artist is not
/// plainly the Spotify song's. For checking matches by eye.
pub fn doubtful(track: &Track, found: &Item) -> bool {
    let video = found.title.contains(" - ") && !found.title.contains(" - Remaster");
    let title = video_title(&found.title, &track.artists);
    let same_title = core_title(&track.title) == core_title(&title);
    let same_artist = found
        .artists
        .iter()
        .any(|(n, _)| track.artists.iter().any(|a| plain(a) == plain(n)));
    !same_title || !same_artist || video
}

// ------------------------------------------------------------------ copies

/// Spotify playlists copied to YouTube Music: by Spotify browse id, the
/// YouTube Music playlist and when. Copying again adds to that playlist
/// instead of making another, and two-way sync will start from here.
fn copies_file() -> Option<PathBuf> {
    Some(crate::store::data_dir()?.join("spotify-copies.json"))
}

fn read_copies() -> serde_json::Map<String, Value> {
    copies_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// The YouTube Music playlist a Spotify playlist was copied to, if any.
pub fn copy_of(id: &str) -> Option<String> {
    read_copies()
        .get(id)?
        .get("playlist")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Notes that `id` was copied to `playlist`: how many songs went in, and
/// which Spotify songs YouTube Music does not have.
pub fn record_copy(id: &str, playlist: &str, added: usize, missing: &[&Track]) {
    let mut copies = read_copies();
    copies.insert(
        id.to_string(),
        json!({
            "playlist": playlist,
            "at": unix_now(),
            "songs": added,
            "missing": missing.iter().map(|t| json!({ "id": t.id, "title": t.title, "artists": t.artists })).collect::<Vec<_>>(),
        }),
    );
    let Some(path) = copies_file() else { return };
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let temp = path.with_extension("tmp");
    if std::fs::write(&temp, Value::Object(copies).to_string()).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// How far a copy has got, for the toast that follows it.
pub enum Stage {
    /// Songs answered, of all of them.
    Finding(usize, usize),
    /// Songs added to the YouTube Music playlist, of all found.
    Adding(usize, usize),
}

/// What a copy did.
pub struct Copied {
    /// The YouTube Music playlist's id.
    pub playlist: String,
    /// Songs found on YouTube Music and sent to the playlist.
    pub songs: usize,
    /// Songs YouTube Music does not have.
    pub missing: usize,
    /// The playlist was there from a copy before, and only took the songs
    /// it lacked.
    pub updated: bool,
}

/// Copies a Spotify playlist, or Liked Songs, into a private YouTube Music
/// playlist of the same name. One copied before takes the songs it lacks
/// instead, in order at its end, so copying again brings in what was added
/// on Spotify. Nothing is ever taken out: that waits for two-way sync and
/// its safeguards. Blocks for as long as the matching takes.
pub fn copy(
    client: &Client,
    http: &ureq::Agent,
    cookie: &str,
    id: &str,
    progress: impl Fn(Stage) + Sync,
) -> Result<Copied, String> {
    let (list, tracks) = load(http, cookie, id)?;
    let found = match_all(client, &tracks, |n| progress(Stage::Finding(n, tracks.len())));
    // In the playlist's order, each song once.
    let mut ids: Vec<String> = Vec::new();
    for video in found.iter().flatten().filter_map(|i| i.video_id.clone()) {
        if !ids.contains(&video) {
            ids.push(video);
        }
    }
    let missing: Vec<&Track> = tracks.iter().zip(&found).filter(|(_, f)| f.is_none()).map(|(t, _)| t).collect();
    if ids.is_empty() {
        return Err("None of these songs are on YouTube Music".into());
    }
    let add = |playlist: &str| client.add_many(playlist, &ids, |n| progress(Stage::Adding(n, ids.len())));
    let make = || -> Result<String, String> {
        // "Liked Songs" alone would sit beside YouTube Music's own liked list.
        let name = if id == LIKED { "Spotify Liked Songs".to_string() } else { list.name.clone() };
        let playlist = client.create_described(&name, "Copied from Spotify by Tonearm.", &[])?;
        add(&playlist)?;
        Ok(playlist)
    };
    let (playlist, updated) = match copy_of(id) {
        Some(existing) => match add(&existing) {
            Ok(()) => (existing, true),
            Err(message) => {
                // Deleted on YouTube Music: make it again. Anything else,
                // such as no connection, is reported, or a second copy
                // would appear.
                if client.browse(&format!("VL{existing}"), None).is_ok() {
                    return Err(message);
                }
                forget_copy(id);
                (make()?, false)
            }
        },
        None => (make()?, false),
    };
    record_copy(id, &playlist, ids.len(), &missing);
    Ok(Copied {
        playlist,
        songs: ids.len(),
        missing: missing.len(),
        updated,
    })
}

/// Forgets a copy whose YouTube Music playlist is gone, so the next copy
/// makes a new one.
pub fn forget_copy(id: &str) {
    let mut copies = read_copies();
    if copies.remove(id).is_none() {
        return;
    }
    if let Some(path) = copies_file() {
        let _ = std::fs::write(path, Value::Object(copies).to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artists: &[&str], secs: u64) -> Track {
        Track {
            id: "x".into(),
            title: title.into(),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            duration_ms: secs * 1000,
            ..Track::default()
        }
    }

    fn result(title: &str, artists: &[&str], length: &str) -> Item {
        Item {
            title: title.into(),
            subtitle: format!("{} • Album • {length} • 1M plays", artists.join(" & ")),
            artists: artists.iter().map(|a| (a.to_string(), None)).collect(),
            video_id: Some("v".into()),
            ..Item::default()
        }
    }

    #[test]
    fn a_remaster_matches_across_both_ways_of_writing_it() {
        let t = track("Body Movin' - Remastered 2009", &["Beastie Boys"], 184);
        let s = score(&t, &result("Body Movin' (Remastered 2009)", &["Beastie Boys"], "3:04")).unwrap();
        assert!(s >= ENOUGH, "{s}");
    }

    #[test]
    fn a_remix_of_the_song_is_not_the_song() {
        let t = track("Body Movin' - Remastered 2009", &["Beastie Boys"], 184);
        let s = score(&t, &result("Body Movin' (Fatboy Slim Remix/Remastered 2009)", &["Beastie Boys"], "5:33"));
        assert!(s.is_none_or(|s| s < ENOUGH), "{s:?}");
    }

    #[test]
    fn accents_and_joined_artists_still_match() {
        let t = track("Si Me Voy (with The Marías)", &["Cuco", "The Marías"], 203);
        let s = score(&t, &result("Si Me Voy", &["Cuco", "The Marias"], "3:24")).unwrap();
        assert!(s >= ENOUGH, "{s}");
    }

    #[test]
    fn another_song_with_another_title_is_no_match() {
        let t = track("Uprising", &["Muse"], 305);
        assert!(score(&t, &result("Supermassive Black Hole", &["Muse"], "3:32")).is_none());
    }

    #[test]
    fn a_cover_of_the_same_length_is_not_the_song() {
        let t = track("Intergalactic", &["Beastie Boys"], 210);
        let s = score(&t, &result("Intergalactic", &["Dolladolla"], "3:29")).unwrap();
        assert!(s < ENOUGH, "{s}");
    }

    #[test]
    fn the_artists_own_song_in_another_edit_still_plays() {
        let t = track("Intergalactic", &["Beastie Boys"], 210);
        let s = score(&t, &result("Intergalactic", &["Beastie Boys"], "3:51")).unwrap();
        assert!(s >= ENOUGH, "{s}");
    }

    #[test]
    fn half_a_minute_off_is_another_recording() {
        let t = track("Uprising", &["Muse"], 305);
        assert!(score(&t, &result("Uprising", &["Muse"], "6:10")).is_none());
    }

    #[test]
    fn the_same_album_wins_a_tie() {
        let mut t = track("Uprising", &["Muse"], 305);
        t.album = "The Resistance".into();
        let mut on_album = result("Uprising", &["Muse"], "5:05");
        on_album.subtitle = "Muse • The Resistance • 5:05 • 500M plays".into();
        let mut compilation = result("Uprising", &["Muse"], "5:05");
        compilation.subtitle = "Muse • Rock Hits 2010 • 5:05 • 500M plays".into();
        assert!(score(&t, &on_album).unwrap() > score(&t, &compilation).unwrap());
    }

    #[test]
    fn a_stem_is_not_the_song() {
        let t = track("Ice Ice Baby", &["Vanilla Ice"], 255);
        let s = score(&t, &result("Ice Ice Baby (Bass Stem)", &["Vanilla Ice"], "4:15")).unwrap();
        assert!(s < ENOUGH, "{s}");
    }

    #[test]
    fn the_artists_own_upload_stands_in_for_the_song() {
        let t = track("Beautiful Places", &["Hannah Ponce"], 176);
        let mut video = result("x", &["Hannah Ponce"], "2:57");
        video.title = video_title("hannah ponce - beautiful places (official audio)", &t.artists);
        video.subtitle = "Hannah Ponce • 182K views • 2:57".into();
        let s = score(&t, &video).unwrap();
        assert!(s >= ENOUGH, "{s}");
    }

    #[test]
    fn a_fans_slowed_upload_does_not() {
        let t = track("Beautiful Places", &["Hannah Ponce"], 176);
        let mut video = result("x", &["luu_muu"], "3:37");
        video.title = video_title("hannah ponce - beautiful places   slowed and reverb", &t.artists);
        assert!(score(&t, &video).is_none_or(|s| s < ENOUGH));
    }

    #[test]
    fn plays_read_from_the_subtitle() {
        assert_eq!(plays_in("Hannah Ponce • 182K views • 2:57"), 182e3);
        assert_eq!(plays_in("Muse • The Resistance • 5:05 • 1.9M plays"), 1.9e6);
        assert_eq!(plays_in("Luqman • Altered Thinking • 2:06 • 273 plays"), 273.0);
        assert_eq!(plays_in("Muse • The Resistance"), 0.0);
    }

    #[test]
    fn lengths_read_from_the_subtitle() {
        assert_eq!(seconds_in("Muse • The Resistance • 5:05 • 300M plays"), Some(305.0));
        assert_eq!(seconds_in("Muse • 1:02:03"), Some(3723.0));
        assert_eq!(seconds_in("Muse • The Resistance"), None);
    }
}
