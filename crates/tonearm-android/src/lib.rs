//! Tonearm's core for the Android app. UniFFI writes the Kotlin side from
//! what is exported here: one `Tonearm` object that searches YouTube Music
//! and plays a queue of songs through the core's own engine.
//!
//! Calls that reach the network block, so the app makes them off its main
//! thread. Before anything plays, the app calls `TonearmNative.init` with
//! its context (see `android`), which Android's audio needs.
//!
//! The queue moves on by itself when a song ends. The app reads where
//! things stand with `playback`, a few times a second.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tonearm_core::innertube::model::{self, Kind};
use tonearm_core::queue::{Added, Queue, Repeat, Step};
use tonearm_core::{audio, innertube, resolver, store};

mod lyrics;
mod menu;
mod pages;
pub use lyrics::{LyricLine, Lyrics, Syllable};
pub use menu::{MenuAction, MenuIcon, MenuLine};
pub use pages::{Chip, Header, Item, ItemKind, Link, Page, Shelf, ShelfLayout};

uniffi::setup_scaffolding!();

/// A song found, as the app lists it.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct Song {
    pub video_id: String,
    pub title: String,
    /// Its artists, joined with commas.
    pub artist: String,
    pub album: String,
    /// The cover's link, about 544 pixels square.
    pub cover: String,
    /// As YouTube Music shows it, such as "3:12". Empty when unknown.
    pub duration: String,
    /// Each credited artist and their page, for its menu.
    pub artists: Vec<Link>,
    pub album_id: Option<String>,
}

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Debug)]
pub enum PlayState {
    /// Nothing loaded yet.
    Idle,
    /// Finding the song's stream, or the engine opening it.
    Loading,
    Playing,
    Paused,
    /// The queue played out.
    Ended,
    Failed,
}

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Debug)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

/// Where playback stands, for the app to poll.
#[derive(uniffi::Record)]
pub struct Playback {
    pub state: PlayState,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Why it failed, when it did.
    pub error: String,
    /// The song playing's place in the queue.
    pub index: u32,
    /// Changes whenever the queue or the song playing does, so the app
    /// knows to read them again.
    pub version: u64,
    pub repeat: RepeatMode,
    pub shuffle: bool,
}

/// In Kotlin, a `TonearmException` whose message says what went wrong.
#[derive(Debug, uniffi::Error)]
#[uniffi(flat_error)]
pub enum TonearmError {
    Failed { message: String },
}

impl std::fmt::Display for TonearmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TonearmError::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for TonearmError {}

impl From<String> for TonearmError {
    fn from(message: String) -> Self {
        TonearmError::Failed { message }
    }
}

#[derive(uniffi::Object)]
pub struct Tonearm {
    client: innertube::Client,
    resolver: Box<dyn resolver::Resolver>,
    audio: audio::Audio,
    queue: Mutex<Queue<Song>>,
    /// Counts songs loaded, so the engine can tell them apart.
    generation: AtomicU64,
    /// Counts requests to play a song. A request that finds a newer one
    /// when its stream arrives lets it be.
    request: AtomicU64,
    /// A stream is being found.
    loading: AtomicBool,
    /// Why the last song couldn't start. Cleared by the next.
    note: Mutex<String>,
    /// Bumped when the queue or the song playing changes.
    version: AtomicU64,
    /// The queue's radio is being looked for.
    topping: AtomicBool,
    /// The last song whose radio was asked for, so it is asked once.
    topped_for: Mutex<String>,
    /// Lyrics found, by video id, so going back to a song doesn't ask again.
    lyrics: Mutex<HashMap<String, Lyrics>>,
    /// Whether songs are liked, by video id, as last read or set.
    liked: Mutex<HashMap<String, bool>>,
}

