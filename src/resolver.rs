//! Turns a video id into an audio stream URL.
//!
//! YouTube has no public API that hands out audio streams, and the tricks
//! for getting one change every few weeks. yt-dlp keeps up with them, so we
//! use it. On first play Tonearm downloads `yt-dlp.exe` from its GitHub
//! releases into the app's local data folder, checks it against GitHub's
//! SHA-256 digest, and after that lets yt-dlp update itself once per start.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex, Once};
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
    Some(Box::new(Cached {
        inner: Box::new(YtDlp {
            install: Mutex::new(()),
            self_update: Once::new(),
        }),
        state: Mutex::default(),
        done: Condvar::new(),
    }))
}

// ------------------------------------------------------------------- cache

/// Stream links last for hours. Half an hour keeps us well clear of that.
const FRESH: Duration = Duration::from_secs(30 * 60);

/// Remembers resolved links, so a prefetched next track starts at once.
/// A play that asks for a track still being prefetched waits for that
/// result instead of starting a second yt-dlp.
struct Cached {
    inner: Box<dyn Resolver>,
    state: Mutex<CacheState>,
    done: Condvar,
}

#[derive(Default)]
struct CacheState {
    ready: HashMap<String, (Stream, Instant)>,
    pending: HashSet<String>,
}

impl Cached {
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
        drop(state);
        let result = self.inner.resolve(video_id);
        let mut state = self.state.lock().unwrap();
        state.pending.remove(video_id);
        if let Ok(stream) = &result {
            state
                .ready
                .insert(video_id.to_string(), (stream.clone(), Instant::now()));
        }
        self.done.notify_all();
        result
    }
}

impl Resolver for Cached {
    fn resolve(&self, video_id: &str) -> Result<Stream, String> {
        self.fetch(video_id)
    }

    fn prefetch(&self, video_id: &str) {
        let _ = self.fetch(video_id);
    }
}

/// YouTube hands out links that only work once the free tier's ad could
/// have been skipped. yt-dlp reports that moment as `available_at`, but in
/// practice links open a couple of seconds sooner. So we knock every 300 ms
/// instead of sleeping the whole time, and stop once `available_at` has
/// passed either way.
fn wait_until_live(url: &str, headers: &[(String, String)], available_at: u64) {
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    };
    let deadline = (available_at as f64).min(now() + 30.0) + 0.25;
    let http = crate::innertube::agent(Duration::from_secs(5));
    while now() < deadline {
        let mut request = http.get(url).set("Range", "bytes=0-0");
        for (key, value) in headers {
            request = request.set(key, value);
        }
        if request.call().is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

// ------------------------------------------------------------------ yt-dlp

const REPO: &str = "yt-dlp/yt-dlp";
#[cfg(windows)]
const ASSET: &str = "yt-dlp.exe";
#[cfg(not(windows))]
const ASSET: &str = "yt-dlp";

/// M4A only. The audio engine decodes AAC, not Opus.
const FORMAT: &str = "140/bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]";

struct YtDlp {
    /// Held while yt-dlp downloads, so two plays do not fetch it twice.
    install: Mutex<()>,
    self_update: Once,
}

fn tool_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("tonearm").join(ASSET))
}

fn command(path: &Path) -> Command {
    let mut command = Command::new(path);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The last `ERROR:` line yt-dlp wrote, minus its noise.
fn error_text(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .rev()
        .find(|l| l.starts_with("ERROR:"))
        .or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("yt-dlp failed without saying why")
        .trim_start_matches("ERROR:")
        .trim();
    // "[youtube] abc123: Sign in to confirm..." reads better without the tag.
    let line = match line.split_once("]: ").or_else(|| {
        line.starts_with('[')
            .then(|| line.split_once(": "))
            .flatten()
    }) {
        Some((_, rest)) => rest,
        None => line,
    };
    format!("Cannot play this track: {line}")
}

impl YtDlp {
    fn ensure(&self) -> Result<PathBuf, String> {
        let path = tool_path().ok_or("No local data folder")?;
        let _guard = self.install.lock().unwrap();
        if path.exists() {
            return Ok(path);
        }
        let http = crate::innertube::agent(Duration::from_secs(30));
        let release = crate::update::latest(&http, REPO, ASSET)
            .map_err(|e| format!("Could not get yt-dlp: {e}"))?;
        let bytes = crate::update::download(&http, &release)
            .map_err(|e| format!("Could not get yt-dlp: {e}"))?;
        let folder = path.parent().ok_or("No local data folder")?;
        std::fs::create_dir_all(folder).map_err(|e| format!("Could not save yt-dlp: {e}"))?;
        let partial = path.with_extension("part");
        std::fs::write(&partial, bytes).map_err(|e| format!("Could not save yt-dlp: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755));
        }
        std::fs::rename(&partial, &path).map_err(|e| format!("Could not save yt-dlp: {e}"))?;
        // Fresh from GitHub, so no need to update it this session.
        self.self_update.call_once(|| {});
        Ok(path)
    }

    /// `yt-dlp -U` checks and verifies its own updates. Runs once per start,
    /// in the background, so it never delays a track.
    fn update_later(&self, path: &Path) {
        self.self_update.call_once(|| {
            let path = path.to_path_buf();
            let _ = std::thread::Builder::new()
                .name("yt-dlp-update".into())
                .spawn(move || {
                    let _ = command(&path)
                        .arg("-U")
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                });
        });
    }
}

impl Resolver for YtDlp {
    fn resolve(&self, video_id: &str) -> Result<Stream, String> {
        let path = self.ensure()?;
        let output = command(&path)
            .args([
                "--no-config",
                "--no-playlist",
                "--no-warnings",
                // yt-dlp needs a JavaScript runtime for some YouTube
                // formats. Use whichever one the machine has.
                "--js-runtimes",
                "deno",
                "--js-runtimes",
                "node",
                "--js-runtimes",
                "bun",
                // The HLS playlist costs a request and we never use it.
                "--extractor-args",
                "youtube:skip=hls",
                "-f",
                FORMAT,
                "-O",
                "%(duration)s",
                "-O",
                "%(http_headers)j",
                "-O",
                "%(available_at)s",
                "-O",
                "urls",
                "--",
            ])
            .arg(format!("https://music.youtube.com/watch?v={video_id}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("Could not start yt-dlp: {e}"))?;
        // Update only after the track resolves, so the exe is not replaced
        // while we are running it.
        self.update_later(&path);
        if !output.status.success() {
            return Err(error_text(&output.stderr));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let duration = lines.next().and_then(|d| d.parse().ok()).unwrap_or(0.0);
        let headers: Vec<(String, String)> = lines
            .next()
            .and_then(|h| serde_json::from_str::<serde_json::Value>(h).ok())
            .and_then(|h| h.as_object().cloned())
            .map(|h| {
                h.into_iter()
                    .filter_map(|(k, v)| Some((k, v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let available_at = lines.next().and_then(|t| t.parse::<u64>().ok());
        let url = lines
            .find(|l| l.starts_with("http"))
            .ok_or("yt-dlp found no playable audio for this track")?
            .to_string();
        if let Some(at) = available_at {
            wait_until_live(&url, &headers, at);
        }
        Ok(Stream {
            url,
            duration,
            headers,
        })
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
        let cache = Arc::new(Cached {
            inner: Box::new(Slow(calls.clone())),
            state: Mutex::default(),
            done: Condvar::new(),
        });
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
}
