//! Two-way sync carried out: pairs of a Spotify playlist and a YouTube
//! Music playlist, each side read, compared by `sync::plan`, and changed on
//! both services. Spotify's Liked Songs pairs with YouTube Music's liked
//! songs, so a like on either side is a like on both.
//!
//! Songs are keyed by their Spotify track, "sp:<id>", on both sides: a
//! YouTube Music song is looked up on Spotify (`spotify::find_on_spotify`),
//! so the same song under two different YouTube videos is still one song,
//! and nothing comes back as a second copy. A YouTube Music song Spotify
//! does not have keeps "yt:<video id>", which the Spotify side never has.
//!
//! Everything here blocks, and runs off the UI thread.

use crate::innertube::model::{Header, Item, Kind, Page, PageKind, Shelf};
use crate::innertube::Client;
use crate::spotify::{self, Track};
use crate::sync::{self, Plan, Side};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Browse ids of the pages this makes: a pair's missing songs, and the
/// removals held for the user.
pub const PREFIX: &str = "tonearm:sync:";
pub const MISSING: &str = "tonearm:sync:missing:";
pub const REVIEW: &str = "tonearm:sync:review:";
/// YouTube Music's liked songs, as a playlist id.
pub const LIKED_MUSIC: &str = "LM";
/// Syncs kept for undoing, per pair.
const KEEP_LOG: usize = 10;

/// A song as the log and the lists keep it: enough to name it and to put
/// it back on either side.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Song {
    pub key: String,
    pub title: String,
    pub artists: String,
    pub video_id: Option<String>,
    pub track_id: Option<String>,
}

/// What one sync changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub at: u64,
    pub to_spotify: Vec<Song>,
    pub to_youtube: Vec<Song>,
    pub from_spotify: Vec<Song>,
    pub from_youtube: Vec<Song>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.to_spotify.is_empty() && self.to_youtube.is_empty() && self.from_spotify.is_empty() && self.from_youtube.is_empty()
    }
}

/// A Spotify playlist and the YouTube Music playlist it syncs with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pair {
    /// The Spotify browse id: `spotify::LIKED` or "spotify:playlist:<id>".
    pub spotify: String,
    /// The YouTube Music playlist id, without "VL". `LIKED_MUSIC` for likes.
    pub youtube: String,
    pub name: String,
    /// Each side's songs after the last sync: Spotify's, YouTube Music's.
    pub base: Option<(Vec<String>, Vec<String>)>,
    /// Removals waiting for the user: from Spotify, from YouTube Music.
    pub held: (Vec<Song>, Vec<Song>),
    /// The latest syncs that changed something, oldest first.
    pub log: Vec<Change>,
    /// Songs one side has that the other cannot.
    pub not_on_youtube: Vec<Song>,
    pub not_on_spotify: Vec<Song>,
    pub last: u64,
    /// Whether each side, Spotify's and YouTube Music's, could be changed
    /// at the last sync. One that could not took nothing then, so once it
    /// can, the next sync starts again as a first one.
    pub writable: (bool, bool),
    /// The pair was made from the Spotify playlist, its YouTube Music side
    /// being the copy, or the other way round. Only the menu's wording
    /// follows it: songs go both ways.
    pub from_spotify: bool,
    /// A cover for the Syncs page.
    pub thumb: String,
    /// Each side on its own, for the Syncs page: Spotify's cover, and YouTube
    /// Music's title and cover.
    pub spotify_thumb: String,
    pub youtube_title: String,
    pub youtube_thumb: String,
    /// Each side's fingerprint after the last sync: Spotify's, YouTube
    /// Music's. One that differs now means that side changed, and the pair
    /// syncs again without waiting for the quarter hour.
    pub prints: (String, String),
}

impl Pair {
    pub fn held_count(&self) -> usize {
        self.held.0.len() + self.held.1.len()
    }

    /// Whether `id`, a Spotify browse id or a YouTube Music playlist or
    /// browse id, is one of this pair's playlists.
    pub fn has(&self, id: &str) -> bool {
        self.spotify == id || self.youtube == id.strip_prefix("VL").unwrap_or(id)
    }

    pub fn liked(&self) -> bool {
        self.spotify == spotify::LIKED
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---------------------------------------------------------------- storage

fn song_json(s: &Song) -> Value {
    json!({ "key": s.key, "title": s.title, "artists": s.artists, "video": s.video_id, "track": s.track_id })
}

fn song_from(v: &Value) -> Option<Song> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(Song {
        key: s("key")?,
        title: s("title").unwrap_or_default(),
        artists: s("artists").unwrap_or_default(),
        video_id: s("video"),
        track_id: s("track"),
    })
}

fn songs_json(list: &[Song]) -> Value {
    Value::Array(list.iter().map(song_json).collect())
}

fn songs_from(v: &Value) -> Vec<Song> {
    v.as_array().into_iter().flatten().filter_map(song_from).collect()
}

fn keys_from(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_string)).collect()
}