#[uniffi::export]
impl Tonearm {
    /// Starts the core, keeping its settings and data under `home`: the
    /// app's own files folder.
    #[uniffi::constructor]
    pub fn new(home: String) -> Result<Arc<Self>, TonearmError> {
        store::set_home(home.into());
        let resolver = resolver::default().ok_or_else(|| "no way to find song links".to_string())?;
        let tonearm = Arc::new(Tonearm {
            client: innertube::Client::new(),
            resolver,
            audio: audio::Audio::new(),
            queue: Mutex::new(Queue::default()),
            generation: AtomicU64::new(0),
            request: AtomicU64::new(0),
            loading: AtomicBool::new(false),
            note: Mutex::new(String::new()),
            version: AtomicU64::new(0),
            topping: AtomicBool::new(false),
            topped_for: Mutex::new(String::new()),
            lyrics: Mutex::new(HashMap::new()),
            liked: Mutex::new(HashMap::new()),
        });
        // Signed in as last time, as the desktop app starts.
        if let Some(cookie) = store::load("session") {
            tonearm.apply(&cookie);
        }
        watch(Arc::downgrade(&tonearm));
        Ok(tonearm)
    }

    /// Signs in with a YouTube Music `Cookie:` header and keeps it for next
    /// time. False when the cookie isn't a signed-in one.
    pub fn sign_in(&self, cookie: String) -> Result<bool, TonearmError> {
        if !self.apply(&cookie) {
            return Ok(false);
        }
        store::save("session", &cookie)?;
        Ok(true)
    }

    pub fn sign_out(&self) {
        store::clear("session");
        self.client.set_cookie(None);
        self.resolver.set_cookie(None);
    }

    pub fn signed_in(&self) -> bool {
        store::load("session").is_some()
    }

    /// Songs matching `query`, best first.
    pub fn search(&self, query: String) -> Result<Vec<Song>, TonearmError> {
        let page = self.client.search(&query)?;
        Ok(page
            .shelves
            .iter()
            .flat_map(|shelf| shelf.items.iter())
            .filter(|item| item.kind == Kind::Song)
            .filter_map(song)
            .collect())
    }

    // ---------------------------------------------------------------- pages

    pub fn home(&self) -> Result<Page, TonearmError> {
        self.browse("FEmusic_home".into(), None)
    }

    /// The signed-in user's library: playlists, albums, artists.
    pub fn library(&self) -> Result<Page, TonearmError> {
        self.browse("FEmusic_library_landing".into(), None)
    }

    /// Any page by its browse id: an album, artist, playlist, a mood.
    pub fn browse(&self, id: String, params: Option<String>) -> Result<Page, TonearmError> {
        Ok(pages::page(&self.client.browse(&id, params.as_deref())?))
    }

    /// The shelves after a page's `continuation`, as it is scrolled.
    pub fn more(&self, continuation: String) -> Result<Page, TonearmError> {
        Ok(pages::page(&self.client.more(&continuation)?))
    }

    /// Everything a search finds: the top result, songs, albums, artists,
    /// playlists, in YouTube Music's shelves.
    pub fn search_page(&self, query: String) -> Result<Page, TonearmError> {
        Ok(pages::page(&self.client.search(&query)?))
    }

    // --------------------------------------------------------------- lyrics

    /// The song's lyrics, synced where any service has them timed. None
    /// when nobody has them. `seconds` is the song's length, which matches
    /// the right recording; zero takes it from the queue.
    pub fn lyrics(&self, video_id: String, title: String, artist: String, seconds: f32) -> Option<Lyrics> {
        if let Some(found) = self.lyrics.lock().unwrap().get(&video_id) {
            return Some(found.clone());
        }
        // The queue's own name for the song wins: the app's can still be
        // the last song's for a moment as the next one comes on, which
        // would find the wrong song's lyrics.
        let queued = {
            let queue = self.queue.lock().unwrap();
            queue.items().iter().find(|s| s.video_id == video_id).cloned()
        };
        let (title, artist) = match &queued {
            Some(song) => (song.title.clone(), song.artist.clone()),
            None => (title, artist),
        };
        let seconds = if seconds > 0.0 {
            seconds
        } else {
            queued.map(|s| lyrics::seconds_of(&s.duration)).unwrap_or(0.0)
        };
        let query = tonearm_core::lyrics::Query {
            video_id: &video_id,
            title: &title,
            artist: &artist,
            seconds,
        };
        let found = Lyrics::from(&tonearm_core::lyrics::fetch(&self.client, &query)?);
        if found.lines.is_empty() {
            return None;
        }
        let mut cache = self.lyrics.lock().unwrap();
        // A few dozen songs is plenty to go back through.
        if cache.len() >= 40 {
            cache.clear();
        }
        cache.insert(video_id, found.clone());
        Some(found)
    }

