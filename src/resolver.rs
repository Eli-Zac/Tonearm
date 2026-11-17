//! Turns a video id into an audio stream URL.
//!
//! YouTube has no public API that hands out audio streams, and the tricks
//! for getting one change every few weeks. yt-dlp keeps up with them, so we
//! use it. On first play Tonearm downloads yt-dlp from its GitHub releases
//! into the app's local data folder and checks it against GitHub's SHA-256
//! digest. On Windows that is the unpacked build, a folder with a small
//! `yt-dlp.exe` in it, because the single-file exe spends about a second
//! unpacking itself on every launch. Elsewhere it is the single binary.
//! Once per start we look for a newer yt-dlp in the background.
//!
//! When the user is signed in, yt-dlp gets their cookie too. Premium
//! accounts then get links that open at once instead of after an ad's worth
//! of waiting.

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
    Some(Box::new(Cached::new(Box::new(YtDlp::new()))))
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

/// How long past `available_at` we keep knocking. Links open one to five
/// seconds before it. About one in ten never opens at all, not even ten
/// seconds later, so waiting longer than this only delays a fresh link.
const GRACE: f64 = 1.5;

/// YouTube hands out links that only work once the free tier's ad could
/// have been skipped. yt-dlp reports that moment as `available_at`, but in
/// practice links usually open a couple of seconds sooner. So we knock
/// every 300 ms instead of sleeping the whole time. Returns false when
/// YouTube still refuses the link a while after `available_at`, so the
/// caller can ask for another instead of playing one that will not open.
fn wait_until_live(url: &str, headers: &[(String, String)], available_at: u64) -> bool {
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    };
    let deadline = (available_at as f64).min(now() + 30.0) + GRACE;
    let http = crate::innertube::agent(Duration::from_secs(5));
    loop {
        let mut request = http.get(url).set("Range", "bytes=0-0");
        for (key, value) in headers {
            request = request.set(key, value);
        }
        let refused = match request.call() {
            Ok(_) => return true,
            Err(ureq::Error::Status(403, _)) => true,
            // Anything else is not the ad gate. Let playback find out.
            Err(_) => false,
        };
        if now() >= deadline {
            return !refused;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

// ------------------------------------------------------------------ yt-dlp

const REPO: &str = "yt-dlp/yt-dlp";
/// The unpacked build: `yt-dlp.exe` next to an `_internal` folder.
#[cfg(windows)]
const ASSET: &str = "yt-dlp_win.zip";
#[cfg(not(windows))]
const ASSET: &str = "yt-dlp";

/// M4A only. The audio engine decodes AAC, not Opus.
const FORMAT: &str = "140/bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]";

struct YtDlp {
    /// Held while yt-dlp downloads, so two plays do not fetch it twice.
    install: Mutex<()>,
    /// Tidies up what earlier runs left behind, before the first resolve.
    tidy: Once,
    self_update: Once,
    /// The signed-in user's `Cookie:` header, if any.
    cookie: Mutex<Option<String>>,
}

fn data_dir() -> Option<PathBuf> {
    crate::store::data_dir()
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

// ----------------------------------------------------------------- cookies

/// Cookie files start with this, so a leftover one is easy to recognise.
const COOKIE_PREFIX: &str = "yt-dlp-cookies-";

/// The app keeps the cookie as a `Cookie:` header for music.youtube.com.
/// yt-dlp reads cookies from a Netscape cookie file, so we write each pair
/// out for `.youtube.com`, which covers music.youtube.com too.
fn netscape(cookie: &str) -> String {
    // A cookie file wants an expiry. An hour is far longer than one run.
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + 3600;
    let mut file = String::from("# Netscape HTTP Cookie File\n");
    for pair in cookie.split(';') {
        let Some((name, value)) = pair.trim().split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        let bad = |s: &str| s.contains(['\t', '\r', '\n']);
        if name.is_empty() || bad(name) || bad(value) {
            continue;
        }
        file.push_str(&format!(
            ".youtube.com\tTRUE\t/\tTRUE\t{expires}\t{name}\t{value}\n"
        ));
    }
    file
}

/// A cookie file for one yt-dlp run. Deleted when dropped, so it is gone
/// as soon as yt-dlp exits, however the run went.
struct CookieFile(PathBuf);

impl CookieFile {
    fn write(folder: &Path, cookie: &str) -> Result<CookieFile, String> {
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = format!(
            "{COOKIE_PREFIX}{}-{}-{nanos}.txt",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        );
        let failed = |e: std::io::Error| format!("Could not sign yt-dlp in: {e}");
        std::fs::create_dir_all(folder).map_err(failed)?;
        let path = folder.join(name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(failed)?;
        // From here on, dropping the guard deletes whatever got written.
        let guard = CookieFile(path);
        file.write_all(netscape(cookie).as_bytes()).map_err(failed)?;
        Ok(guard)
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Deletes cookie files a crashed run left behind. Ones younger than a
/// few minutes may belong to another Tonearm that is resolving right now.
fn sweep_cookie_files(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(COOKIE_PREFIX) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_none_or(|age| age > Duration::from_secs(5 * 60));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

// ------------------------------------------------------- the Windows build

/// Each version unpacks into its own folder in here, named after its tag.
/// An update unpacks next to the old one and only then appears under its
/// final name, so a resolve that is running keeps its files.
#[cfg(windows)]
fn builds_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("yt-dlp"))
}

/// The newest complete build: its version and its `yt-dlp.exe`.
#[cfg(windows)]
fn newest_build() -> Option<(String, PathBuf)> {
    let mut best: Option<(String, PathBuf)> = None;
    for entry in std::fs::read_dir(builds_dir()?).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let exe = entry.path().join("yt-dlp.exe");
        if !is_version(&name) || !exe.is_file() {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(version, _)| crate::update::newer(&name, version))
        {
            best = Some((name, exe));
        }
    }
    best
}

/// Release tags look like `2026.10.26`, sometimes with a fourth number.
/// Folders being unpacked or thrown away have other names.
#[cfg(windows)]
fn is_version(name: &str) -> bool {
    !name.is_empty()
        && name
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Downloads a release's zip, checks it, and unpacks it into the builds
/// folder. Returns the new `yt-dlp.exe`.
#[cfg(windows)]
fn install_build(
    http: &ureq::Agent,
    release: &crate::update::Release,
) -> Result<PathBuf, String> {
    if !is_version(&release.version) {
        return Err(format!("Unexpected yt-dlp version {:?}", release.version));
    }
    let builds = builds_dir().ok_or("No local data folder")?;
    let failed = |e: std::io::Error| format!("Could not save yt-dlp: {e}");
    std::fs::create_dir_all(&builds).map_err(failed)?;
    let bytes = crate::update::download(http, release)?;
    // Named for this process, so two Tonearms never unpack into one folder.
    let work = format!("{}.{}", release.version, std::process::id());
    let zip = builds.join(format!("{work}.zip"));
    let part = builds.join(format!("{work}.part"));
    let _ = std::fs::remove_dir_all(&part);
    let unpacked = (|| {
        std::fs::write(&zip, &bytes).map_err(failed)?;
        std::fs::create_dir_all(&part).map_err(failed)?;
        // Windows 10 and later ship bsdtar, which reads zips. Ask for it by
        // full path so nothing on PATH can stand in for it.
        let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let tar = Path::new(&system).join("System32").join("tar.exe");
        let output = command(&tar)
            .arg("-xf")
            .arg(&zip)
            .arg("-C")
            .arg(&part)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("Could not unpack yt-dlp: {e}"))?;
        if !output.status.success() {
            let why = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Could not unpack yt-dlp: {}", why.trim()));
        }
        if !part.join("yt-dlp.exe").is_file() {
            return Err("The yt-dlp download has no yt-dlp.exe in it".to_string());
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&zip);
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&part);
        return Err(e);
    }
    let done = builds.join(&release.version);
    if done.join("yt-dlp.exe").is_file() {
        // Another Tonearm got there first.
        let _ = std::fs::remove_dir_all(&part);
    } else {
        let _ = std::fs::remove_dir_all(&done);
        if let Err(e) = std::fs::rename(&part, &done) {
            let _ = std::fs::remove_dir_all(&part);
            return Err(failed(e));
        }
    }
    Ok(done.join("yt-dlp.exe"))
}

/// Deletes every build but the newest, and unpacking an earlier run did
/// not finish. A build still in use by another Tonearm cannot be renamed,
/// so we rename first and delete only what moved.
#[cfg(windows)]
fn tidy_builds() {
    let Some(builds) = builds_dir() else {
        return;
    };
    let keep = newest_build().map(|(version, _)| version);
    let Ok(entries) = std::fs::read_dir(&builds) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if Some(&name) == keep.as_ref() {
            continue;
        }
        let path = entry.path();
        if path.is_file() {
            // A zip from an unpack that never finished.
            if name.ends_with(".zip") {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        let young = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(10 * 60));
        if name.ends_with(".part") && young {
            // Another Tonearm may be unpacking an update into it.
            continue;
        }
        let doomed = builds.join(format!("{name}.{}.old", std::process::id()));
        if std::fs::rename(&path, &doomed).is_ok() {
            let _ = std::fs::remove_dir_all(&doomed);
        }
    }
}

impl YtDlp {
    fn new() -> Self {
        YtDlp {
            install: Mutex::new(()),
            tidy: Once::new(),
            self_update: Once::new(),
            cookie: Mutex::new(None),
        }
    }

    #[cfg(windows)]
    fn ensure(&self) -> Result<PathBuf, String> {
        let folder = data_dir().ok_or("No local data folder")?;
        self.tidy.call_once(|| {
            sweep_cookie_files(&folder);
            tidy_builds();
        });
        if let Some((_, exe)) = newest_build() {
            return Ok(exe);
        }
        let _guard = self.install.lock().unwrap();
        // Someone may have installed it while we waited for the lock.
        if let Some((_, exe)) = newest_build() {
            return Ok(exe);
        }
        let http = crate::innertube::agent(Duration::from_secs(30));
        let release = crate::update::latest(&http, REPO, ASSET)
            .map_err(|e| format!("Could not get yt-dlp: {e}"))?;
        let exe =
            install_build(&http, &release).map_err(|e| format!("Could not get yt-dlp: {e}"))?;
        // Fresh from GitHub, so no need to update it this session.
        self.self_update.call_once(|| {});
        Ok(exe)
    }

    #[cfg(not(windows))]
    fn ensure(&self) -> Result<PathBuf, String> {
        let folder = data_dir().ok_or("No local data folder")?;
        self.tidy.call_once(|| sweep_cookie_files(&folder));
        let path = folder.join(ASSET);
        let _guard = self.install.lock().unwrap();
        if path.exists() {
            return Ok(path);
        }
        let http = crate::innertube::agent(Duration::from_secs(30));
        let release = crate::update::latest(&http, REPO, ASSET)
            .map_err(|e| format!("Could not get yt-dlp: {e}"))?;
        let bytes = crate::update::download(&http, &release)
            .map_err(|e| format!("Could not get yt-dlp: {e}"))?;
        std::fs::create_dir_all(&folder).map_err(|e| format!("Could not save yt-dlp: {e}"))?;
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

    /// `yt-dlp -U` cannot update the unpacked build: it mistakes it for the
    /// single-file exe and swaps that in, slow start and all. So once per
    /// start, in the background, we ask GitHub for the latest release
    /// ourselves and unpack it next to the old one when it is newer. The
    /// next resolve picks it up; the old build goes at the next start.
    #[cfg(windows)]
    fn update_later(&self, _path: &Path) {
        self.self_update.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("yt-dlp-update".into())
                .spawn(|| {
                    let http = crate::innertube::agent(Duration::from_secs(30));
                    let Ok(release) = crate::update::latest(&http, REPO, ASSET) else {
                        return;
                    };
                    let current = newest_build().map(|(version, _)| version);
                    if current.is_none_or(|v| crate::update::newer(&release.version, &v)) {
                        let _ = install_build(&http, &release);
                    }
                });
        });
    }

    /// `yt-dlp -U` checks and verifies its own updates. Runs once per start,
    /// in the background, so it never delays a track.
    #[cfg(not(windows))]
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

    /// The single-file `yt-dlp.exe` that Tonearm used before the unpacked
    /// build. Not needed once the new one has resolved a track.
    #[cfg(windows)]
    fn retire_single_file(&self) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            if let Some(folder) = data_dir() {
                let _ = std::fs::remove_file(folder.join("yt-dlp.exe"));
            }
        });
    }

    /// Asks yt-dlp for a stream link. Also returns `available_at`, when
    /// the link will not open before then.
    fn link(&self, video_id: &str) -> Result<(Stream, Option<u64>), String> {
        let path = self.ensure()?;
        let cookie = self.cookie.lock().unwrap().clone();
        let result = match cookie.as_deref() {
            // Signed in, the YouTube Music web client alone is enough and
            // saves yt-dlp asking two more. Should it fail, ask them all.
            Some(cookie) => self
                .run(&path, video_id, Some(cookie), true)
                .or_else(|_| self.run(&path, video_id, Some(cookie), false)),
            None => self.run(&path, video_id, None, false),
        };
        // Update only after the track resolves, so the exe is not replaced
        // while we are running it.
        self.update_later(&path);
        #[cfg(windows)]
        if result.is_ok() {
            self.retire_single_file();
        }
        result
    }

    /// Runs the yt-dlp at `path` once for `video_id`, signed in when
    /// there is a cookie. `music_only` limits yt-dlp to the YouTube Music
    /// web client instead of the several it asks by default.
    fn run(
        &self,
        path: &Path,
        video_id: &str,
        cookie: Option<&str>,
        music_only: bool,
    ) -> Result<(Stream, Option<u64>), String> {
        let jar = match cookie {
            Some(cookie) => Some(CookieFile::write(
                &data_dir().ok_or("No local data folder")?,
                cookie,
            )?),
            None => None,
        };
        // The HLS playlist costs a request and we never use it.
        let extractor = if music_only {
            "youtube:skip=hls;player_client=web_music"
        } else {
            "youtube:skip=hls"
        };
        let mut yt_dlp = command(path);
        yt_dlp.args([
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
            "--extractor-args",
            extractor,
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
        ]);
        if let Some(jar) = &jar {
            // A file, never the cookie itself, so it stays off the
            // command line where other programs could read it.
            yt_dlp.arg("--cookies").arg(&jar.0);
        }
        let output = yt_dlp
            .arg("--")
            .arg(format!("https://music.youtube.com/watch?v={video_id}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        drop(jar);
        let output = output.map_err(|e| format!("Could not start yt-dlp: {e}"))?;
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
        Ok((
            Stream {
                url,
                duration,
                headers,
            },
            available_at,
        ))
    }
}

impl Resolver for YtDlp {
    fn resolve(&self, video_id: &str) -> Result<Stream, String> {
        // A link that never opens gets one more try with a fresh one.
        for _ in 0..2 {
            let (stream, available_at) = self.link(video_id)?;
            match available_at {
                Some(at) if !wait_until_live(&stream.url, &stream.headers, at) => continue,
                _ => return Ok(stream),
            }
        }
        Err("Cannot play this track: YouTube kept refusing the stream link".into())
    }

    fn set_cookie(&self, cookie: Option<String>) {
        *self.cookie.lock().unwrap() = cookie.filter(|c| !c.trim().is_empty());
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

    #[test]
    fn cookie_file_is_netscape_and_goes_away() {
        let text = netscape("SID=abc; __Secure-3PAPISID=d/e=f;  bad ; HSID=g");
        let rows: Vec<Vec<&str>> = text
            .lines()
            .skip(1)
            .map(|l| l.split('\t').collect())
            .collect();
        assert!(text.starts_with("# Netscape HTTP Cookie File\n"));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1][0], ".youtube.com");
        assert_eq!((rows[1][5], rows[1][6]), ("__Secure-3PAPISID", "d/e=f"));

        let folder = std::env::temp_dir().join(format!("tonearm-test-{}", std::process::id()));
        let path = {
            let file = CookieFile::write(&folder, "SID=abc").unwrap();
            assert!(file.0.is_file());
            file.0.clone()
        };
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(folder);
    }

    #[cfg(windows)]
    #[test]
    fn recognises_versions() {
        assert!(is_version("2026.10.26"));
        assert!(is_version("2026.10.26.1"));
        assert!(!is_version("2026.10.26.1234.part"));
        assert!(!is_version("2026.10.26.1234.zip"));
        assert!(!is_version(""));
    }
}
