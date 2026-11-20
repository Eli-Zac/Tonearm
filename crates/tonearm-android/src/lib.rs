//! Tonearm's core for the Android app. UniFFI writes the Kotlin side from
//! what is exported here: one `Tonearm` object that searches YouTube Music
//! and plays songs through the core's own engine.
//!
//! Calls that reach the network block, so the app makes them off its main
//! thread. Before anything plays, the app calls `TonearmNative.init` with
//! its context (see `android`), which Android's audio needs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tonearm_core::innertube::model::Kind;
use tonearm_core::{audio, innertube, resolver, store};

uniffi::setup_scaffolding!();

/// A song found, as the app lists it.
#[derive(uniffi::Record)]
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
    Idle,
    Loading,
    Playing,
    Paused,
    Ended,
    Failed,
}

/// Where playback stands, for the app to poll.
#[derive(uniffi::Record)]
pub struct Playback {
    pub state: PlayState,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Why it failed, when it did.
    pub error: String,
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
    /// Counts songs loaded, so the engine can tell them apart.
    generation: AtomicU64,
}

#[uniffi::export]
impl Tonearm {
    /// Starts the core, keeping its settings and data under `home`: the
    /// app's own files folder.
    #[uniffi::constructor]
    pub fn new(home: String) -> Result<Arc<Self>, TonearmError> {
        store::set_home(home.into());
        let resolver = resolver::default().ok_or_else(|| "no way to find song links".to_string())?;
        let tonearm = Tonearm {
            client: innertube::Client::new(),
            resolver,
            audio: audio::Audio::new(),
            generation: AtomicU64::new(0),
        };
        // Signed in as last time, as the desktop app starts.
        if let Some(cookie) = store::load("session") {
            tonearm.apply(&cookie);
        }
        Ok(Arc::new(tonearm))
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

    /// Finds the song's stream and plays it, in place of whatever played.
    pub fn play(&self, video_id: String) -> Result<(), TonearmError> {
        let stream = self.resolver.resolve(&video_id)?;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.audio.load(
            audio::Input::Http {
                url: stream.url,
                headers: stream.headers,
                cache_as: None,
            },
            generation,
        );
        Ok(())
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
        let state = match status.state.load(Ordering::SeqCst) {
            audio::LOADING => PlayState::Loading,
            audio::PLAYING => PlayState::Playing,
            audio::PAUSED => PlayState::Paused,
            audio::ENDED => PlayState::Ended,
            audio::FAILED => PlayState::Failed,
            _ => PlayState::Idle,
        };
        Playback {
            state,
            position_ms: status.position_ms.load(Ordering::SeqCst),
            duration_ms: status.duration_ms.load(Ordering::SeqCst),
            error: status.error.lock().map(|e| e.clone()).unwrap_or_default(),
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