fn change_json(c: &Change) -> Value {
    json!({
        "at": c.at,
        "to_spotify": songs_json(&c.to_spotify),
        "to_youtube": songs_json(&c.to_youtube),
        "from_spotify": songs_json(&c.from_spotify),
        "from_youtube": songs_json(&c.from_youtube),
    })
}

fn change_from(v: &Value) -> Change {
    Change {
        at: v.get("at").and_then(Value::as_u64).unwrap_or(0),
        to_spotify: songs_from(&v["to_spotify"]),
        to_youtube: songs_from(&v["to_youtube"]),
        from_spotify: songs_from(&v["from_spotify"]),
        from_youtube: songs_from(&v["from_youtube"]),
    }
}

fn pair_json(p: &Pair) -> Value {
    json!({
        "spotify": p.spotify,
        "youtube": p.youtube,
        "name": p.name,
        "base": p.base.as_ref().map(|(a, b)| json!([a, b])),
        "held": [songs_json(&p.held.0), songs_json(&p.held.1)],
        "log": p.log.iter().map(change_json).collect::<Vec<_>>(),
        "not_on_youtube": songs_json(&p.not_on_youtube),
        "not_on_spotify": songs_json(&p.not_on_spotify),
        "last": p.last,
        "writable": [p.writable.0, p.writable.1],
        "from_spotify": p.from_spotify,
        "thumb": p.thumb,
        "spotify_thumb": p.spotify_thumb,
        "youtube_title": p.youtube_title,
        "youtube_thumb": p.youtube_thumb,
        "prints": [p.prints.0, p.prints.1],
    })
}

fn pair_from(v: &Value) -> Option<Pair> {
    Some(Pair {
        spotify: v.get("spotify")?.as_str()?.to_string(),
        youtube: v.get("youtube")?.as_str()?.to_string(),
        name: v.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
        base: v.get("base").filter(|b| b.is_array()).map(|b| (keys_from(&b[0]), keys_from(&b[1]))),
        held: (songs_from(&v["held"][0]), songs_from(&v["held"][1])),
        log: v["log"].as_array().into_iter().flatten().map(change_from).collect(),
        not_on_youtube: songs_from(&v["not_on_youtube"]),
        not_on_spotify: songs_from(&v["not_on_spotify"]),
        last: v.get("last").and_then(Value::as_u64).unwrap_or(0),
        // Saved before this was kept: as if neither could, so a pair whose
        // first sync was cut short by that gets another.
        writable: (
            v["writable"][0].as_bool().unwrap_or(false),
            v["writable"][1].as_bool().unwrap_or(false),
        ),
        from_spotify: v.get("from_spotify").and_then(Value::as_bool).unwrap_or(false),
        thumb: v.get("thumb").and_then(Value::as_str).unwrap_or_default().to_string(),
        spotify_thumb: v.get("spotify_thumb").and_then(Value::as_str).unwrap_or_default().to_string(),
        youtube_title: v.get("youtube_title").and_then(Value::as_str).unwrap_or_default().to_string(),
        youtube_thumb: v.get("youtube_thumb").and_then(Value::as_str).unwrap_or_default().to_string(),
        prints: (
            v["prints"][0].as_str().unwrap_or_default().to_string(),
            v["prints"][1].as_str().unwrap_or_default().to_string(),
        ),
    })
}

fn pairs_file() -> Option<PathBuf> {
    Some(crate::store::data_dir()?.join("sync-pairs.json"))
}

static PAIRS: Mutex<Option<Vec<Pair>>> = Mutex::new(None);

fn with_pairs<T>(f: impl FnOnce(&mut Vec<Pair>) -> T) -> T {
    let mut guard = PAIRS.lock().unwrap_or_else(|e| e.into_inner());
    let pairs = guard.get_or_insert_with(|| {
        pairs_file()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|v| v.as_array().map(|list| list.iter().filter_map(pair_from).collect()))
            .unwrap_or_default()
    });
    f(pairs)
}

