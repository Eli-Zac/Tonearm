//! Spotify Canvas: the short looping video some tracks show in place of
//! their cover.
//!
//! Canvas is not in Spotify's public Web API. The Spotify web player asks a
//! private endpoint for it, with a web player token, and gets back the
//! clip's URL. Client ID tokens are refused there (403), so Canvas needs a
//! Spotify web sign in: the `sp_dc` cookie, which Connections keeps.
//! `spotify_token` trades it for a token. When anything here fails, the
//! song simply shows its still cover.
//!
//! The clip is an MP4 loop. On Windows, Media Foundation decodes it on its
//! own thread, frame by frame, so memory stays small however long it is.

use crate::images::Pixels;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CANVAS_URL: &str = "https://spclient.wg.spotify.com/canvaz-cache/v0/canvases";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
/// The clip is decoded at this width. Wide enough for the Now playing panel.
pub const WIDTH: u32 = 400;
const MAX_CLIP: u64 = 24 * 1024 * 1024;
/// Clips kept on disk, so a song played again needs no download.
const KEEP_CLIPS: usize = 60;

/// What Canvas needs from Connections.
pub struct Keys {
    pub cookie: String,
    pub client_id: String,
    pub client_secret: String,
}

impl Keys {
    /// The token to search with: the client ID's when set, as the web
    /// player's is soon rate limited.
    fn search(&self, http: &ureq::Agent, web: &str) -> String {
        if self.client_id.is_empty() || self.client_secret.is_empty() {
            return web.to_string();
        }
        crate::spotify_token::search_token(http, &self.client_id, &self.client_secret).unwrap_or_else(|_| web.to_string())
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ----------------------------------------------------------------- lookup

/// The Spotify id of the track, matched on its artist where we can.
fn track_id(http: &ureq::Agent, token: &str, title: &str, artist: &str) -> Result<Option<String>, String> {
    let response: Value = http
        .get("https://api.spotify.com/v1/search")
        .set("Authorization", &format!("Bearer {token}"))
        .query("q", &format!("{title} {artist}"))
        .query("type", "track")
        .query("limit", "5")
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    let items = response
        .pointer("/tracks/items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Letters and digits only, so "ROSÉ" on Spotify matches "ROSE" here.
    let plain = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let wanted = plain(artist);
    let by_artist = |item: &Value| {
        item.get("artists")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| a.get("name").and_then(Value::as_str))
            .map(plain)
            .any(|name| !name.is_empty() && (wanted.contains(&name) || name.contains(&wanted)))
    };
    // A track by someone else is a wrong guess, and a wrong clip is worse
    // than the still cover.
    Ok(items
        .iter()
        .find(|item| by_artist(item))
        .and_then(|item| item.get("id").and_then(Value::as_str))
        .map(str::to_string))
}

/// The Canvas clip's URL for a track, or None when it has none.
fn canvas_url(http: &ureq::Agent, token: &str, track_id: &str) -> Result<Option<String>, String> {
    // EntityCanvazRequest { repeated Entity entities = 1 }
    // Entity { string entity_uri = 1 }
    let uri = format!("spotify:track:{track_id}");
    let body = field(1, &field(1, uri.as_bytes()));
    let response = http
        .post(CANVAS_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/x-protobuf")
        .set("Accept", "application/protobuf")
        .set("App-Platform", "WebPlayer")
        .set("User-Agent", USER_AGENT)
        .send_bytes(&body)
        .map_err(|e| e.to_string())?;
    let bytes = read_all(response, 1024 * 1024)?;
    // EntityCanvazResponse { repeated Canvaz canvases = 1 }
    // Canvaz { string id = 1; string url = 2; ... }
    Ok(fields(&bytes)
        .into_iter()
        .filter(|(n, _)| *n == 1)
        .find_map(|(_, canvas)| {
            fields(canvas)
                .into_iter()
                .find(|(n, _)| *n == 2)
                .and_then(|(_, url)| String::from_utf8(url.to_vec()).ok())
        })
        .filter(|url| url.ends_with(".mp4")))
}

fn read_all(response: ureq::Response, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn clips_dir() -> Option<PathBuf> {
    Some(crate::store::data_dir()?.join("canvas"))
}

/// Finds the track's Canvas and keeps the clip on disk. Ok(None) means the
/// track has no Canvas. Blocks, so call it from a worker.
/// How long "this song has no Canvas" holds before we ask again. Artists
/// add Canvases to old songs now and then.
const MISS_DAYS: u64 = 14;

/// What each song turned out to have, kept in `lookups.json` beside the
/// clips: the clip's file name, or "" for no Canvas, and when we looked.
/// A replay, even after a restart, asks Spotify nothing.
static SEEN: Mutex<Option<serde_json::Map<String, Value>>> = Mutex::new(None);

fn lookups_file() -> Option<PathBuf> {
    Some(clips_dir()?.join("lookups.json"))
}

fn with_seen<T>(f: impl FnOnce(&mut serde_json::Map<String, Value>) -> T) -> T {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(|| {
        lookups_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    });
    f(seen)
}

pub fn fetch(http: &ureq::Agent, keys: &Keys, title: &str, artist: &str) -> Result<Option<PathBuf>, String> {
    let key = format!("{}\u{1f}{}", title.to_lowercase(), artist.to_lowercase());
    let known = with_seen(|seen| {
        let entry = seen.get(&key)?;
        let file = entry.get(0)?.as_str()?.to_string();
        let at = entry.get(1)?.as_u64()?;
        Some((file, at))
    });
    if let Some((file, at)) = known {
        if file.is_empty() {
            if unix_now().saturating_sub(at) < MISS_DAYS * 86_400 {
                return Ok(None);
            }
        } else if let Some(path) = clips_dir().map(|d| d.join(&file)).filter(|p| p.is_file()) {
            return Ok(Some(path));
        }
    }
    let found = look_up(http, keys, title, artist)?;
    let file = found
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let text = with_seen(|seen| {
        seen.insert(key, serde_json::json!([file, unix_now()]));
        Value::Object(seen.clone()).to_string()
    });
    if let Some(path) = lookups_file() {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, text);
    }
    Ok(found)
}

fn look_up(http: &ureq::Agent, keys: &Keys, title: &str, artist: &str) -> Result<Option<PathBuf>, String> {
    let token = crate::spotify_token::web_token(http, &keys.cookie)?;
    let Some(id) = track_id(http, &keys.search(http, &token), title, artist)? else {
        return Ok(None);
    };
    let dir = clips_dir().ok_or("No local data folder")?;
    let path = dir.join(format!("{id}.mp4"));
    if path.is_file() {
        return Ok(Some(path));
    }
    let Some(url) = canvas_url(http, &token, &id)? else {
        return Ok(None);
    };
    let bytes = read_all(http.get(&url).call().map_err(|e| e.to_string())?, MAX_CLIP)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let partial = path.with_extension("part");
    std::fs::write(&partial, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    trim_clips(&dir);
    Ok(Some(path))
}

/// Keeps the newest clips and deletes the rest.
fn trim_clips(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut clips: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "mp4"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    clips.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in clips.into_iter().skip(KEEP_CLIPS) {
        let _ = std::fs::remove_file(path);
    }
}

// --------------------------------------------------------------- protobuf

/// One length-delimited protobuf field.
fn field(number: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(&mut out, u64::from(number << 3 | 2));
    varint(&mut out, bytes.len() as u64);
    out.extend_from_slice(bytes);
    out
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// The length-delimited fields of a message, by number. Others are skipped.
fn fields(bytes: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let Some(key) = read_varint(bytes, &mut at) else { break };
        let number = (key >> 3) as u32;
        match key & 7 {
            0 => {
                if read_varint(bytes, &mut at).is_none() {
                    break;
                }
            }
            1 => at += 8,
            2 => {
                let Some(len) = read_varint(bytes, &mut at) else { break };
                let end = at + len as usize;
                if end > bytes.len() {
                    break;
                }
                out.push((number, &bytes[at..end]));
                at = end;
            }
            5 => at += 4,
            _ => break,
        }
    }
    out
}

// ---------------------------------------------------------------- playback

/// A clip looping on its own thread. Dropping it stops the thread.
pub struct Clip {
    stop: Arc<AtomicBool>,
    /// While set, the thread holds its frame and decodes nothing.
    pub paused: Arc<AtomicBool>,
}

impl Drop for Clip {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Clip {
    /// Starts looping the clip at `path`, handing each frame to `show`
    /// with a heavy blur of it for a background. The clip loops from its
    /// start; the blur goes back and forth, so a clip that does not loop
    /// cleanly never jumps behind the expanded view.
    /// `failed` runs once if the clip cannot be decoded at all.
    pub fn play(
        path: PathBuf,
        show: impl Fn(Pixels, Pixels) + Send + 'static,
        failed: impl FnOnce(String) + Send + 'static,
    ) -> Clip {
        Clip::play_at(path, WIDTH, show, failed)
    }

    /// As `play`, decoding at `width` instead, or the clip's own width when
    /// that is smaller. Apple's moving covers are shown larger than Canvas.
    pub fn play_at(
        path: PathBuf,
        width: u32,
        show: impl Fn(Pixels, Pixels) + Send + 'static,
        failed: impl FnOnce(String) + Send + 'static,
    ) -> Clip {
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let (s, p) = (stop.clone(), paused.clone());
        std::thread::Builder::new()
            .name("canvas".into())
            .spawn(move || {
                if let Err(message) = video::run(&path, width, &s, &p, &show) {
                    failed(message);
                }
            })
            .ok();
        Clip { stop, paused }
    }
}

#[cfg(windows)]
mod video {
    use super::Pixels;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use windows::core::{Interface, HSTRING, PROPVARIANT};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    /// MFVideoFormat_ABGR32 from mfapi.h, which the crate leaves out: bytes
    /// in R, G, B, A order. D3DFMT_A8B8G8R8 is 32.
    const ABGR32: windows::core::GUID = windows::core::GUID::from_u128(0x00000020_0000_0010_8000_00aa00389b71);

    /// MF_VERSION from mfapi.h: the SDK version in the high word.
    const MF_VERSION: u32 = (0x0002 << 16) | 0x0070;

    pub fn run(
        path: &Path,
        width: u32,
        stop: &AtomicBool,
        paused: &AtomicBool,
        show: &dyn Fn(Pixels, Pixels),
    ) -> Result<(), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let started = MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| e.to_string());
            let result = started.and_then(|_| decode(path, width, stop, paused, show));
            let _ = MFShutdown();
            CoUninitialize();
            result
        }
    }

    /// A Direct3D 11 device on the graphics card, wrapped for Media
    /// Foundation to decode on.
    unsafe fn gpu_manager() -> windows::core::Result<IMFDXGIDeviceManager> {
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
        use windows::Win32::Graphics::Direct3D11::*;
        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
        let device = device.ok_or_else(windows::core::Error::empty)?;
        // The decoder uses it from threads of its own.
        if let Ok(threads) = device.cast::<ID3D11Multithread>() {
            let _ = threads.SetMultithreadProtected(true);
        }
        let mut token = 0u32;
        let mut manager: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
        let manager = manager.ok_or_else(windows::core::Error::empty)?;
        manager.ResetDevice(&device, token)?;
        Ok(manager)
    }

    unsafe fn decode(
        path: &Path,
        wanted_width: u32,
        stop: &AtomicBool,
        paused: &AtomicBool,
        show: &dyn Fn(Pixels, Pixels),
    ) -> Result<(), String> {
        let err = |e: windows::core::Error| e.to_string();
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 1).map_err(err)?;
        let attributes = attributes.ok_or("No attributes")?;
        // Lets the reader convert to RGB and scale for us.
        attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(err)?;
        // On the graphics card when there is one: decoding, scaling and
        // converting a clip there costs next to nothing, where Windows'
        // own software takes about three cores for a 960 pixel clip. Kept
        // until the reader is done with it.
        let gpu = gpu_manager().ok();
        if let Some(manager) = &gpu {
            attributes.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, manager).map_err(err)?;
        }
        let url = HSTRING::from(path.as_os_str());
        let reader = MFCreateSourceReaderFromURL(&url, &attributes).map_err(err)?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(err)?;
        reader.SetStreamSelection(stream, true).map_err(err)?;

        // Keep the clip's shape at our width, with even sides.
        let native = reader.GetNativeMediaType(stream, 0).map_err(err)?;
        let size = native.GetUINT64(&MF_MT_FRAME_SIZE).map_err(err)?;
        let (w, h) = ((size >> 32) as u32, size as u32);
        if w == 0 || h == 0 {
            return Err("The clip has no size".into());
        }
        let width = wanted_width.min(w) & !1;
        let height = ((h as u64 * width as u64 / w as u64) as u32).max(2) & !1;
        let wanted = MFCreateMediaType().map_err(err)?;
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(err)?;
        wanted
            .SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(width) << 32) | u64::from(height))
            .map_err(err)?;
        // Bytes in RGBA order when the decoder can, so each row is a plain
        // copy, which is fast even in a debug build. BGRX otherwise.
        let mut rgba = false;
        for (subtype, is_rgba) in [(ABGR32, true), (MFVideoFormat_RGB32, false)] {
            wanted.SetGUID(&MF_MT_SUBTYPE, &subtype).map_err(err)?;
            if reader.SetCurrentMediaType(stream, None, &wanted).is_ok() {
                rgba = is_rgba;
                break;
            }
            if !is_rgba {
                // Some decoders will not scale. Take the full size instead.
                wanted.DeleteItem(&MF_MT_FRAME_SIZE).map_err(err)?;
                reader.SetCurrentMediaType(stream, None, &wanted).map_err(err)?;
            }
        }
        let current = reader.GetCurrentMediaType(stream).map_err(err)?;
        let size = current.GetUINT64(&MF_MT_FRAME_SIZE).map_err(err)?;
        let (width, height) = ((size >> 32) as u32, size as u32);
        // Positive for top-down rows, negative for bottom-up.
        let stride = current
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map(|s| s as i32)
            .unwrap_or(width as i32 * 4);

        let mut loop_start = Instant::now();
        let mut shown_any = false;
        // How far into the clip the last frame was, to resume from there.
        let mut last = Duration::ZERO;
        let mut was_paused = false;
        // The first frame's time in this pass. Apple's clips count from ten
        // seconds in, not from zero.
        let mut base: Option<i64> = None;
        // The blurs of the first pass, kept to play back and forth after.
        // A few kilobytes each.
        let mut blurs: Vec<Pixels> = Vec::new();
        // Times through the clip so far, and frames into this one.
        let mut pass = 0usize;
        let mut step = 0usize;
        while !stop.load(Ordering::SeqCst) {
            if paused.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
                was_paused = true;
                continue;
            }
            if was_paused {
                // Carry on from the held frame, not with a burst of late ones.
                loop_start = Instant::now() - last;
                was_paused = false;
            }
            let mut flags = 0u32;
            let mut time = 0i64;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(stream, 0, None, Some(&mut flags), Some(&mut time), Some(&mut sample))
                .map_err(err)?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                if !shown_any {
                    return Err("The clip has no frames".into());
                }
                reader
                    .SetCurrentPosition(&windows::core::GUID::zeroed(), &PROPVARIANT::from(0i64))
                    .map_err(err)?;
                loop_start = Instant::now();
                base = None;
                pass += 1;
                step = 0;
                continue;
            }
            let Some(sample) = sample else { continue };
            // Times are in 100 ns units from the start of the clip.
            let first = *base.get_or_insert(time);
            last = Duration::from_nanos((time - first).max(0) as u64 * 100);
            let due = loop_start + last;
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let pixels = frame(&sample, width, height, stride, rgba).map_err(err)?;
            let blur = if pass == 0 {
                let blur = crate::blur::backdrop(&pixels);
                blurs.push(blur.clone());
                blur
            } else {
                // Backwards on odd passes, forwards on even ones.
                let last = blurs.len() - 1;
                let at = step.min(last);
                blurs[if pass % 2 == 1 { last - at } else { at }].clone()
            };
            step += 1;
            show(pixels, blur);
            shown_any = true;
        }
        Ok(())
    }

    /// Copies the middle square of a sample's BGRX rows into RGBA. A cover
    /// is square, and the page rounds its corners.
    unsafe fn frame(
        sample: &IMFSample,
        width: u32,
        height: u32,
        stride: i32,
        rgba: bool,
    ) -> windows::core::Result<Pixels> {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let side = width.min(height);
        let (left, top) = ((width - side) as usize / 2, (height - side) as usize / 2);
        let mut pixels = Pixels::new(side, side);
        let out = pixels.make_mut_bytes();
        let row_bytes = side as usize * 4;
        let mut copy = |base: *const u8, pitch: isize| {
            for y in 0..side as usize {
                let row = std::slice::from_raw_parts(
                    base.offset(pitch * (y + top) as isize).add(left * 4),
                    row_bytes,
                );
                let dest = &mut out[y * row_bytes..(y + 1) * row_bytes];
                if rgba {
                    dest.copy_from_slice(row);
                    continue;
                }
                for (d, s) in dest.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
        };
        if let Ok(two_d) = buffer.cast::<IMF2DBuffer>() {
            // Lock2D gives the first row of the picture and the step between
            // rows, which is negative when rows are stored bottom up.
            let mut first: *mut u8 = std::ptr::null_mut();
            let mut pitch = 0i32;
            two_d.Lock2D(&mut first, &mut pitch)?;
            copy(first, pitch as isize);
            two_d.Unlock2D()?;
        } else {
            let mut data: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            let base = if stride < 0 {
                data.offset(-(stride as isize) * (height as isize - 1))
            } else {
                data
            };
            copy(base, stride as isize);
            buffer.Unlock()?;
        }
        Ok(pixels)
    }
}

