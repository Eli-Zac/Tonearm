//! Spotify Canvas: the short looping video some tracks show in place of
//! their cover.
//!
//! Canvas is not in Spotify's public Web API. The Spotify web player asks a
//! private endpoint for it, with a web player token, and gets back the
//! clip's URL. Client ID tokens are refused there (403), so Canvas needs a
//! Spotify web sign in: the `sp_dc` cookie, which Connections keeps.
//!
//! Spotify only trades the cookie for a token alongside a TOTP code. The
//! secret behind that code is in the web player's script and changes every
//! few months. A community repository republishes each version as it comes
//! out, and we read the newest from there, so a change needs no new build.
//! When anything here fails, the song simply shows its still cover.
//!
//! The clip is an MP4 loop. On Windows, Media Foundation decodes it on its
//! own thread, frame by frame, so memory stays small however long it is.

use crate::images::Pixels;
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CANVAS_URL: &str = "https://spclient.wg.spotify.com/canvaz-cache/v0/canvases";
const SECRETS_URL: &str =
    "https://raw.githubusercontent.com/xyloflake/spot-secrets-go/main/secrets/secretDict.json";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
/// The clip is decoded at this width. Wide enough for the Now playing panel.
pub const WIDTH: u32 = 400;
const MAX_CLIP: u64 = 24 * 1024 * 1024;
/// Clips kept on disk, so a song played again needs no download.
const KEEP_CLIPS: usize = 60;

// ------------------------------------------------------------------ token

/// The web player token and when it stops working.
static TOKEN: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Drops the token, after the cookie changed or was removed.
pub fn forget_token() {
    *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// A token for the public Web API from the Connections client ID and
/// secret. Search with it: Spotify answers 429 to most searches made with
/// a web player token.
static SEARCH_TOKEN: Mutex<Option<(String, Instant)>> = Mutex::new(None);

fn search_token(http: &ureq::Agent, id: &str, secret: &str) -> Result<String, String> {
    if let Some((token, until)) = SEARCH_TOKEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if Instant::now() < *until {
            return Ok(token.clone());
        }
    }
    let basic = crate::services::base64(format!("{id}:{secret}").as_bytes());
    let response: Value = http
        .post("https://accounts.spotify.com/api/token")
        .set("Authorization", &format!("Basic {basic}"))
        .send_form(&[("grant_type", "client_credentials")])
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    let token = response
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("Spotify gave no token")?
        .to_string();
    let life = response.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
    *SEARCH_TOKEN.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((token.clone(), Instant::now() + Duration::from_secs(life.saturating_sub(60))));
    Ok(token)
}

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
        search_token(http, &self.client_id, &self.client_secret).unwrap_or_else(|_| web.to_string())
    }
}

/// A web player token for the signed in account, from the `sp_dc` cookie.
fn web_token(http: &ureq::Agent, cookie: &str) -> Result<String, String> {
    if let Some((token, until)) = TOKEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if Instant::now() < *until {
            return Ok(token.clone());
        }
    }
    let (version, cipher) = newest_secret(http)?;
    let code = totp(&secret_key(&cipher), unix_now());
    let response: Value = http
        .get("https://open.spotify.com/api/token")
        .query("reason", "transport")
        .query("productType", "web-player")
        .query("totp", &code)
        .query("totpServer", &code)
        .query("totpVer", &version)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/json")
        .set("Referer", "https://open.spotify.com/")
        .set("App-Platform", "WebPlayer")
        .set("Cookie", &format!("sp_dc={cookie}"))
        .call()
        .map_err(|e| format!("Spotify refused the sign in: {e}"))?
        .into_json()
        .map_err(|e| e.to_string())?;
    if response.get("isAnonymous").and_then(Value::as_bool) == Some(true) {
        return Err("Spotify did not accept that cookie. Copy a fresh sp_dc.".into());
    }
    let token = response
        .get("accessToken")
        .and_then(Value::as_str)
        .ok_or("Spotify gave no token")?
        .to_string();
    let expires_ms = response
        .get("accessTokenExpirationTimestampMs")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let left = Duration::from_millis(expires_ms.saturating_sub(unix_now() * 1000));
    let left = left.saturating_sub(Duration::from_secs(60)).max(Duration::from_secs(60));
    *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = Some((token.clone(), Instant::now() + left));
    Ok(token)
}

/// The newest secret version and its numbers, from the community list.
fn newest_secret(http: &ureq::Agent) -> Result<(String, Vec<u32>), String> {
    let dict: Value = http
        .get(SECRETS_URL)
        .call()
        .map_err(|e| format!("Could not fetch the Spotify secret list: {e}"))?
        .into_json()
        .map_err(|e| e.to_string())?;
    let (version, numbers) = dict
        .as_object()
        .ok_or("The Spotify secret list is not a list")?
        .iter()
        .filter_map(|(k, v)| Some((k.parse::<u32>().ok()?, v.as_array()?)))
        .max_by_key(|(k, _)| *k)
        .ok_or("The Spotify secret list is empty")?;
    let cipher = numbers
        .iter()
        .filter_map(|n| n.as_u64().map(|n| n as u32))
        .collect();
    Ok((version.to_string(), cipher))
}

/// The TOTP key from the secret's numbers: each is XORed with a value from
/// its position, and the results are written out as one string of digits.
fn secret_key(cipher: &[u32]) -> Vec<u8> {
    cipher
        .iter()
        .enumerate()
        .map(|(i, n)| (n ^ ((i as u32 % 33) + 9)).to_string())
        .collect::<String>()
        .into_bytes()
}

