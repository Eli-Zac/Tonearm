//! Turns a video id into an audio stream URL.
//!
//! We ask InnerTube's `player` endpoint ourselves (`innertube::player`), as
//! a few clients that need no proof-of-origin token, and solve the player
//! script's challenges with `innertube::challenge`. Nothing else is
//! installed or run: the computer and the phone resolve the same way.
//! When YouTube changes how stream links work, those two files are where
//! to follow it, and yt-dlp's own changes are the best guide to how.

use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Stream {
    pub url: String,
    /// Seconds. Zero when unknown.
    pub duration: f32,
    /// Some stream links only work with the exact headers of the client
    /// that asked for them, user agent above all.
    pub headers: Vec<(String, String)>,
}

pub trait Resolver: Send + Sync {
    fn resolve(&self, video_id: &str) -> Result<Stream, String>;

    /// Gets a track ready ahead of time. Blocks, so run it on a worker.
    fn prefetch(&self, _video_id: &str) {}

    /// Resolves as the signed-in user from now on, or signed out with
    /// `None`. The cookie is the raw `Cookie:` header the app signs in with.
    fn set_cookie(&self, _cookie: Option<String>) {}

    /// Drops what is remembered about a track, so the next resolve asks
    /// afresh. For a link that turned out not to play.
    fn forget(&self, _video_id: &str) {}
}

/// Plays one fixed URL for every track. Set `TONEARM_TEST_STREAM` to an
/// M4A or MP4 audio URL to exercise the audio path end to end.
struct Fixed(String);

impl Resolver for Fixed {
    fn resolve(&self, _video_id: &str) -> Result<Stream, String> {
        Ok(Stream {
            url: self.0.clone(),
            duration: 0.0,
            headers: Vec::new(),
        })
    }
}

pub fn default() -> Option<Box<dyn Resolver>> {
    if let Ok(url) = std::env::var("TONEARM_TEST_STREAM") {
        return Some(Box::new(Fixed(url)));
    }
    forget_yt_dlp();
    Some(Box::new(Cached::new(Box::new(Native::new()))))
}

/// Deletes what the yt-dlp fallback of older versions kept in the local
/// data folder: yt-dlp, the Deno it ran with, and any cookie file a crash
/// left behind. Once per start, in the background. Finding nothing is
/// the usual case and costs a few file checks.
fn forget_yt_dlp() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(folder) = crate::store::data_dir() else {
            return;
        };
        let _ = std::thread::Builder::new()
            .name("forget-yt-dlp".into())
            .spawn(move || {
                for dir in ["yt-dlp", "deno"] {
                    let _ = std::fs::remove_dir_all(folder.join(dir));
                }
                for file in ["yt-dlp.exe", "yt-dlp", "yt-dlp.part"] {
                    let _ = std::fs::remove_file(folder.join(file));
                }
                let Ok(entries) = std::fs::read_dir(&folder) else {
                    return;
                };
                for entry in entries.flatten() {
                    if entry.file_name().to_string_lossy().starts_with("yt-dlp-cookies-") {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            });
    });
}

// ------------------------------------------------------------------ native

/// Asks InnerTube directly.
struct Native {
    /// Replaced after a network failure: a change of network leaves its kept
    /// connections dead.
    http: std::sync::RwLock<ureq::Agent>,
    cookie: Mutex<Option<String>>,
}

impl Native {
    fn new() -> Self {
        Native {
            http: std::sync::RwLock::new(crate::innertube::agent(Duration::from_secs(15))),
            cookie: Mutex::new(None),
        }
    }

    fn attempt(&self, video_id: &str) -> Result<Stream, crate::innertube::player::Miss> {
        let cookie = self.cookie.lock().unwrap().clone();
        let http = self.http.read().unwrap().clone();
        match crate::innertube::player::resolve(&http, cookie.as_deref(), video_id) {
            // Network trouble may be the network having changed under a kept
            // connection: once more on a fresh one. Only then: a client that
            // refused would refuse again, and asking twice doubles the wait.
            Err(crate::innertube::player::Miss::Network(_)) => {
                let fresh = crate::innertube::agent(Duration::from_secs(15));
                *self.http.write().unwrap() = fresh.clone();
                crate::innertube::player::resolve(&fresh, cookie.as_deref(), video_id)
            }
            result => result,
        }
    }
}

impl Resolver for Native {
    fn resolve(&self, video_id: &str) -> Result<Stream, String> {
        match self.attempt(video_id) {
            Ok(stream) => Ok(stream),
            Err(miss) => Err(format!("Cannot play this track: {miss}")),
        }
    }