    // ------------------------------------------------- playing, as on desktop

    /// A song tapped in a list (an album's tracks, a playlist, search's
    /// songs): plays the list's songs, from that one.
    pub fn play_list(&self, items: Vec<Item>, index: u32) -> Result<(), TonearmError> {
        let index = index as usize;
        let start = items.iter().take(index).filter(|i| i.video_id.is_some()).count();
        let songs: Vec<Song> = items
            .iter()
            .map(pages::core_item)
            .filter_map(|i| song(&i))
            .collect();
        self.set_queue(songs, start as u32)
    }

    /// A song card tapped: plays the song, then its radio. A music video
    /// plays the album track it is of, so its radio is songs.
    pub fn play_radio(&self, item: Item) -> Result<(), TonearmError> {
        let core = pages::core_item(&item);
        let core = self.client.song_for(&core).unwrap_or(core);
        let Some(first) = song(&core) else {
            return Err("Nothing to play".to_string().into());
        };
        let id = first.video_id.clone();
        self.set_queue(vec![first], 0)?;
        // The radio comes after the song has started.
        let (items, _) = self.client.queue(Some(&id), None)?;
        let more: Vec<Song> = items.iter().filter_map(song).filter(|s| s.video_id != id).collect();
        let mut queue = self.queue.lock().unwrap();
        if queue.items().len() == 1 && queue.current().is_some_and(|s| s.video_id == id) {
            queue.extend(more);
            drop(queue);
            self.changed();
        }
        Ok(())
    }

    /// A card's play button: an album, playlist or mix plays from its
    /// start, an artist plays the songs on their page, a song its radio.
    pub fn play_item(&self, item: Item) -> Result<(), TonearmError> {
        if item.video_id.is_some() {
            return self.play_radio(item);
        }
        let items = match (&item.playlist_id, &item.browse_id) {
            (Some(playlist), _) => self.client.queue(None, Some(playlist))?.0,
            (None, Some(browse)) => songs_of(&self.client.browse(browse, item.params.as_deref())?),
            (None, None) => Vec::new(),
        };
        let songs: Vec<Song> = items.iter().filter_map(song).collect();
        if songs.is_empty() {
            return Err("Nothing to play".to_string().into());
        }
        self.set_queue(songs, 0)
    }

    /// An artist's shuffle or radio button, as YouTube Music starts them.
    pub fn play_watch(&self, item: Item) -> Result<(), TonearmError> {
        let (items, _) = self.client.queue_with(
            item.video_id.as_deref(),
            item.playlist_id.as_deref(),
            item.params.as_deref(),
        )?;
        let songs: Vec<Song> = items.iter().filter_map(song).collect();
        if songs.is_empty() {
            return Err("Nothing to play".to_string().into());
        }
        self.set_queue(songs, 0)
    }

    /// Plays `songs` from `index`, in place of the queue.
    pub fn set_queue(&self, songs: Vec<Song>, index: u32) -> Result<(), TonearmError> {
        self.queue.lock().unwrap().start(songs, index as usize);
        self.changed();
        self.play_current()
    }

    /// Plays the song at `index` in the queue.
    pub fn skip_to(&self, index: u32) -> Result<(), TonearmError> {
        self.step(Step::Play(index as usize))
    }

    pub fn next(&self) -> Result<(), TonearmError> {
        let step = self.queue.lock().unwrap().next();
        self.step(step)
    }

    pub fn previous(&self) -> Result<(), TonearmError> {
        let position = self.audio.status.position_ms.load(Ordering::SeqCst);
        let step = self.queue.lock().unwrap().previous(position);
        self.step(step)
    }

    /// Moves the song at `from` in the queue to `to`, as a row dragged in
    /// the queue view. The song playing carries on.
    pub fn move_in_queue(&self, from: u32, to: u32) {
        self.queue.lock().unwrap().move_item(from as usize, to as usize);
        self.changed();
    }

    // ------------------------------------------------------ a song's menu

