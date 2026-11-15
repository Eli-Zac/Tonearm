//! Discord Rich Presence and Last.fm scrobbling.
//!
//! One background thread owns both, so slow network or a stuck Discord
//! pipe never touches the UI. Events arrive in order over a channel.
//!
//! When Spotify credentials are set, we look each track up on Spotify first
//! and report Spotify's title, artist and album. YouTube Music titles are
//! messy ("Artist - Song (Official Video)"), and Spotify's names match what
//! most Last.fm libraries already hold.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Default, PartialEq)]
pub struct Settings {
    pub discord_app: String,
    pub lastfm_key: String,
    pub lastfm_secret: String,
    pub lastfm_session: String,
    pub lastfm_user: String,
    pub spotify_id: String,
    pub spotify_secret: String,
}

const FIELDS: [&str; 7] = [
    "discord_app",
    "lastfm_key",
    "lastfm_secret",
    "lastfm_session",
    "lastfm_user",
    "spotify_id",
    "spotify_secret",
];

impl Settings {
    fn slots(&mut self) -> [&mut String; 7] {
        [
            &mut self.discord_app,
            &mut self.lastfm_key,
            &mut self.lastfm_secret,
            &mut self.lastfm_session,
            &mut self.lastfm_user,
            &mut self.spotify_id,
            &mut self.spotify_secret,
        ]
    }

    pub fn load() -> Self {
        let mut settings = Settings::default();
        let saved =
            crate::store::load("services").and_then(|s| serde_json::from_str::<Value>(&s).ok());
        if let Some(saved) = saved {
            for (name, slot) in FIELDS.iter().zip(settings.slots()) {
                *slot = saved
                    .get(*name)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
            }
        }
        settings
    }

    pub fn save(&self) -> Result<(), String> {
        let mut copy = self.clone();
        let map: serde_json::Map<String, Value> = FIELDS
            .iter()
            .zip(copy.slots())
            .map(|(name, slot)| (name.to_string(), json!(slot.as_str())))
            .collect();
        crate::store::save("services", &Value::Object(map).to_string())
    }
}

/// What we know about the playing track before any lookup.
#[derive(Clone, Default)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub seconds: f32,
    pub art: String,
}

pub enum Event {
    Configure(Settings),
    Started(Track),
    /// The track has played long enough to count.
    Scrobble,
    Paused,
    Resumed {
        position: f32,
    },
    Stopped,
}

/// The names we report. From Spotify when we found a match.
#[derive(Clone)]
struct Meta {
    title: String,
    artist: String,
    album: String,
    seconds: f32,
    art: String,
}

pub struct Services(Sender<Event>);

impl Services {
    pub fn start(settings: Settings) -> Self {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("services".into())
            .spawn(move || {
                let mut worker = Worker {
                    http: crate::innertube::agent(Duration::from_secs(8)),
                    settings,
                    spotify_token: None,
                    discord: None,
                    current: None,
                    started: 0,
                };
                for event in rx {
                    worker.handle(event);
                }
            })
            .expect("cannot start the services thread");
        Services(tx)
    }

