//! Songs kept on this computer.
//!
//! Two folders under the app's local data folder:
//!
//! - `cache` holds every track that played to the end of its download, and
//!   the next track in the queue, fetched ahead. Playing a track again starts
//!   at once and needs no yt-dlp or network. The oldest played go first once
//!   the cache passes its size limit.
//! - `downloads` holds the songs you chose to keep. They are never evicted,
//!   carry their details and cover in `index.json`, and play with no
//!   connection at all.

use crate::innertube::model::{Item, Kind, Page, Shelf};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Cache size when the user has not picked one.
pub const DEFAULT_CACHE_MB: u64 = 1024;
/// The sizes Settings offers, in MB. Zero turns the cache off.
pub const CACHE_CHOICES: [u64; 5] = [0, 512, 1024, 2048, 5120];

const AUDIO: &str = "m4a";
const COVER: &str = "jpg";
const COVER_PX: u32 = 544;

struct Saved {
    item: Item,
    /// Seconds, from the stream. Zero when unknown.
    seconds: f32,
    /// When it was downloaded, in Unix seconds.
    at: u64,
}

#[derive(Default)]
struct State {
    loaded: bool,
    downloads: HashMap<String, Saved>,
    /// Being downloaded right now.
    busy: HashSet<String>,
    /// How far each running download has got, from 0 to 1.
    progress: HashMap<String, f32>,
    cache_mb: Option<u64>,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    let mut state = STATE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if !state.loaded {
        state.loaded = true;
        load_index(&mut state);
        state.cache_mb = load_cache_limit();
    }
    state
}

fn root() -> Option<PathBuf> {
    crate::store::data_dir()
}

fn cache_dir() -> Option<PathBuf> {
    Some(root()?.join("cache"))
}

fn downloads_dir() -> Option<PathBuf> {
    Some(root()?.join("downloads"))
}