    /// Play next: right after the song playing, as on desktop. Gives what
    /// to tell the user, empty when the song simply starts.
    pub fn play_next(&self, item: Item) -> Result<String, TonearmError> {
        let song = song(&pages::core_item(&item)).ok_or_else(|| "Nothing to play".to_string())?;
        let (id, title) = (song.video_id.clone(), song.title.clone());
        let added = self.queue.lock().unwrap().play_next(song, |s| s.video_id == id);
        self.after_adding(added, format!("{title} plays next"), format!("{title} already plays next"))
    }

    /// Add to queue: at the end, unless it is queued already.
    pub fn add_to_queue(&self, item: Item) -> Result<String, TonearmError> {
        let song = song(&pages::core_item(&item)).ok_or_else(|| "Nothing to add".to_string())?;
        let (id, title) = (song.video_id.clone(), song.title.clone());
        let added = self.queue.lock().unwrap().add(song, |s| s.video_id == id);
        self.after_adding(added, format!("Added {title} to the queue"), String::new())
    }

    /// What an item's long press offers, as the desktop's right click does.
    /// `in_library` is true on the library and its lists, whose artists
    /// are followed.
    pub fn menu(&self, item: Item, in_library: bool) -> Vec<MenuLine> {
        let playing = item.video_id.is_some()
            && self.queue.lock().unwrap().current().map(|s| &s.video_id) == item.video_id.as_ref();
        let liked = playing && item.video_id.as_ref().is_some_and(|id| self.liked.lock().unwrap().get(id) == Some(&true));
        let context = tonearm_core::menu::Context {
            signed_in: self.client.signed_in(),
            playing,
            liked,
            in_library,
            downloads: false,
        };
        menu::lines(&item, &context)
    }

    /// Whether the user has liked a song, for Now Playing's heart. Asks
    /// YouTube Music once per song. None signed out, or when it can't say.
    pub fn is_liked(&self, video_id: String) -> Option<bool> {
        if let Some(known) = self.liked.lock().unwrap().get(&video_id) {
            return Some(*known);
        }
        let liked = self.client.liked(&video_id).ok().flatten()?;
        let mut cache = self.liked.lock().unwrap();
        if cache.len() >= 200 {
            cache.clear();
        }
        // A like set meanwhile wins over what was read.
        Some(*cache.entry(video_id).or_insert(liked))
    }

    /// Likes a song, or takes the like back.
    pub fn set_liked(&self, video_id: String, like: bool) -> Result<(), TonearmError> {
        self.liked.lock().unwrap().insert(video_id.clone(), like);
        let result = self.client.rate(&video_id, like);
        if result.is_err() {
            self.liked.lock().unwrap().remove(&video_id);
        }
        Ok(result?)
    }

    /// Likes a song on YouTube Music.
    pub fn like(&self, video_id: String) -> Result<(), TonearmError> {
        self.set_liked(video_id, true)
    }

    /// An album's or playlist's menu Play or Shuffle: all its songs.
    pub fn play_collection(&self, item: Item, shuffle: bool) -> Result<(), TonearmError> {
        let items = tonearm_core::menu::collection_songs(&self.client, &pages::core_item(&item))?;
        let songs: Vec<Song> = items.iter().filter_map(song).collect();
        if songs.is_empty() {
            return Err("There are no songs in it".to_string().into());
        }
        let songs = if shuffle { tonearm_core::queue::shuffled(songs) } else { songs };
        self.set_queue(songs, 0)
    }

    /// An artist's shuffle or radio from their menu, as the desktop plays
    /// them: their page's own, or their top songs.
    pub fn play_artist(&self, artist_id: String, shuffle: bool) -> Result<(), TonearmError> {
        let items = tonearm_core::menu::artist_songs(&self.client, &artist_id, shuffle)?;
        let songs: Vec<Song> = items.iter().filter_map(song).collect();
        if songs.is_empty() {
            return Err("Nothing to play".to_string().into());
        }
        self.set_queue(songs, 0)
    }

    /// Saves an album or playlist to the library, or takes it out. Gives
    /// what to tell the user.
    pub fn set_saved(&self, playlist_id: String, save: bool, title: String) -> Result<String, TonearmError> {
        self.client.rate_playlist(&playlist_id, save)?;
        Ok(if save {
            format!("Saved {title} to your library")
        } else {
            format!("Removed {title} from your library")
        })
    }