    pub fn send(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

struct Worker {
    http: ureq::Agent,
    settings: Settings,
    spotify_token: Option<(String, Instant)>,
    discord: Option<Discord>,
    current: Option<Meta>,
    started: u64,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Worker {
    fn handle(&mut self, event: Event) {
        match event {
            Event::Configure(settings) => {
                if settings.discord_app != self.settings.discord_app {
                    self.discord = None;
                }
                if settings.spotify_id != self.settings.spotify_id
                    || settings.spotify_secret != self.settings.spotify_secret
                {
                    self.spotify_token = None;
                }
                self.settings = settings;
            }
            Event::Started(track) => {
                let meta = self.spotify(&track).unwrap_or(Meta {
                    title: track.title,
                    artist: track.artist,
                    album: String::new(),
                    seconds: track.seconds,
                    art: track.art,
                });
                self.started = unix_now();
                self.lastfm("track.updateNowPlaying", &meta, None);
                self.presence(Some(&meta), 0.0);
                self.current = Some(meta);
            }
            Event::Scrobble => {
                if let Some(meta) = self.current.clone() {
                    self.lastfm("track.scrobble", &meta, Some(self.started));
                }
            }
            Event::Paused => self.presence(None, 0.0),
            Event::Resumed { position } => {
                let meta = self.current.clone();
                self.presence(meta.as_ref(), position);
            }
            Event::Stopped => {
                self.current = None;
                self.presence(None, 0.0);
            }
        }
    }

    // ------------------------------------------------------------ Spotify

    fn spotify_token(&mut self) -> Option<String> {
        if self.settings.spotify_id.is_empty() || self.settings.spotify_secret.is_empty() {
            return None;
        }
        if let Some((token, expires)) = &self.spotify_token {
            if Instant::now() < *expires {
                return Some(token.clone());
            }
        }
        let basic = base64(
            format!(
                "{}:{}",
                self.settings.spotify_id, self.settings.spotify_secret
            )
            .as_bytes(),
        );
        let response: Value = self
            .http
            .post("https://accounts.spotify.com/api/token")
            .set("Authorization", &format!("Basic {basic}"))
            .send_form(&[("grant_type", "client_credentials")])
            .ok()?
            .into_json()
            .ok()?;
        let token = response.get("access_token")?.as_str()?.to_string();
        let life = response
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(3600);
        self.spotify_token = Some((
            token.clone(),
            Instant::now() + Duration::from_secs(life.saturating_sub(60)),
        ));
        Some(token)
    }

    fn spotify(&mut self, track: &Track) -> Option<Meta> {
        let token = self.spotify_token()?;
        let response: Value = self
            .http
            .get("https://api.spotify.com/v1/search")
            .set("Authorization", &format!("Bearer {token}"))
            .query(
                "q",
                &format!(
                    "{} {}",
                    clean_title(&track.title, &track.artist),
                    track.artist
                ),
            )
            .query("type", "track")
            .query("limit", "5")
            .call()
            .ok()?
            .into_json()
            .ok()?;
        let items = response.get("tracks")?.get("items")?.as_array()?;
        pick(items, track)
    }

    // ------------------------------------------------------------ Last.fm

    fn lastfm(&self, method: &str, meta: &Meta, timestamp: Option<u64>) {
        let s = &self.settings;
        if s.lastfm_key.is_empty() || s.lastfm_session.is_empty() {
            return;
        }
        let mut params = vec![
            ("method", method.to_string()),
            ("api_key", s.lastfm_key.clone()),
            ("sk", s.lastfm_session.clone()),
            ("artist", meta.artist.clone()),
            ("track", meta.title.clone()),
        ];
        if !meta.album.is_empty() {
            params.push(("album", meta.album.clone()));
        }
        if meta.seconds > 0.0 {
            params.push(("duration", (meta.seconds as u32).to_string()));
        }
        if let Some(t) = timestamp {
            params.push(("timestamp", t.to_string()));
        }
        let _ = lastfm_call(&self.http, &s.lastfm_secret, params);
    }

    // ------------------------------------------------------------ Discord

    fn presence(&mut self, meta: Option<&Meta>, position: f32) {
        if self.settings.discord_app.is_empty() {
            return;
        }
        if self.discord.is_none() {
            self.discord = Discord::connect(&self.settings.discord_app);
        }
        let Some(discord) = self.discord.as_mut() else {
            return;
        };
        let activity = meta.map(|m| {
            let start = unix_now().saturating_sub(position as u64);
            let mut activity = json!({
                "type": 2,
                "details": pad(&m.title),
                "state": pad(&m.artist),
                "timestamps": {"start": start * 1000},
            });
            if m.seconds > 0.0 {
                activity["timestamps"]["end"] = json!((start + m.seconds as u64) * 1000);
            }
            if m.art.starts_with("https://") {
                activity["assets"] = json!({"large_image": m.art});
                if !m.album.is_empty() {
                    activity["assets"]["large_text"] = json!(pad(&m.album));
                }
            }
            activity
        });
        let frame = json!({
            "cmd": "SET_ACTIVITY",
            "args": {"pid": std::process::id(), "activity": activity},
            "nonce": unix_now().to_string(),
        });
        if discord.send(1, &frame).is_none() {
            self.discord = None; // Discord closed. We reconnect on the next track.
        }
    }
}

/// Discord rejects text shorter than two characters.
fn pad(s: &str) -> String {
    let trimmed: String = s.chars().take(120).collect();
    if trimmed.chars().count() < 2 {
        format!("{trimmed} ")
    } else {
        trimmed
    }
}

/// "Artist - Song (Official Video)" becomes "Song".
fn clean_title(title: &str, artist: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1i32).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.trim();
    let prefix = format!("{} - ", artist.to_lowercase());
    let stripped = if out.to_lowercase().starts_with(&prefix) {
        &out[prefix.len().min(out.len())..]
    } else {
        out
    };
    if stripped.is_empty() {
        title.to_string()
    } else {
        stripped.trim().to_string()
    }
}

fn fold(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The Spotify result that is this song. We need the artist to match, and
/// we prefer the title and length to match too. No match means we report
/// the YouTube Music names rather than a wrong song.
fn pick(items: &[Value], track: &Track) -> Option<Meta> {
    let wanted_title = fold(&clean_title(&track.title, &track.artist));
    let wanted_artist = fold(&track.artist);
    let mut best: Option<(i32, Meta)> = None;
    for item in items {
        let title = item.get("name").and_then(Value::as_str).unwrap_or("");
        let artists: Vec<&str> = item
            .get("artists")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.get("name")?.as_str()).collect())
            .unwrap_or_default();
        let artist_match = artists.iter().any(|a| {
            let a = fold(a);
            !a.is_empty() && (wanted_artist.contains(&a) || a.contains(&wanted_artist))
        });
        if !artist_match && !wanted_artist.is_empty() {
            continue;
        }
        let seconds = item
            .get("duration_ms")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32
            / 1000.0;
        let folded = fold(&clean_title(title, ""));
        let mut score = 0;
        if folded == wanted_title {
            score += 4;
        } else if folded.contains(&wanted_title) || wanted_title.contains(&folded) {
            score += 2;
        } else {
            continue;
        }
        if track.seconds > 0.0 && (seconds - track.seconds).abs() <= 3.0 {
            score += 2;
        }
        let album = item.get("album");
        let art = album
            .and_then(|a| a.get("images")?.as_array()?.first()?.get("url")?.as_str())
            .unwrap_or(&track.art)
            .to_string();
        let meta = Meta {
            title: title.to_string(),
            // Spotify itself scrobbles the first credited artist.
            artist: artists
                .first()
                .copied()
                .unwrap_or(&track.artist)
                .to_string(),
            album: album
                .and_then(|a| a.get("name")?.as_str())
                .unwrap_or("")
                .to_string(),
            seconds,
            art,
        };
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, meta));
        }
    }
    best.map(|(_, meta)| meta)
}