#[cfg(not(windows))]
mod video {
    use super::Pixels;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    pub fn run(_: &Path, _: u32, _: &AtomicBool, _: &AtomicBool, _: &dyn Fn(Pixels, Pixels)) -> Result<(), String> {
        Err("Canvas plays on Windows only".into())
    }
}

/// For `--probe canvas`: looks the track up and reports each step.
pub fn probe(keys: &Keys, title: &str, artist: &str) {
    let http = crate::innertube::agent(Duration::from_secs(15));
    let token = match crate::spotify_token::web_token(&http, &keys.cookie) {
        Ok(token) => token,
        Err(message) => return println!("token: {message}"),
    };
    println!("token ok");
    let id = match track_id(&http, &keys.search(&http, &token), title, artist) {
        Ok(Some(id)) => id,
        Ok(None) => return println!("no such track on Spotify"),
        Err(message) => return println!("search: {message}"),
    };
    println!("track {id}");
    match canvas_url(&http, &token, &id) {
        Ok(Some(url)) => println!("canvas {url}"),
        Ok(None) => println!("no canvas"),
        Err(message) => println!("canvas: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_read_back() {
        let body = field(1, &field(1, b"spotify:track:abc"));
        let outer = fields(&body);
        assert_eq!(outer.len(), 1);
        let inner = fields(outer[0].1);
        assert_eq!(inner[0], (1, &b"spotify:track:abc"[..]));
    }
}