/// Video ids are short and URL safe. Anything else never reaches the disk.
fn safe(video_id: &str) -> bool {
    !video_id.is_empty()
        && video_id.len() <= 64
        && video_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn cache_file(video_id: &str) -> Option<PathBuf> {
    safe(video_id).then(|| Some(cache_dir()?.join(format!("{video_id}.{AUDIO}"))))?
}

fn download_file(video_id: &str) -> Option<PathBuf> {
    safe(video_id).then(|| Some(downloads_dir()?.join(format!("{video_id}.{AUDIO}"))))?
}

fn cover_file(video_id: &str) -> Option<PathBuf> {
    safe(video_id).then(|| Some(downloads_dir()?.join(format!("{video_id}.{COVER}"))))?
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Writes through a `.part` file, so a crash never leaves half a track
/// that looks whole.
fn write_atomic(path: &PathBuf, bytes: &[u8]) -> Result<(), String> {
    let folder = path.parent().ok_or("No local data folder")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let partial = path.with_extension("part");
    std::fs::write(&partial, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, path).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ lookup

/// The file to play instead of streaming: a download, or a cached copy.
/// A cache hit counts as a play, so it is the last to be evicted.
pub fn local_file(video_id: &str) -> Option<PathBuf> {
    if is_downloaded(video_id) {
        if let Some(path) = download_file(video_id).filter(|p| p.is_file()) {
            return Some(path);
        }
    }
    let path = cache_file(video_id).filter(|p| p.is_file())?;
    if let Ok(file) = std::fs::File::options().write(true).open(&path) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(path)
}

pub fn is_downloaded(video_id: &str) -> bool {
    state().downloads.contains_key(video_id)
}

/// How far a download has got, from 0 to 1. Zero while it waits its turn.
pub fn progress(video_id: &str) -> f32 {
    state().progress.get(video_id).copied().unwrap_or(0.0)
}

pub fn is_downloading(video_id: &str) -> bool {
    state().busy.contains(video_id)
}

/// Downloaded songs, newest first.
pub fn downloads() -> Vec<Item> {
    let state = state();
    let mut saved: Vec<&Saved> = state.downloads.values().collect();
    saved.sort_by(|a, b| b.at.cmp(&a.at));
    saved
        .into_iter()
        .map(|s| {
            let mut item = s.item.clone();
            if let Some(cover) = item.video_id.as_deref().and_then(cover_file) {
                if cover.is_file() {
                    item.thumb = format!("file:{}", cover.display());
                }
            }
            if item.duration.is_empty() && s.seconds > 0.0 {
                let total = s.seconds as u32;
                item.duration = format!("{}:{:02}", total / 60, total % 60);
            }
            item
        })
        .collect()
}

/// The Downloads page. Built from disk, so it opens with no connection.
pub fn downloads_page() -> Page {
    let items = downloads();
    let count = items.len();
    Page {
        title: "Downloads".into(),
        subtitle: match count {
            0 => "Songs you download play here, even offline".into(),
            1 => "1 song".into(),
            n => format!("{n} songs"),
        },
        thumb: String::new(),
        shelves: if items.is_empty() {
            Vec::new()
        } else {
            vec![Shelf {
                title: String::new(),
                carousel: false,
                items,
                more: None,
            }]
        },
        continuation: None,
        header: None,
    }
}

// ------------------------------------------------------------------- cache

pub fn cache_limit_mb() -> u64 {
    state().cache_mb.unwrap_or(DEFAULT_CACHE_MB)
}

fn limit_file() -> Option<PathBuf> {
    Some(root()?.join("cache-limit.txt"))
}

fn load_cache_limit() -> Option<u64> {
    std::fs::read_to_string(limit_file()?).ok()?.trim().parse().ok()
}

pub fn set_cache_limit_mb(mb: u64) {
    state().cache_mb = Some(mb);
    if let Some(path) = limit_file() {
        let _ = write_atomic(&path, mb.to_string().as_bytes());
    }
    trim_cache();
}

/// Keeps a fully fetched track for next time. Downloads already have it.
pub fn store_cache(video_id: &str, bytes: &[u8]) {
    if bytes.is_empty() || cache_limit_mb() == 0 || is_downloaded(video_id) {
        return;
    }
    let Some(path) = cache_file(video_id) else {
        return;
    };
    if path.is_file() {
        return;
    }
    if write_atomic(&path, bytes).is_ok() {
        trim_cache();
    }
}

/// Whether a track can play from disk, without counting it as a play.
pub fn has_local(video_id: &str) -> bool {
    (is_downloaded(video_id) && download_file(video_id).is_some_and(|p| p.is_file()))
        || cache_file(video_id).is_some_and(|p| p.is_file())
}

/// Throws away a cached file that would not play, so the next play
/// streams it again instead of failing the same way. Downloads are kept.
pub fn discard_broken(path: &std::path::Path) {
    if cache_dir().is_some_and(|dir| path.starts_with(dir)) {
        let _ = std::fs::remove_file(path);
    }
}

/// Files in a folder with their size and last play, oldest first.
fn listing(folder: Option<PathBuf>, extension: &str) -> Vec<(PathBuf, u64, SystemTime)> {
    let Some(entries) = folder.and_then(|f| std::fs::read_dir(f).ok()) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != extension {
                return None;
            }
            let meta = e.metadata().ok()?;
            Some((path, meta.len(), meta.modified().unwrap_or(UNIX_EPOCH)))
        })
        .collect();
    files.sort_by_key(|f| f.2);
    files
}

/// Drops the least recently played tracks until the cache fits its limit.
fn trim_cache() {
    let limit = cache_limit_mb() * 1024 * 1024;
    let files = listing(cache_dir(), AUDIO);
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    for (path, size, _) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

pub fn clear_cache() {
    for (path, _, _) in listing(cache_dir(), AUDIO) {
        let _ = std::fs::remove_file(path);
    }
}

/// Bytes used by the cache and by downloads.
pub fn usage() -> (u64, u64) {
    let sum = |folder, ext| listing(folder, ext).iter().map(|f| f.1).sum::<u64>();
    (
        sum(cache_dir(), AUDIO),
        sum(downloads_dir(), AUDIO) + sum(downloads_dir(), COVER),
    )
}

pub fn size_text(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

// --------------------------------------------------------------- downloads

fn index_file() -> Option<PathBuf> {
    Some(downloads_dir()?.join("index.json"))
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Song => "song",
        Kind::Album => "album",
        Kind::Artist => "artist",
        Kind::Playlist => "playlist",
        Kind::Other => "other",
    }
}

fn kind_from(name: &str) -> Kind {
    match name {
        "album" => Kind::Album,
        "artist" => Kind::Artist,
        "playlist" => Kind::Playlist,
        "other" => Kind::Other,
        _ => Kind::Song,
    }
}

fn load_index(state: &mut State) {
    let Some(text) = index_file().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let s = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
    for e in v["songs"].as_array().into_iter().flatten() {
        let Some(id) = s(e, "video") else { continue };
        // A song whose file went missing is no longer downloaded.
        if !download_file(&id).is_some_and(|p| p.is_file()) {
            continue;
        }
        let item = Item {
            kind: kind_from(&s(e, "kind").unwrap_or_default()),
            title: s(e, "title").unwrap_or_default(),
            subtitle: s(e, "subtitle").unwrap_or_default(),
            thumb: s(e, "thumb").unwrap_or_default(),
            duration: s(e, "duration").unwrap_or_default(),
            video_id: Some(id.clone()),
            playlist_id: s(e, "playlist"),
            browse_id: s(e, "browse"),
            params: s(e, "params"),
            artists: Item::artists_from_json(e.get("artist_links")),
            album: Item::album_from_json(e.get("album")),
            ..Item::default()
        };
        state.downloads.insert(
            id,
            Saved {
                item,
                seconds: e["seconds"].as_f64().unwrap_or(0.0) as f32,
                at: e["at"].as_u64().unwrap_or(0),
            },
        );
    }
}

fn save_index(state: &State) {
    let songs: Vec<Value> = state
        .downloads
        .values()
        .map(|s| {
            let i = &s.item;
            let (artist_links, album) = i.links_json();
            json!({
                "kind": kind_name(i.kind),
                "title": i.title,
                "subtitle": i.subtitle,
                "thumb": i.thumb,
                "duration": i.duration,
                "video": i.video_id,
                "playlist": i.playlist_id,
                "browse": i.browse_id,
                "params": i.params,
                "artist_links": artist_links,
                "album": album,
                "seconds": s.seconds,
                "at": s.at,
            })
        })
        .collect();
    if let Some(path) = index_file() {
        let _ = write_atomic(&path, json!({ "songs": songs }).to_string().as_bytes());
    }
}

/// Fetches the cover at a size that still looks sharp in the player.
fn save_cover(http: &ureq::Agent, item: &Item, video_id: &str) {
    let Some(path) = cover_file(video_id) else {
        return;
    };
    if item.thumb.is_empty() || item.thumb.starts_with("file:") {
        return;
    }
    let url = if item.thumb.starts_with("//") {
        format!("https:{}", item.thumb)
    } else {
        crate::images::sized(&item.thumb, COVER_PX)
    };
    let Ok(response) = http
        .get(&url)
        .set("User-Agent", crate::innertube::USER_AGENT)
        .call()
    else {
        return;
    };
    let mut bytes = Vec::new();
    use std::io::Read;
    if response
        .into_reader()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .is_ok()
        && !bytes.is_empty()
    {
        let _ = write_atomic(&path, &bytes);
    }
}

/// Downloads one song. Blocks, so run it on a worker. A cached copy is
/// moved over instead of fetched again.
pub fn download(
    resolver: Option<&dyn crate::resolver::Resolver>,
    http: &ureq::Agent,
    item: &Item,
) -> Result<(), String> {
    let video_id = item.video_id.as_deref().ok_or("That is not a song")?;
    let target = download_file(video_id).ok_or("No local data folder")?;
    let mut seconds = 0.0;
    let cached = cache_file(video_id).filter(|p| p.is_file());
    let moved = cached.is_some_and(|from| {
        std::fs::create_dir_all(target.parent().unwrap()).is_ok()
            && std::fs::rename(&from, &target).is_ok()
    });
    if !moved {
        let resolver = resolver.ok_or("Playback is not set up yet")?;
        let stream = resolver.resolve(video_id)?;
        seconds = stream.duration;
        // Shows on the song's row, every couple of percent.
        let id = video_id.to_string();
        let shown = Mutex::new(0.0f32);
        let report = move |so_far: u64, whole: u64| {
            let done = (so_far as f32 / whole.max(1) as f32).min(1.0);
            let mut last = shown.lock().unwrap();
            if done - *last >= 0.02 || done >= 1.0 {
                *last = done;
                state().progress.insert(id.clone(), done);
                let id = id.clone();
                crate::on_ui(move |app| app.show_download_progress(&id));
            }
        };
        let bytes =
            crate::audio::fetch_all_with(&stream.url, &stream.headers, Some(Box::new(report)))?;
        write_atomic(&target, &bytes).map_err(|e| format!("Could not save the song: {e}"))?;
    }
    save_cover(http, item, video_id);
    let mut state = state();
    let mut item = item.clone();
    item.kind = Kind::Song;
    state.downloads.insert(
        video_id.to_string(),
        Saved {
            item,
            seconds,
            at: now(),
        },
    );
    save_index(&state);
    Ok(())
}

/// Albums with downloads saved before album tracks named their artist.
/// Those songs carry a play count where the artist belongs.
fn uncredited_albums() -> Vec<String> {
    let state = state();
    let mut albums: Vec<String> = state
        .downloads
        .values()
        .filter(|s| s.item.artists.is_empty())
        .filter_map(|s| s.item.album.as_ref().map(|(_, id)| id.clone()))
        .filter(|id| id.starts_with("MPREb_"))
        .collect();
    albums.sort();
    albums.dedup();
    albums
}

/// Takes the artist and subtitle of fresh tracks into saved songs that
/// have no artist. True when any changed.
fn credit(tracks: &[Item]) -> bool {
    let mut state = state();
    let mut changed = false;
    for track in tracks.iter().filter(|t| !t.artists.is_empty()) {
        let Some(saved) = track.video_id.as_deref().and_then(|id| state.downloads.get_mut(id)) else {
            continue;
        };
        if saved.item.artists.is_empty() {
            saved.item.artists = track.artists.clone();
            saved.item.subtitle = track.subtitle.clone();
            changed = true;
        }
    }
    if changed {
        save_index(&state);
    }
    changed
}

fn remove(video_id: &str) {
    let mut state = state();
    if state.downloads.remove(video_id).is_none() {
        return;
    }
    save_index(&state);
    drop(state);
    if let Some(path) = download_file(video_id) {
        let _ = std::fs::remove_file(path);
    }
    if let Some(path) = cover_file(video_id) {
        let _ = std::fs::remove_file(path);
    }
}

/// One download at a time, in the order asked, so a whole album does not
/// start forty yt-dlp processes.
fn workers() -> &'static Mutex<crate::pool::Pool> {
    static POOL: OnceLock<Mutex<crate::pool::Pool>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(crate::pool::Pool::new("save", 1)))
}