/// Writes the pairs, by way of a temporary file so a crash mid-write
/// loses nothing.
fn save(pairs: &[Pair]) {
    let Some(path) = pairs_file() else { return };
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let text = Value::Array(pairs.iter().map(pair_json).collect()).to_string();
    let temp = path.with_extension("tmp");
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// Every pair.
pub fn pairs() -> Vec<Pair> {
    with_pairs(|p| p.clone())
}

/// The pair `id` belongs to: a Spotify browse id, or a YouTube Music
/// playlist or browse id.
pub fn pair_of(id: &str) -> Option<Pair> {
    with_pairs(|pairs| pairs.iter().find(|p| p.has(id)).cloned())
}

fn update(spotify_id: &str, f: impl FnOnce(&mut Pair)) {
    with_pairs(|pairs| {
        if let Some(p) = pairs.iter_mut().find(|p| p.spotify == spotify_id) {
            f(p);
        }
        save(pairs);
    });
}

/// Stops syncing a pair. Both playlists stay as they are.
pub fn unpair(spotify_id: &str) {
    with_pairs(|pairs| {
        pairs.retain(|p| p.spotify != spotify_id);
        save(pairs);
    });
}

fn add_pair(pair: Pair) {
    with_pairs(|pairs| {
        pairs.retain(|p| p.spotify != pair.spotify && p.youtube != pair.youtube);
        pairs.push(pair);
        save(pairs);
    });
}

// ---------------------------------------------------------------- pairing

/// Pairs a Spotify playlist, or Liked Songs, with YouTube Music: Liked
/// Songs with YouTube Music's liked songs, a playlist with its copy there
/// when it has one, or with a new one. The first sync fills it.
pub fn pair_spotify(client: &Client, spotify_id: &str, name: &str) -> Result<Pair, String> {
    let youtube = if spotify_id == spotify::LIKED {
        LIKED_MUSIC.to_string()
    } else {
        let existing = spotify::copy_of(spotify_id).filter(|id| client.browse(&format!("VL{id}"), None).is_ok());
        match existing {
            Some(id) => id,
            None => client.create_described(name, "Synced with Spotify by Tonearm.", &[])?,
        }
    };
    let pair = Pair { spotify: spotify_id.to_string(), youtube, name: name.to_string(), from_spotify: true, ..Pair::default() };
    add_pair(pair.clone());
    Ok(pair)
}

/// Pairs a YouTube Music playlist, or its liked songs, with Spotify: liked
/// songs with Liked Songs, a playlist with a new Spotify playlist of the
/// same name. The first sync fills it.
pub fn pair_youtube(http: &ureq::Agent, cookie: &str, youtube_id: &str, name: &str) -> Result<Pair, String> {
    let youtube = youtube_id.strip_prefix("VL").unwrap_or(youtube_id).to_string();
    let spotify_id = if youtube == LIKED_MUSIC {
        spotify::LIKED.to_string()
    } else {
        format!("{}playlist:{}", spotify::PREFIX, spotify::create_playlist(http, cookie, name)?)
    };
    let pair = Pair { spotify: spotify_id, youtube, name: name.to_string(), ..Pair::default() };
    add_pair(pair.clone());
    Ok(pair)
}

// ---------------------------------------------------------------- reading

/// Both sides as keys, and what changing them needs.
#[derive(Default)]
struct Keyed {
    spotify: Vec<String>,
    youtube: Vec<String>,
    only_spotify: HashSet<String>,
    only_youtube: HashSet<String>,
    songs: HashMap<String, Song>,
    /// Each song's places in the Spotify playlist.
    uids: HashMap<String, Vec<String>>,
    /// Each song's videos and places in the YouTube Music playlist.
    places: HashMap<String, Vec<(String, String)>>,
    spotify_writable: bool,
    youtube_writable: bool,
    name: String,
    thumb: String,
    spotify_thumb: String,
    youtube_title: String,
    youtube_thumb: String,
}

fn songs_of(page: &Page) -> Vec<Item> {
    page.shelves
        .iter()
        .filter(|s| !s.carousel)
        .flat_map(|s| s.items.iter())
        .filter(|i| i.video_id.is_some())
        .cloned()
        .collect()
}

/// A YouTube Music playlist's songs, every page of them, whether it can
/// be changed, and its title and cover.
fn youtube_songs(client: &Client, playlist: &str) -> Result<(Vec<Item>, bool, (String, String)), String> {
    let page = client.browse(&format!("VL{playlist}"), None)?;
    let editable = playlist == LIKED_MUSIC || page.header.as_ref().is_some_and(|h| h.editable);
    let thumb = page
        .header
        .as_ref()
        .map(|h| (if h.title.is_empty() { page.title.clone() } else { h.title.clone() }, h.thumb.clone()))
        .unwrap_or_else(|| (page.title.clone(), String::new()));
    let mut songs = songs_of(&page);
    let mut token = page.continuation;
    // 10,000 songs at 100 a page.
    for _ in 0..100 {
        let Some(t) = token.take() else { break };
        let more = client.more(&t)?;
        songs.extend(songs_of(&more));
        token = more.continuation;
    }
    Ok((songs, editable, thumb))
}

/// A short stand-in for a value, to compare against later.
fn digest(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Each side's fingerprint, cheaply: Spotify's revision, and YouTube
/// Music's count and first page of songs. A change on either side, made
/// anywhere, changes its fingerprint.
fn prints(client: &Client, http: &ureq::Agent, cookie: &str, pair: &Pair) -> Result<(String, String), String> {
    let spotify = spotify::fingerprint(http, cookie, &pair.spotify)?;
    let page = client.browse(&format!("VL{}", pair.youtube), None)?;
    let meta = page.header.as_ref().map(|h| h.meta.clone()).unwrap_or_default();
    let ids: Vec<String> = songs_of(&page).into_iter().filter_map(|i| i.video_id).collect();
    Ok((spotify, digest(&format!("{meta}|{}", ids.join(",")))))
}

/// The pairs, by Spotify browse id, whose playlists changed since they
/// last synced. A pair that cannot be checked now is left for the
/// quarter-hourly sync.
pub fn changed(client: &Client, http: &ureq::Agent, cookie: &str) -> Vec<String> {
    pairs()
        .into_iter()
        .filter(|p| prints(client, http, cookie, p).is_ok_and(|now| now != p.prints))
        .map(|p| p.spotify)
        .collect()
}

fn names(track: &Track) -> String {
    track.artists.join(", ")
}

fn item_artists(item: &Item) -> String {
    if item.artists.is_empty() {
        item.subtitle.split(" • ").next().unwrap_or("").to_string()
    } else {
        item.artists.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
    }
}

/// Reads both sides of a pair and keys their songs. Songs are matched to
/// the other service as needed, and the matches are remembered, so a sync
/// cut short carries on where it stopped.
fn read(client: &Client, http: &ureq::Agent, cookie: &str, pair: &Pair) -> Result<Keyed, String> {
    let (list, tracks) = spotify::load(http, cookie, &pair.spotify)?;
    let found = spotify::match_all(client, &tracks, |_| {});
    let (items, youtube_writable, (youtube_title, youtube_thumb)) = youtube_songs(client, &pair.youtube)?;
    let token = crate::spotify_token::web_token(http, cookie)?;
    let mut k = Keyed {
        spotify_writable: list.editable,
        youtube_writable,
        name: if pair.liked() { "Liked Songs".into() } else { list.name.clone() },
        thumb: if list.thumb.is_empty() { youtube_thumb.clone() } else { list.thumb.clone() },
        spotify_thumb: if pair.liked() { spotify::LIKED_COVER.into() } else { list.thumb.clone() },
        youtube_title: if pair.youtube == LIKED_MUSIC { "Liked Music".into() } else { youtube_title },
        youtube_thumb,
        ..Keyed::default()
    };
    // The Spotify song each matched video stands for in this playlist.
    let mut by_video: HashMap<String, String> = HashMap::new();
    for (track, found) in tracks.iter().zip(&found) {
        let key = format!("sp:{}", track.id);
        let video = found.as_ref().and_then(|f| f.video_id.clone());
        match &video {
            Some(v) => {
                by_video.entry(v.clone()).or_insert_with(|| key.clone());
            }
            None => {
                k.only_spotify.insert(key.clone());
            }
        }
        k.songs.entry(key.clone()).or_insert_with(|| Song {
            key: key.clone(),
            title: track.title.clone(),
            artists: names(track),
            video_id: video,
            track_id: Some(track.id.clone()),
        });
        if !track.uid.is_empty() {
            k.uids.entry(key.clone()).or_default().push(track.uid.clone());
        }
        k.spotify.push(key);
    }
    for item in &items {
        let Some(video) = item.video_id.clone() else { continue };
        // The Spotify playlist's own song first, under another video or
        // version, so neither side gets a second copy of it; then Spotify.
        let same = || spotify::same_song_in(&tracks, item).map(|n| format!("sp:{}", tracks[n].id));
        let key = match by_video.get(&video).cloned().or_else(same) {
            Some(key) => key,
            None => match spotify::find_on_spotify(http, &token, item)? {
                Some(track) => {
                    let key = format!("sp:{}", track.id);
                    let song = k.songs.entry(key.clone()).or_insert_with(|| Song {
                        key: key.clone(),
                        title: track.title.clone(),
                        artists: names(&track),
                        video_id: None,
                        track_id: Some(track.id.clone()),
                    });
                    song.video_id.get_or_insert_with(|| video.clone());
                    key
                }
                None => {
                    let key = format!("yt:{video}");
                    k.only_youtube.insert(key.clone());
                    k.songs.entry(key.clone()).or_insert_with(|| Song {
                        key: key.clone(),
                        title: item.title.clone(),
                        artists: item_artists(item),
                        video_id: Some(video.clone()),
                        track_id: None,
                    });
                    key
                }
            },
        };
        k.places
            .entry(key.clone())
            .or_default()
            .push((video.clone(), item.set_video_id.clone().unwrap_or_default()));
        k.youtube.push(key);
    }
    spotify::save_matches();
    Ok(k)
}

// ---------------------------------------------------------------- writing

fn spotify_playlist(pair: &Pair) -> Option<&str> {
    pair.spotify.strip_prefix(spotify::PREFIX)?.strip_prefix("playlist:")
}

/// Carries out a plan on both services. Removals are left out when the
/// plan holds them. Gives what was done, for the log.
fn apply(client: &Client, http: &ureq::Agent, cookie: &str, pair: &Pair, k: &Keyed, plan: &Plan) -> Result<Change, String> {
    let song = |key: &String| k.songs.get(key).cloned().unwrap_or(Song { key: key.clone(), ..Song::default() });
    let mut change = Change { at: unix_now(), ..Change::default() };
    // Adds first: a sync cut short then has lost nothing.
    let to_youtube: Vec<Song> = plan.add_to_b.iter().map(song).filter(|s| s.video_id.is_some()).collect();
    if !to_youtube.is_empty() {
        let videos: Vec<String> = to_youtube.iter().filter_map(|s| s.video_id.clone()).collect();
        if pair.youtube == LIKED_MUSIC {
            for v in &videos {
                client.rate(v, true)?;
            }
        } else {
            client.add_many(&pair.youtube, &videos, |_| {})?;
        }
        change.to_youtube = to_youtube;
    }
    let to_spotify: Vec<Song> = plan.add_to_a.iter().map(song).filter(|s| s.track_id.is_some()).collect();
    if !to_spotify.is_empty() {
        let ids: Vec<String> = to_spotify.iter().filter_map(|s| s.track_id.clone()).collect();
        match spotify_playlist(pair) {
            Some(id) => spotify::add_tracks(http, cookie, id, &ids)?,
            None => spotify::like_tracks(http, cookie, &ids, true)?,
        }
        change.to_spotify = to_spotify;
    }
    if plan.hold_removals {
        return Ok(change);
    }
    if !plan.remove_from_b.is_empty() {
        for key in &plan.remove_from_b {
            for (video, place) in k.places.get(key).into_iter().flatten() {
                if pair.youtube == LIKED_MUSIC {
                    client.rate(video, false)?;
                } else {
                    client.remove_from_playlist(&pair.youtube, video, place)?;
                }
            }
        }
        change.from_youtube = plan.remove_from_b.iter().map(song).collect();
    }
    if !plan.remove_from_a.is_empty() {
        match spotify_playlist(pair) {
            Some(id) => {
                let uids: Vec<String> = plan.remove_from_a.iter().flat_map(|key| k.uids.get(key).cloned().unwrap_or_default()).collect();
                spotify::remove_tracks(http, cookie, id, &uids)?;
            }
            None => {
                let ids: Vec<String> = plan.remove_from_a.iter().filter_map(|key| song(key).track_id).collect();
                spotify::like_tracks(http, cookie, &ids, false)?;
            }
        }
        change.from_spotify = plan.remove_from_a.iter().map(song).collect();
    }
    Ok(change)
}

/// The songs only one side can have, each once.
fn only(k: &Keyed, keys: &[String], only: &HashSet<String>) -> Vec<Song> {
    let mut seen = HashSet::new();
    keys.iter()
        .filter(|key| only.contains(*key) && seen.insert(*key))
        .filter_map(|key| k.songs.get(key).cloned())
        .collect()
}

fn log_change(p: &mut Pair, change: Change) {
    if change.is_empty() {
        return;
    }
    p.log.push(change);
    let extra = p.log.len().saturating_sub(KEEP_LOG);
    p.log.drain(..extra);
}

/// What a sync did, for the message after it.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub name: String,
    pub change: Change,
    /// Removals held for the user, now or still from before.
    pub held: usize,
    /// The held removals are new since the last sync, so worth a message.
    pub newly_held: bool,
    pub first: bool,
    /// Songs one side has that the other cannot.
    pub missing: usize,
}

/// Syncs one pair, by its Spotify browse id.
pub fn run(client: &Client, http: &ureq::Agent, cookie: &str, spotify_id: &str) -> Result<Report, String> {
    let pair = pair_of(spotify_id).ok_or("That playlist is not synced")?;
    let k = read(client, http, cookie, &pair)?;
    let a = Side { songs: &k.spotify, writable: k.spotify_writable, only_here: &k.only_spotify };
    let b = Side { songs: &k.youtube, writable: k.youtube_writable, only_here: &k.only_youtube };
    // A side that can take songs now but could not last time took none
    // of what it lacked: start again, which only adds.
    let newly_writable = (k.spotify_writable && !pair.writable.0) || (k.youtube_writable && !pair.writable.1);
    let base = pair
        .base
        .as_ref()
        .filter(|_| !newly_writable)
        .map(|(x, y)| (x.as_slice(), y.as_slice()));
    let plan = sync::plan(&a, &b, base);
    let change = if plan.is_empty() { Change::default() } else { apply(client, http, cookie, &pair, &k, &plan)? };
    let next = sync::after(&k.spotify, &k.youtube, &plan);
    let song = |key: &String| k.songs.get(key).cloned().unwrap_or(Song { key: key.clone(), ..Song::default() });
    let held = if plan.hold_removals {
        (plan.remove_from_a.iter().map(song).collect(), plan.remove_from_b.iter().map(song).collect())
    } else {
        (Vec::new(), Vec::new())
    };
    // The same removals held again are not news: the menu still has them.
    let newly_held = (!held.0.is_empty() || !held.1.is_empty()) && held != pair.held;
    let report = Report {
        name: k.name.clone(),
        change: change.clone(),
        held: held.0.len() + held.1.len(),
        newly_held,
        first: base.is_none(),
        missing: k.only_spotify.len() + k.only_youtube.len(),
    };
    // As both sides are now, edits included, so only a later change
    // shows as one.
    let fresh = prints(client, http, cookie, &pair).unwrap_or_default();
    update(spotify_id, |p| {
        p.prints = fresh;
        p.base = Some(next);
        p.held = held;
        p.not_on_youtube = only(&k, &k.spotify, &k.only_spotify);
        p.not_on_spotify = only(&k, &k.youtube, &k.only_youtube);
        p.name = k.name.clone();
        p.last = unix_now();
        p.writable = (k.spotify_writable, k.youtube_writable);
        p.thumb = k.thumb.clone();
        p.spotify_thumb = k.spotify_thumb.clone();
        p.youtube_title = k.youtube_title.clone();
        p.youtube_thumb = k.youtube_thumb.clone();
        log_change(p, change);
    });
    Ok(report)
}

/// What the first sync of these two playlists would do, without pairing
/// them or changing either. For `--probe spsync`.
pub fn preview(client: &Client, http: &ureq::Agent, cookie: &str, spotify_id: &str, youtube_id: &str) -> Result<String, String> {
    let pair = Pair {
        spotify: spotify_id.to_string(),
        youtube: youtube_id.strip_prefix("VL").unwrap_or(youtube_id).to_string(),
        ..Pair::default()
    };
    let k = read(client, http, cookie, &pair)?;
    let a = Side { songs: &k.spotify, writable: k.spotify_writable, only_here: &k.only_spotify };
    let b = Side { songs: &k.youtube, writable: k.youtube_writable, only_here: &k.only_youtube };
    let plan = sync::plan(&a, &b, None);
    let name = |key: &String| k.songs.get(key).map_or(key.clone(), |s| format!("{} | {}", s.title, s.artists));
    let mut out = format!(
        "{}: Spotify {} songs (writable {}), YouTube Music {} songs (writable {})\n",
        k.name,
        k.spotify.len(),
        k.spotify_writable,
        k.youtube.len(),
        k.youtube_writable
    );
    let shared = k.spotify.iter().filter(|key| k.youtube.contains(key)).count();
    out += &format!("on both: {shared}, only Spotify can have: {}, only YouTube Music can have: {}\n", k.only_spotify.len(), k.only_youtube.len());
    for (title, list) in [("add to YouTube Music", &plan.add_to_b), ("add to Spotify", &plan.add_to_a)] {
        out += &format!("{title}: {}\n", list.len());
        for key in list.iter().take(15) {
            out += &format!("  {}\n", name(key));
        }
    }
    for (title, list) in [("not on YouTube Music", &k.only_spotify), ("not on Spotify", &k.only_youtube)] {
        out += &format!("{title}:\n");
        for key in list.iter().take(15) {
            out += &format!("  {}\n", name(key));
        }
    }
    Ok(out)
}

fn keys(list: &[Song]) -> Vec<String> {
    list.iter().map(|s| s.key.clone()).collect()
}

/// Makes the removals held for the user. Gives how many songs went.
pub fn remove_held(client: &Client, http: &ureq::Agent, cookie: &str, spotify_id: &str) -> Result<usize, String> {
    let pair = pair_of(spotify_id).ok_or("That playlist is not synced")?;
    let k = read(client, http, cookie, &pair)?;
    let (from_a, from_b) = (keys(&pair.held.0), keys(&pair.held.1));
    let plan = Plan {
        remove_from_a: from_a.iter().filter(|key| k.spotify.contains(key)).cloned().collect(),
        remove_from_b: from_b.iter().filter(|key| k.youtube.contains(key)).cloned().collect(),
        ..Plan::default()
    };
    let change = apply(client, http, cookie, &pair, &k, &plan)?;
    let gone = plan.remove_from_a.len() + plan.remove_from_b.len();
    update(spotify_id, |p| {
        if let Some((a, b)) = p.base.take() {
            p.base = Some(sync::agreed((&a, &b), &from_a, &from_b));
        }
        p.held = (Vec::new(), Vec::new());
        log_change(p, change);
    });
    Ok(gone)
}

/// Keeps the songs held for removal: they stay where they are, and are
/// not asked about again.
pub fn keep_held(spotify_id: &str) {
    update(spotify_id, |p| {
        let (from_a, from_b) = (keys(&p.held.0), keys(&p.held.1));
        if let Some((a, b)) = p.base.take() {
            p.base = Some(sync::kept((&a, &b), &from_a, &from_b));
        }
        p.held = (Vec::new(), Vec::new());
    });
}

/// Undoes a pair's latest sync: takes out what it added and puts back
/// what it removed, at the end. Gives how many songs changed.
pub fn undo(client: &Client, http: &ureq::Agent, cookie: &str, spotify_id: &str) -> Result<usize, String> {
    let pair = pair_of(spotify_id).ok_or("That playlist is not synced")?;
    let last = pair.log.last().cloned().ok_or("Nothing to undo")?;
    let mut k = read(client, http, cookie, &pair)?;
    // What was removed is not in either list now: name it from the log.
    for s in last.from_spotify.iter().chain(&last.from_youtube) {
        k.songs.entry(s.key.clone()).or_insert_with(|| s.clone());
    }
    let plan = Plan {
        add_to_a: keys(&last.from_spotify).into_iter().filter(|key| !k.spotify.contains(key)).collect(),
        add_to_b: keys(&last.from_youtube).into_iter().filter(|key| !k.youtube.contains(key)).collect(),
        remove_from_a: keys(&last.to_spotify).into_iter().filter(|key| k.spotify.contains(key)).collect(),
        remove_from_b: keys(&last.to_youtube).into_iter().filter(|key| k.youtube.contains(key)).collect(),
        hold_removals: false,
    };
    apply(client, http, cookie, &pair, &k, &plan)?;
    let changed = plan.add_to_a.len() + plan.add_to_b.len() + plan.remove_from_a.len() + plan.remove_from_b.len();
    // As both sides are now, so the next sync starts from here and does
    // not do it all again.
    let next = sync::after(&k.spotify, &k.youtube, &plan);
    update(spotify_id, |p| {
        p.base = Some(next);
        p.held = (Vec::new(), Vec::new());
        p.log.pop();
    });
    Ok(changed)
}

// ------------------------------------------------------------------ pages

fn row(s: &Song) -> Item {
    Item {
        kind: Kind::Song,
        title: s.title.clone(),
        subtitle: s.artists.clone(),
        video_id: s.video_id.clone(),
        artists: s.artists.split(", ").filter(|a| !a.is_empty()).map(|a| (a.to_string(), None)).collect(),
        ..Item::default()
    }
}

fn shelf(title: &str, songs: &[Song]) -> Option<Shelf> {
    (!songs.is_empty()).then(|| Shelf {
        title: title.into(),
        items: songs.iter().map(row).collect(),
        ..Shelf::default()
    })
}

/// When a pair last synced, short, for the chip on its line.
pub fn when(at: u64) -> String {
    let secs = unix_now().saturating_sub(at);
    match secs {
        _ if at == 0 => "Not synced yet".into(),
        0..=59 => "Just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// One side of a pair as the Syncs page shows it: its title, service
/// name, badge number (1 YouTube Music, 2 Spotify), cover, and the browse
/// id that opens it.
pub struct SideView {
    pub title: String,
    pub label: &'static str,
    pub service: i32,
    pub thumb: String,
    pub browse_id: String,
}

impl Pair {
    /// The side it was made from, then the side it syncs to.
    pub fn sides(&self) -> (SideView, SideView) {
        let name = if self.name.is_empty() { "Untitled".to_string() } else { self.name.clone() };
        let spotify = SideView {
            title: name.clone(),
            label: "Spotify",
            service: 2,
            thumb: if self.liked() {
                spotify::LIKED_COVER.into()
            } else if self.spotify_thumb.is_empty() {
                self.thumb.clone()
            } else {
                self.spotify_thumb.clone()
            },
            browse_id: self.spotify.clone(),
        };
        let youtube = SideView {
            title: if self.youtube_title.is_empty() { name } else { self.youtube_title.clone() },
            label: "YouTube Music",
            service: 1,
            thumb: if self.youtube_thumb.is_empty() { self.thumb.clone() } else { self.youtube_thumb.clone() },
            browse_id: format!("VL{}", self.youtube),
        };
        if self.from_spotify { (spotify, youtube) } else { (youtube, spotify) }
    }
}

/// The Syncs page: its title only. The window draws the pairs itself, as
/// rows with a line from each source to its destination.
fn overview() -> Page {
    Page { title: "Syncs".into(), ..Page::default() }
}

/// The Syncs page's browse id.
pub const ALL: &str = "tonearm:sync:all";

/// A page of a pair's: the songs one side cannot have, or the removals
/// held for the user. Read from the pairs on disk, so it opens at once.
pub fn page(id: &str) -> Result<Page, String> {
    if id == ALL {
        return Ok(overview());
    }
    let (spotify_id, review) = match (id.strip_prefix(MISSING), id.strip_prefix(REVIEW)) {
        (Some(rest), _) => (rest, false),
        (_, Some(rest)) => (rest, true),
        _ => return Err("Tonearm cannot open that page".into()),
    };
    let pair = pair_of(spotify_id).ok_or("That playlist is no longer synced")?;
    let (title, about, shelves) = if review {
        (
            format!("Songs to remove from {}", pair.name),
            "These were taken out of one playlist since the last sync, which is more than sync removes without asking. \
             Open the menu (•••) to remove them from the other playlist too, or to keep them."
                .to_string(),
            [shelf("From Spotify", &pair.held.0), shelf("From YouTube Music", &pair.held.1)],
        )
    } else {
        (
            format!("Songs not synced in {}", pair.name),
            "Each of these is on one service only: the other has no match for it.".to_string(),
            [shelf("Not on YouTube Music", &pair.not_on_youtube), shelf("Not on Spotify", &pair.not_on_spotify)],
        )
    };
    let shelves: Vec<Shelf> = shelves.into_iter().flatten().collect();
    let count: usize = shelves.iter().map(|s| s.items.len()).sum();
    let header = Header {
        kind: PageKind::Playlist,
        label: "Sync".into(),
        title: title.clone(),
        byline: pair.name.clone(),
        meta: format!("{count} song{}", if count == 1 { "" } else { "s" }),
        about,
        ..Header::default()
    };
    Ok(Page { title, shelves, header: Some(header), ..Page::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_survives_json() {
        let song = Song { key: "sp:1".into(), title: "Uprising".into(), artists: "Muse".into(), video_id: Some("v".into()), track_id: Some("1".into()) };
        let pair = Pair {
            spotify: "spotify:playlist:abc".into(),
            youtube: "PL123".into(),
            name: "Mine".into(),
            base: Some((vec!["sp:1".into()], vec!["sp:1".into(), "yt:x".into()])),
            held: (vec![song.clone()], Vec::new()),
            log: vec![Change { at: 5, to_youtube: vec![song.clone()], ..Change::default() }],
            not_on_youtube: Vec::new(),
            not_on_spotify: vec![Song { key: "yt:x".into(), ..Song::default() }],
            last: 9,
            writable: (true, false),
            from_spotify: true,
            thumb: "t".into(),
            spotify_thumb: "s".into(),
            youtube_title: "Mine".into(),
            youtube_thumb: "y".into(),
            prints: ("a".into(), "b".into()),
        };
        assert_eq!(pair_from(&pair_json(&pair)), Some(pair));
    }

    #[test]
    fn a_pair_is_found_by_either_playlist() {
        let pair = Pair { spotify: "spotify:playlist:abc".into(), youtube: "PL123".into(), ..Pair::default() };
        assert!(pair.has("spotify:playlist:abc"));
        assert!(pair.has("VLPL123"));
        assert!(pair.has("PL123"));
        assert!(!pair.has("VLPL999"));
    }
}