    fn set_cookie(&self, cookie: Option<String>) {
        let cookie = cookie.filter(|c| !c.trim().is_empty());
        let signing_in = cookie.is_some() && self.cookie.lock().unwrap().is_none();
        *self.cookie.lock().unwrap() = cookie;
        // Signed in, the first clients asked need the player's challenges
        // solved. Getting the player ready now saves the first song a few
        // seconds.
        if signing_in {
            let http = self.http.read().unwrap().clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::innertube::challenge::warm(&http) {
                    eprintln!("native resolver: warming the player failed: {e}");
                }
            });
        }
    }
}

// ------------------------------------------------------------------- cache

/// Stream links last for hours. Half an hour keeps us well clear of that.
const FRESH: Duration = Duration::from_secs(30 * 60);

/// Remembers resolved links, so a prefetched next track starts at once.
/// A play that asks for a track still being prefetched waits for that
/// result instead of asking YouTube a second time.
struct Cached {
    inner: Box<dyn Resolver>,
    state: Mutex<CacheState>,
    done: Condvar,
}

#[derive(Default)]
struct CacheState {
    ready: HashMap<String, (Stream, Instant)>,
    pending: HashSet<String>,
    /// A fingerprint of the cookie in use, so we can tell when it changes
    /// without keeping a second copy of it.
    signed_in_as: Option<u64>,
    /// Goes up whenever the cookie changes. A resolve that started under an
    /// older cookie does not go in the cache.
    generation: u64,
}

impl Cached {
    fn new(inner: Box<dyn Resolver>) -> Self {
        Cached {
            inner,
            state: Mutex::default(),
            done: Condvar::new(),
        }
    }

    fn fetch(&self, video_id: &str) -> Result<Stream, String> {
        let mut state = self.state.lock().unwrap();
        loop {
            state.ready.retain(|_, (_, at)| at.elapsed() < FRESH);
            if let Some((stream, _)) = state.ready.get(video_id) {
                return Ok(stream.clone());
            }
            if !state.pending.contains(video_id) {
                break;
            }
            state = self.done.wait(state).unwrap();
        }
        state.pending.insert(video_id.to_string());
        let generation = state.generation;
        drop(state);
        let result = self.inner.resolve(video_id);
        let mut state = self.state.lock().unwrap();
        state.pending.remove(video_id);
        if let Ok(stream) = &result {
            if state.generation == generation {
                state
                    .ready
                    .insert(video_id.to_string(), (stream.clone(), Instant::now()));
            }
        }
        self.done.notify_all();
        result
    }
}

fn fingerprint(cookie: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cookie.hash(&mut hasher);
    hasher.finish()
}

impl Resolver for Cached {
    fn resolve(&self, video_id: &str) -> Result<Stream, String> {
        self.fetch(video_id)
    }

    fn prefetch(&self, video_id: &str) {
        let _ = self.fetch(video_id);
    }

    fn set_cookie(&self, cookie: Option<String>) {
        let print = cookie.as_deref().map(fingerprint);
        let mut state = self.state.lock().unwrap();
        if state.signed_in_as != print {
            // Links resolved signed out wait for an ad, and links resolved
            // as someone else belong to them. Neither should play now.
            state.signed_in_as = print;
            state.generation += 1;
            state.ready.clear();
        }
        self.inner.set_cookie(cookie);
    }

    fn forget(&self, video_id: &str) {
        self.state.lock().unwrap().ready.remove(video_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Slow(Arc<AtomicUsize>);

    impl Resolver for Slow {
        fn resolve(&self, video_id: &str) -> Result<Stream, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(200));
            Ok(Stream {
                url: format!("https://example.invalid/{video_id}"),
                duration: 1.0,
                headers: Vec::new(),
            })
        }
    }

    #[test]
    fn prefetch_and_play_share_one_resolve() {
        let calls = Arc::new(AtomicUsize::new(0));
        let cache = Arc::new(Cached::new(Box::new(Slow(calls.clone()))));
        let early = cache.clone();
        let prefetch = std::thread::spawn(move || early.prefetch("a"));
        std::thread::sleep(Duration::from_millis(50));
        // Asked while the prefetch is still running.
        assert_eq!(cache.resolve("a").unwrap().url, "https://example.invalid/a");
        prefetch.join().unwrap();
        // Asked again later, straight from the cache.
        cache.resolve("a").unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cache.resolve("b").unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn forgetting_resolves_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let cache = Cached::new(Box::new(Slow(calls.clone())));
        cache.resolve("a").unwrap();
        cache.forget("a");
        cache.resolve("a").unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn signing_in_forgets_links() {
        let calls = Arc::new(AtomicUsize::new(0));
        let cache = Cached::new(Box::new(Slow(calls.clone())));
        cache.resolve("a").unwrap();
        // The same cookie again changes nothing.
        cache.set_cookie(None);
        cache.resolve("a").unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cache.set_cookie(Some("SAPISID=x".into()));
        cache.resolve("a").unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        cache.set_cookie(Some("SAPISID=x".into()));
        cache.resolve("a").unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