// --------------------------------------------------------------------- app

impl crate::App {
    /// Downloads songs in the background. Songs already saved or on their
    /// way are skipped.
    pub fn download_items(self: &Rc<Self>, items: Vec<Item>) {
        let fresh: Vec<Item> = {
            let mut state = state();
            items
                .into_iter()
                .filter(|i| {
                    i.video_id.as_deref().is_some_and(|id| {
                        safe(id) && !state.downloads.contains_key(id) && state.busy.insert(id.into())
                    })
                })
                .collect()
        };
        if fresh.is_empty() {
            self.toast("Already downloaded");
            return;
        }
        let total = fresh.len();
        self.toast(if total == 1 {
            format!("Downloading {}", fresh[0].title)
        } else {
            format!("Downloading {total} songs")
        });
        self.downloads_changed();
        for (n, item) in fresh.into_iter().enumerate() {
            let resolver = self.resolver.clone();
            let http = self.http.clone();
            workers().lock().unwrap().run(move || {
                let result = download(resolver.as_deref(), &http, &item);
                let video_id = item.video_id.clone().unwrap_or_default();
                {
                    let mut state = state();
                    state.busy.remove(&video_id);
                    state.progress.remove(&video_id);
                }
                crate::on_ui(move |app| {
                    match result {
                        Ok(()) if total == 1 => app.toast(format!("Downloaded {}", item.title)),
                        Ok(()) if n + 1 == total => app.toast(format!("Downloaded {total} songs")),
                        Ok(()) => {}
                        Err(message) => {
                            app.toast(format!("Could not download {}: {message}", item.title))
                        }
                    }
                    app.downloads_changed();
                });
            });
        }
    }