    /// Follows an artist, or stops. Gives what to tell the user.
    pub fn follow(&self, channel_id: String, follow: bool, name: String) -> Result<String, TonearmError> {
        self.client.subscribe(&channel_id, follow)?;
        Ok(if follow { format!("Following {name}") } else { format!("Stopped following {name}") })
    }

    /// The user's playlists songs can be added to: each title, and its
    /// playlist id.
    pub fn my_playlists(&self) -> Result<Vec<Link>, TonearmError> {
        Ok(self
            .client
            .my_playlists()?
            .into_iter()
            .map(|(name, id)| Link { name, id: Some(id) })
            .collect())
    }

    pub fn add_to_playlist(&self, playlist_id: String, video_id: String) -> Result<(), TonearmError> {
        Ok(self.client.add_to_playlist(&playlist_id, &video_id)?)
    }

    pub fn queue(&self) -> Vec<Song> {
        self.queue.lock().unwrap().items().to_vec()
    }

    /// The song playing, or the one the queue is on.
    pub fn now_playing(&self) -> Option<Song> {
        self.queue.lock().unwrap().current().cloned()
    }

    /// Shuffle on mixes the songs after the one playing; off puts the
    /// queue back in its order.
    pub fn set_shuffle(&self, on: bool) {
        self.queue.lock().unwrap().set_shuffle(on, |a, b| a.video_id == b.video_id);
        self.changed();
    }

    pub fn set_repeat(&self, mode: RepeatMode) {
        self.queue.lock().unwrap().repeat = match mode {
            RepeatMode::Off => Repeat::Off,
            RepeatMode::All => Repeat::All,
            RepeatMode::One => Repeat::One,
        };
        self.changed();
    }

    pub fn pause(&self) {
        self.audio.pause();
    }

    pub fn resume(&self) {
        self.audio.play();
    }

    pub fn seek(&self, seconds: f32) {
        self.audio.seek(seconds);
    }

    pub fn playback(&self) -> Playback {
        let status = &self.audio.status;
        let note = self.note.lock().unwrap().clone();
        let state = if self.loading.load(Ordering::SeqCst) {
            PlayState::Loading
        } else if !note.is_empty() {
            PlayState::Failed
        } else {
            match status.state.load(Ordering::SeqCst) {
                audio::LOADING => PlayState::Loading,
                audio::PLAYING => PlayState::Playing,
                audio::PAUSED => PlayState::Paused,
                audio::ENDED => PlayState::Ended,
                audio::FAILED => PlayState::Failed,
                _ => PlayState::Idle,
            }
        };
        let error = if note.is_empty() {
            status.error.lock().map(|e| e.clone()).unwrap_or_default()
        } else {
            note
        };
        let queue = self.queue.lock().unwrap();
        Playback {
            state,
            position_ms: status.position_ms.load(Ordering::SeqCst),
            duration_ms: status.duration_ms.load(Ordering::SeqCst),
            error,
            index: queue.index() as u32,
            version: self.version.load(Ordering::SeqCst),
            repeat: match queue.repeat {
                Repeat::Off => RepeatMode::Off,
                Repeat::All => RepeatMode::All,
                Repeat::One => RepeatMode::One,
            },
            shuffle: queue.shuffle(),
        }
    }
}

impl Tonearm {
    /// Searches and finds links as the cookie's owner. False when it isn't
    /// a signed-in cookie.
    fn apply(&self, cookie: &str) -> bool {
        let ok = self.client.set_cookie(Some(cookie));
        if ok {
            self.resolver.set_cookie(Some(cookie.to_string()));
        }
        ok
    }