pub fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---------------------------------------------------------------- Last.fm

/// Signs and sends one Last.fm call. The signature is the MD5 of every
/// parameter in name order, followed by the shared secret.
fn lastfm_call(
    http: &ureq::Agent,
    secret: &str,
    mut params: Vec<(&str, String)>,
) -> Result<Value, String> {
    params.sort_by(|a, b| a.0.cmp(b.0));
    let mut signature = String::new();
    for (name, value) in &params {
        signature.push_str(name);
        signature.push_str(value);
    }
    signature.push_str(secret);
    params.push(("api_sig", format!("{:x}", md5::compute(signature))));
    params.push(("format", "json".into()));
    let form: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let response = match http
        .post("https://ws.audioscrobbler.com/2.0/")
        .send_form(&form)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Err(format!("Last.fm is unreachable: {e}")),
    };
    let json: Value = response.into_json().map_err(|e| e.to_string())?;
    match json.get("message").and_then(Value::as_str) {
        Some(message) if json.get("error").is_some() => Err(format!("Last.fm: {message}")),
        _ => Ok(json),
    }
}

/// Step one of the Last.fm sign in. Returns the token and the page the
/// user must approve in their browser.
pub fn lastfm_begin(key: &str, secret: &str) -> Result<(String, String), String> {
    let http = crate::innertube::agent(Duration::from_secs(8));
    let json = lastfm_call(
        &http,
        secret,
        vec![("method", "auth.getToken".into()), ("api_key", key.into())],
    )?;
    let token = json
        .get("token")
        .and_then(Value::as_str)
        .ok_or("Last.fm sent no token")?
        .to_string();
    let url = format!("https://www.last.fm/api/auth/?api_key={key}&token={token}");
    Ok((token, url))
}