    /// Fills in the artist of downloads saved without one, from their
    /// album pages. Quiet, and a no-op once every song is credited.
    pub fn repair_downloads(self: &Rc<Self>) {
        let albums = uncredited_albums();
        if albums.is_empty() {
            return;
        }
        let client = self.client.clone();
        self.api.run(move || {
            let mut changed = false;
            for id in albums {
                if let Ok(page) = client.browse(&id, None) {
                    let tracks: Vec<Item> =
                        page.shelves.into_iter().flat_map(|s| s.items).collect();
                    changed |= credit(&tracks);
                }
            }
            if changed {
                crate::on_ui(|app| app.downloads_changed());
            }
        });
    }

    /// Downloads a song, or removes it when it is already downloaded.
    pub fn toggle_download(self: &Rc<Self>, item: Item) {
        let Some(video_id) = item.video_id.clone() else {
            return;
        };
        if is_downloaded(&video_id) {
            self.remove_download(&video_id);
        } else if !is_downloading(&video_id) {
            self.download_items(vec![item]);
        }
    }

    pub fn remove_download(self: &Rc<Self>, video_id: &str) {
        let video_id = video_id.to_string();
        let title = state()
            .downloads
            .get(&video_id)
            .map(|s| s.item.title.clone())
            .unwrap_or_default();
        self.disk.run(move || {
            remove(&video_id);
            crate::on_ui(move |app| {
                app.toast(format!("Removed {title} from downloads"));
                app.downloads_changed();
            });
        });
    }