    fn changed(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    /// Says what came of adding a song to the queue, and starts it when the
    /// queue was empty.
    fn after_adding(&self, added: Added, done: String, next: String) -> Result<String, TonearmError> {
        self.changed();
        Ok(match added {
            Added::Started => {
                self.play_current()?;
                String::new()
            }
            Added::Added => done,
            Added::Playing => "That is playing now".into(),
            Added::AlreadyNext => next,
            Added::AlreadyQueued => "That is already in the queue".into(),
        })
    }

    /// Whether `top_up` has something to do: few songs left, repeat off,
    /// and the last song's radio not asked for yet. Cheap, for the queue
    /// thread to check often.
    fn wants_top_up(&self) -> bool {
        let queue = self.queue.lock().unwrap();
        queue.repeat == Repeat::Off
            && queue.index() + 2 >= queue.items().len()
            && queue
                .items()
                .last()
                .is_some_and(|last| *self.topped_for.lock().unwrap() != last.video_id)
    }

    /// With two songs or fewer left and repeat off, adds the last song's
    /// radio, so the music keeps going, as the desktop does. Each last song
    /// is asked about once.
    fn top_up(&self) {
        let last = {
            let queue = self.queue.lock().unwrap();
            if queue.repeat != Repeat::Off || queue.index() + 2 < queue.items().len() {
                return;
            }
            let Some(last) = queue.items().last().cloned() else { return };
            last
        };
        {
            let mut asked = self.topped_for.lock().unwrap();
            if *asked == last.video_id {
                return;
            }
            *asked = last.video_id.clone();
        }
        // A video's radio is videos: seed it with the song instead.
        let seed = model::Item {
            video_id: Some(last.video_id.clone()),
            title: last.title.clone(),
            ..model::Item::default()
        };
        let seed = self.client.song_for(&seed).and_then(|s| s.video_id).unwrap_or(last.video_id.clone());
        let Ok((items, _)) = self.client.queue(Some(&seed), None) else { return };
        let mut queue = self.queue.lock().unwrap();
        // Only onto the same queue, still ending with that song.
        if queue.items().last().map(|s| &s.video_id) != Some(&last.video_id) {
            return;
        }
        let known: std::collections::HashSet<String> = queue.items().iter().map(|s| s.video_id.clone()).collect();
        let more: Vec<Song> = items.iter().filter_map(song).filter(|s| !known.contains(&s.video_id)).collect();
        if !more.is_empty() {
            queue.extend(more);
            drop(queue);
            self.changed();
        }
    }

    fn step(&self, step: Step) -> Result<(), TonearmError> {
        match step {
            Step::Play(index) => {
                self.queue.lock().unwrap().go(index);
                self.changed();
                self.play_current()
            }
            Step::Restart => {
                self.audio.seek(0.0);
                self.audio.play();
                Ok(())
            }
            Step::Stop => Ok(()),
        }
    }

    /// Finds the stream of the queue's song and plays it. A newer request
    /// made meanwhile wins.
    fn play_current(&self) -> Result<(), TonearmError> {
        let Some(song) = self.queue.lock().unwrap().current().cloned() else {
            return Ok(());
        };
        let request = self.request.fetch_add(1, Ordering::SeqCst) + 1;
        self.note.lock().unwrap().clear();
        self.loading.store(true, Ordering::SeqCst);
        let found = self.resolver.resolve(&song.video_id);
        if self.request.load(Ordering::SeqCst) != request {
            return Ok(());
        }
        let stream = match found {
            Ok(stream) => stream,
            Err(message) => {
                *self.note.lock().unwrap() = message.clone();
                self.loading.store(false, Ordering::SeqCst);
                return Err(message.into());
            }
        };
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.audio.load(
            audio::Input::Http {
                url: stream.url,
                headers: stream.headers,
                cache_as: None,
            },
            generation,
        );
        self.loading.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// The song playing has run out: what the queue says comes next.
    fn song_ended(&self) {
        let step = self.queue.lock().unwrap().ended();
        let result = match step {
            // The engine has let go of an ended song, so it loads again.
            Step::Restart => self.play_current(),
            step => self.step(step),
        };
        if let Err(e) = result {
            eprintln!("tonearm: the next song didn't start: {e}");
        }
    }
}

/// Moves the queue on when a song ends, for as long as the app holds the
/// core.
fn watch(tonearm: Weak<Tonearm>) {
    std::thread::Builder::new()
        .name("queue".into())
        .spawn(move || {
            // Only a song seen playing can end: a song just loaded can
            // still show the last one's state for a moment.
            let mut played = 0;
            let mut handled = 0;
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let Some(tonearm) = tonearm.upgrade() else { return };
                let status = &tonearm.audio.status;
                let generation = status.generation.load(Ordering::SeqCst);
                match status.state.load(Ordering::SeqCst) {
                    audio::PLAYING | audio::PAUSED => {
                        played = generation;
                        // Near the end of the queue: its radio joins it,
                        // asked for on a thread of its own.
                        if tonearm.wants_top_up() && !tonearm.topping.swap(true, Ordering::SeqCst) {
                            let worker = tonearm.clone();
                            std::thread::spawn(move || {
                                worker.top_up();
                                worker.topping.store(false, Ordering::SeqCst);
                            });
                        }
                    }
                    audio::ENDED if generation == played && generation != handled => {
                        handled = generation;
                        tonearm.song_ended();
                    }
                    _ => {}
                }
            }
        })
        .expect("cannot start the queue thread");
}

/// Now Playing's background from the cover's RGBA pixels: a soft wash of
/// its colours, `BASE` pixels a side, as on the desktop.
#[uniffi::export]
pub fn backdrop(rgba: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
    tonearm_core::backdrop::backdrop(&rgba, width as usize, height as usize)
}

/// One frame of the background flowing, `t` seconds in: `FRAME` pixels a
/// side, from what `backdrop` gave.
#[uniffi::export]
pub fn backdrop_wave(base: Vec<u8>, t: f32) -> Vec<u8> {
    let n = tonearm_core::backdrop::BASE;
    tonearm_core::backdrop::wave(&base, n, n, t)
}

/// A page of the library, whose artists are followed, by its browse id.
#[uniffi::export]
pub fn is_library_page(id: String) -> bool {
    tonearm_core::menu::is_library_page(&id)
}

use tonearm_core::menu::songs_of;

/// The artist in a subtitle such as "Song • The Weeknd • Starboy • 3:50":
/// the first part that isn't the kind, a length or a count. As the
/// desktop's `credits_with_album`.
fn credit(subtitle: &str) -> String {
    let labels = ["Song", "Video", "Single", "Album", "EP", "Episode"];
    let is_count = |p: &str| [" views", " plays", " likes"].iter().any(|w| p.ends_with(w));
    let is_duration = |p: &str| p.contains(':') && p.chars().all(|c| c.is_ascii_digit() || c == ':');
    subtitle
        .split(" • ")
        .map(str::trim)
        .find(|p| !p.is_empty() && !labels.contains(p) && !is_duration(p) && !is_count(p))
        .unwrap_or_default()
        .to_string()
}

fn song(item: &innertube::model::Item) -> Option<Song> {
    let artist = if item.artists.is_empty() {
        credit(&item.subtitle)
    } else {
        item.artists.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ")
    };
    let cover = innertube::sized(&item.thumb, 544);
    Some(Song {
        video_id: item.video_id.clone()?,
        title: item.title.clone(),
        artist,
        album: item.album.as_ref().map(|(name, _)| name.clone()).unwrap_or_default(),
        cover: if cover.starts_with("//") { format!("https:{cover}") } else { cover },
        duration: item.duration.clone(),
        artists: pages::links(&item.artists),
        album_id: item.album.as_ref().map(|(_, id)| id.clone()),
    })
}

/// What Android's audio needs from Java before anything plays: the app's
/// Java VM and context. Kotlin calls it as `TonearmNative.init(context)`.
#[cfg(target_os = "android")]
mod android {
    use jni::objects::{JClass, JObject};
    use jni::JNIEnv;

    #[no_mangle]
    pub extern "system" fn Java_app_tonearm_TonearmNative_init(env: JNIEnv, _class: JClass, context: JObject) {
        static DONE: std::sync::Once = std::sync::Once::new();
        DONE.call_once(|| {
            let (Ok(vm), Ok(context)) = (env.get_java_vm(), env.new_global_ref(context)) else {
                return;
            };
            let vm_pointer = vm.get_java_vm_pointer() as *mut std::ffi::c_void;
            let context_pointer = context.as_obj().as_raw() as *mut std::ffi::c_void;
            // The context stays referenced for as long as the app runs.
            std::mem::forget(context);
            unsafe { ndk_context::initialize_android_context(vm_pointer, context_pointer) };
        });
    }
}
