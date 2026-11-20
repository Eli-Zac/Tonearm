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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tonearm_core::innertube::model::Kind;
use tonearm_core::queue::{Queue, Repeat, Step};
use tonearm_core::{audio, innertube, resolver, store};

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

    /// Plays `songs` from `index`, in place of the queue.
    pub fn set_queue(&self, songs: Vec<Song>, index: u32) -> Result<(), TonearmError> {
        self.queue.lock().unwrap().set(songs, index as usize);
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

    pub fn queue(&self) -> Vec<Song> {
        self.queue.lock().unwrap().items().to_vec()
    }

    /// The song playing, or the one the queue is on.
    pub fn now_playing(&self) -> Option<Song> {
        self.queue.lock().unwrap().current().cloned()
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
                    audio::PLAYING | audio::PAUSED => played = generation,
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

fn song(item: &innertube::model::Item) -> Option<Song> {
    let artist = if item.artists.is_empty() {
        item.subtitle.split(" • ").next().unwrap_or_default().to_string()
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
