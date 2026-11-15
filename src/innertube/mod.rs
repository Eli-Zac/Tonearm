//! Minimal InnerTube client for music.youtube.com.
//!
//! Blocking HTTP on worker threads. No async runtime, which keeps both the
//! binary and the idle memory small.

pub mod model;
pub mod parse;

use model::*;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ORIGIN: &str = "https://music.youtube.com";
const BASE: &str = "https://music.youtube.com/youtubei/v1/";
const CLIENT_VERSION: &str = "1.20261101.01.00";
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0";

struct Auth {
    cookie: String,
    sapisid: String,
}

pub struct Client {
    agent: ureq::Agent,
    auth: RwLock<Option<Auth>>,
}

pub fn agent(read_timeout: Duration) -> ureq::Agent {
    let tls = native_tls::TlsConnector::new().expect("system TLS is unavailable");
    ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(read_timeout)
        .try_proxy_from_env(true)
        .build()
}

/// Pulls one cookie value out of a `name=value; name=value` header.
fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    cookie.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

/// Accepts what people paste: the raw value or a whole `Cookie: ...` line.
pub fn normalize_cookie(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| l.contains("SAPISID"))?;
    let line = line
        .strip_prefix("Cookie:")
        .or_else(|| line.strip_prefix("cookie:"))
        .unwrap_or(line);
    let cookie = line.trim().to_string();
    cookie_value(&cookie, "__Secure-3PAPISID").or_else(|| cookie_value(&cookie, "SAPISID"))?;
    Some(cookie)
}

impl Client {
    pub fn new() -> Self {
        Client {
            agent: agent(Duration::from_secs(20)),
            auth: RwLock::new(None),
        }
    }

    /// Returns false when the cookie has no SAPISID in it.
    pub fn set_cookie(&self, cookie: Option<&str>) -> bool {
        let auth = cookie.and_then(|c| {
            let sapisid =
                cookie_value(c, "__Secure-3PAPISID").or_else(|| cookie_value(c, "SAPISID"))?;
            Some(Auth {
                cookie: c.to_string(),
                sapisid: sapisid.to_string(),
            })
        });
        let ok = auth.is_some();
        *self.auth.write().unwrap() = auth;
        ok
    }

    pub fn signed_in(&self) -> bool {
        self.auth.read().unwrap().is_some()
    }

    pub fn post(&self, endpoint: &str, mut body: Value) -> Result<Value, String> {
        body["context"] = json!({"client": {
            "clientName": "WEB_REMIX", "clientVersion": CLIENT_VERSION, "hl": "en"
        }});
        let url = format!("{BASE}{endpoint}?prettyPrint=false");
        let mut request = self
            .agent
            .post(&url)
            .set("User-Agent", USER_AGENT)
            .set("Origin", ORIGIN)
            .set("X-Origin", ORIGIN)
            .set("Accept-Language", "en");
        if let Some(auth) = self.auth.read().unwrap().as_ref() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let digest = Sha1::digest(format!("{now} {} {ORIGIN}", auth.sapisid));
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            request = request
                .set("Cookie", &auth.cookie)
                .set("Authorization", &format!("SAPISIDHASH {now}_{hex}"))
                .set("X-Goog-AuthUser", "0");
        }
        match request.send_json(body) {
            Ok(response) => response
                .into_json()
                .map_err(|e| format!("Bad response: {e}")),
            Err(ureq::Error::Status(401 | 403, _)) if self.signed_in() => {
                Err("YouTube Music rejected the sign in. Paste a fresh cookie in Settings.".into())
            }
            Err(ureq::Error::Status(code, _)) => {
                Err(format!("YouTube Music answered with HTTP {code}"))
            }
            Err(e) => Err(format!("Network error: {e}")),
        }
    }

    pub fn browse(&self, id: &str, params: Option<&str>) -> Result<Page, String> {
        let mut body = json!({"browseId": id});
        if let Some(p) = params {
            body["params"] = json!(p);
        }
        Ok(parse::page(&self.post("browse", body)?))
    }

    pub fn more(&self, token: &str) -> Result<Page, String> {
        Ok(parse::page(
            &self.post("browse", json!({"continuation": token}))?,
        ))
    }

    pub fn search(&self, query: &str) -> Result<Page, String> {
        let mut page = parse::page(&self.post("search", json!({"query": query}))?);
        page.title = format!("Results for \u{201c}{query}\u{201d}");
        Ok(page)
    }

    /// The play queue for a song, a playlist, or both. With only a song we
    /// ask for its radio, which is what the web client does on a bare click.
    pub fn queue(
        &self,
        video_id: Option<&str>,
        playlist_id: Option<&str>,
    ) -> Result<(Vec<Item>, Option<String>), String> {
        let mut body = json!({
            "isAudioOnly": true,
            "enablePersistentPlaylistPanel": true,
            "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
        });
        if let Some(id) = video_id {
            body["videoId"] = json!(id);
        }
        match (playlist_id, video_id) {
            (Some(list), _) => body["playlistId"] = json!(list),
            (None, Some(id)) => body["playlistId"] = json!(format!("RDAMVM{id}")),
            (None, None) => return Err("Nothing to play".into()),
        }
        let response = self.post("next", body)?;
        let lyrics = parse::lyrics_browse_id(&response);
        let items = parse::page(&response)
            .shelves
            .into_iter()
            .flat_map(|s| s.items)
            .filter(|i| i.video_id.is_some())
            .collect();
        Ok((items, lyrics))
    }

    /// Plain, untimed lyrics from the track's own Lyrics tab.
    pub fn plain_lyrics(&self, video_id: &str) -> Result<Option<String>, String> {
        let next = self.post("next", json!({"videoId": video_id, "isAudioOnly": true}))?;
        let Some(id) = parse::lyrics_browse_id(&next) else {
            return Ok(None);
        };
        Ok(parse::lyrics(
            &self.post("browse", json!({"browseId": id}))?,
        ))
    }

    pub fn rate(&self, video_id: &str, like: bool) -> Result<(), String> {
        let endpoint = if like { "like/like" } else { "like/removelike" };
        self.post(endpoint, json!({"target": {"videoId": video_id}}))
            .map(|_| ())
    }

    pub fn account_name(&self) -> Result<Option<String>, String> {
        Ok(parse::account_name(
            &self.post("account/account_menu", json!({}))?,
        ))
    }
}