/// Step two. Returns the session key and the user name.
pub fn lastfm_finish(key: &str, secret: &str, token: &str) -> Result<(String, String), String> {
    let http = crate::innertube::agent(Duration::from_secs(8));
    let params = vec![
        ("method", "auth.getSession".into()),
        ("api_key", key.into()),
        ("token", token.into()),
    ];
    let json = lastfm_call(&http, secret, params)?;
    let session = json
        .get("session")
        .ok_or("Approve Tonearm on the Last.fm page first")?;
    let field = |name: &str| {
        session
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    Ok((field("key"), field("name")))
}

pub fn open_browser(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

// ---------------------------------------------------------------- Discord

/// Discord's local IPC. A frame is an opcode, a length, then JSON.
struct Discord(Box<dyn ReadWrite>);

trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

impl Discord {
    fn connect(app: &str) -> Option<Self> {
        let pipe = (0..10).find_map(open_pipe)?;
        let mut discord = Discord(pipe);
        discord.send(0, &json!({"v": 1, "client_id": app}))?;
        Some(discord)
    }

    /// Writes one frame and reads the reply, which keeps the pipe drained.
    fn send(&mut self, opcode: u32, body: &Value) -> Option<()> {
        let payload = body.to_string();
        let mut frame = Vec::with_capacity(payload.len() + 8);
        frame.extend_from_slice(&opcode.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(payload.as_bytes());
        self.0.write_all(&frame).ok()?;
        let mut header = [0u8; 8];
        self.0.read_exact(&mut header).ok()?;
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let mut reply = vec![0u8; length.min(64 * 1024)];
        self.0.read_exact(&mut reply).ok()?;
        // Opcode 2 means Discord closed the connection, for example on a bad application id.
        (u32::from_le_bytes([header[0], header[1], header[2], header[3]]) != 2).then_some(())
    }
}

#[cfg(windows)]
fn open_pipe(index: u32) -> Option<Box<dyn ReadWrite>> {
    let path = format!(r"\\.\pipe\discord-ipc-{index}");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .ok()?;
    Some(Box::new(file))
}

#[cfg(unix)]
fn open_pipe(index: u32) -> Option<Box<dyn ReadWrite>> {
    let folder = std::env::var("XDG_RUNTIME_DIR")
        .or_else(|_| std::env::var("TMPDIR"))
        .unwrap_or_else(|_| "/tmp".into());
    let stream =
        std::os::unix::net::UnixStream::connect(format!("{folder}/discord-ipc-{index}")).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    Some(Box::new(stream))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b"id:secret"), "aWQ6c2VjcmV0");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
    }

    #[test]
    fn spotify_pick_cleans_video_titles_and_checks_the_artist() {
        let items: Vec<Value> = serde_json::from_str(
            r#"[{"name":"Get Lucky","duration_ms":248000,"artists":[{"name":"Someone Else"}],"album":{"name":"Covers","images":[]}},
                {"name":"Get Lucky (feat. Pharrell Williams and Nile Rodgers)","duration_ms":369000,
                 "artists":[{"name":"Daft Punk"},{"name":"Pharrell Williams"}],
                 "album":{"name":"Random Access Memories","images":[{"url":"https://i.scdn.co/image/x"}]}}]"#,
        )
        .unwrap();
        let track = Track {
            title: "Daft Punk - Get Lucky (Official Audio)".into(),
            artist: "Daft Punk".into(),
            seconds: 369.0,
            art: String::new(),
        };
        let meta = pick(&items, &track).unwrap();
        assert_eq!(meta.artist, "Daft Punk");
        assert_eq!(meta.album, "Random Access Memories");
        assert_eq!(
            meta.title,
            "Get Lucky (feat. Pharrell Williams and Nile Rodgers)"
        );
    }
}