/// RFC 6238: HMAC-SHA1 over the 30 second step, six digits.
fn totp(key: &[u8], unix: u64) -> String {
    let mac = hmac_sha1(key, &(unix / 30).to_be_bytes());
    let offset = (mac[19] & 0x0f) as usize;
    let value = u32::from_be_bytes([mac[offset], mac[offset + 1], mac[offset + 2], mac[offset + 3]])
        & 0x7fff_ffff;
    format!("{:06}", value % 1_000_000)
}

fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.iter().map(|b| b ^ byte).collect::<Vec<u8>>();
    let inner = Sha1::new()
        .chain_update(pad(0x36))
        .chain_update(message)
        .finalize();
    Sha1::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
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
    Some(dirs::data_local_dir()?.join("tonearm").join("canvas"))
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
    let token = web_token(http, &keys.cookie)?;
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
    /// Starts looping the clip at `path`, handing each frame to `show`.
    /// `failed` runs once if the clip cannot be decoded at all.
    pub fn play(
        path: PathBuf,
        show: impl Fn(Pixels) + Send + 'static,
        failed: impl FnOnce(String) + Send + 'static,
    ) -> Clip {
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let (s, p) = (stop.clone(), paused.clone());
        std::thread::Builder::new()
            .name("canvas".into())
            .spawn(move || {
                if let Err(message) = video::run(&path, &s, &p, &show) {
                    failed(message);
                }
            })
            .ok();
        Clip { stop, paused }
    }
}

#[cfg(windows)]
mod video {
    use super::{Pixels, WIDTH};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use windows::core::{Interface, HSTRING, PROPVARIANT};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    /// MF_VERSION from mfapi.h: the SDK version in the high word.
    const MF_VERSION: u32 = (0x0002 << 16) | 0x0070;

    pub fn run(
        path: &Path,
        stop: &AtomicBool,
        paused: &AtomicBool,
        show: &dyn Fn(Pixels),
    ) -> Result<(), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let started = MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| e.to_string());
            let result = started.and_then(|_| decode(path, stop, paused, show));
            let _ = MFShutdown();
            CoUninitialize();
            result
        }
    }

    unsafe fn decode(
        path: &Path,
        stop: &AtomicBool,
        paused: &AtomicBool,
        show: &dyn Fn(Pixels),
    ) -> Result<(), String> {
        let err = |e: windows::core::Error| e.to_string();
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 1).map_err(err)?;
        let attributes = attributes.ok_or("No attributes")?;
        // Lets the reader convert to RGB and scale for us.
        attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(err)?;
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
        let width = WIDTH.min(w) & !1;
        let height = ((h as u64 * width as u64 / w as u64) as u32).max(2) & !1;
        let wanted = MFCreateMediaType().map_err(err)?;
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(err)?;
        wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(err)?;
        wanted
            .SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(width) << 32) | u64::from(height))
            .map_err(err)?;
        if reader.SetCurrentMediaType(stream, None, &wanted).is_err() {
            // Some decoders will not scale. Take the full size instead.
            wanted.DeleteItem(&MF_MT_FRAME_SIZE).map_err(err)?;
            reader.SetCurrentMediaType(stream, None, &wanted).map_err(err)?;
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
                continue;
            }
            let Some(sample) = sample else { continue };
            // Times are in 100 ns units from the start of the clip.
            last = Duration::from_nanos(time.max(0) as u64 * 100);
            let due = loop_start + last;
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let pixels = frame(&sample, width, height, stride).map_err(err)?;
            show(pixels);
            shown_any = true;
        }
        Ok(())
    }

    /// Copies a sample's BGRX rows into RGBA.
    unsafe fn frame(
        sample: &IMFSample,
        width: u32,
        height: u32,
        stride: i32,
    ) -> windows::core::Result<Pixels> {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let mut pixels = Pixels::new(width, height);
        let out = pixels.make_mut_bytes();
        let row_bytes = width as usize * 4;
        let mut copy = |base: *const u8, pitch: isize| {
            for y in 0..height as usize {
                let row = std::slice::from_raw_parts(base.offset(pitch * y as isize), row_bytes);
                let dest = &mut out[y * row_bytes..(y + 1) * row_bytes];
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

    pub fn run(_: &Path, _: &AtomicBool, _: &AtomicBool, _: &dyn Fn(Pixels)) -> Result<(), String> {
        Err("Canvas plays on Windows only".into())
    }
}

/// For `--probe canvas`: looks the track up and reports each step.
pub fn probe(keys: &Keys, title: &str, artist: &str) {
    let http = crate::innertube::agent(Duration::from_secs(15));
    let token = match web_token(&http, &keys.cookie) {
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

    #[test]
    fn totp_matches_rfc_6238() {
        // RFC 6238 appendix B, SHA1, with the last six digits.
        let key = b"12345678901234567890";
        assert_eq!(totp(key, 59), "287082");
        assert_eq!(totp(key, 1111111109), "081804");
        assert_eq!(totp(key, 1234567890), "005924");
    }

    #[test]
    fn secret_numbers_become_digits() {
        // 0 ^ 9 = 9, 1 ^ 10 = 11, 2 ^ 11 = 9
        assert_eq!(secret_key(&[0, 1, 2]), b"9119".to_vec());
    }
}