    /// Fetches the whole next track into the cache, so it starts at once
    /// and keeps playing if the connection drops.
    pub fn cache_ahead(self: &Rc<Self>, video_id: String) {
        if cache_limit_mb() == 0 || has_local(&video_id) {
            return;
        }
        let Some(resolver) = self.resolver.clone() else {
            return;
        };
        {
            let mut state = state();
            if !state.busy.insert(video_id.clone()) {
                return;
            }
        }
        self.api.run(move || {
            if let Ok(stream) = resolver.resolve(&video_id) {
                if let Ok(bytes) = crate::audio::fetch_all(&stream.url, &stream.headers) {
                    store_cache(&video_id, &bytes);
                }
            }
            state().busy.remove(&video_id);
        });
    }

    /// Refreshes whatever shows what is downloaded: the Downloads page if
    /// it is open, and the storage line in Settings.
    pub fn downloads_changed(self: &Rc<Self>) {
        self.show_storage();
        self.refresh_downloaded();
        let on_downloads = matches!(self.history.borrow().last(), Some(crate::Route::Downloads))
            && self.ui.get_nav() != 3;
        if on_downloads {
            self.open(crate::Route::Downloads, false);
        }
    }

    /// The storage card in Settings.
    pub fn show_storage(&self) {
        let (cache, saved) = usage();
        self.ui.set_storage_note(
            format!(
                "Cache {} of {} · Downloads {}",
                size_text(cache),
                match cache_limit_mb() {
                    0 => "off".to_string(),
                    mb => size_text(mb * 1024 * 1024),
                },
                size_text(saved),
            )
            .into(),
        );
        let limit = cache_limit_mb();
        let choice = CACHE_CHOICES
            .iter()
            .position(|&c| c == limit)
            .unwrap_or(2);
        self.ui.set_cache_choice(choice as i32);
    }

    pub fn set_cache_choice(self: &Rc<Self>, choice: i32) {
        let mb = CACHE_CHOICES
            .get(choice.max(0) as usize)
            .copied()
            .unwrap_or(DEFAULT_CACHE_MB);
        self.disk.run(move || {
            set_cache_limit_mb(mb);
            crate::on_ui(|app| app.show_storage());
        });
    }

    pub fn clear_cache(self: &Rc<Self>) {
        self.disk.run(|| {
            clear_cache();
            crate::on_ui(|app| {
                app.show_storage();
                app.toast("Cache cleared");
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_ids_reach_the_disk() {
        assert!(safe("dQw4w9WgXcQ"));
        assert!(!safe("../evil"));
        assert!(!safe("a b"));
        assert!(!safe(""));
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(size_text(5 * 1024 * 1024), "5 MB");
        assert_eq!(size_text(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }
}
