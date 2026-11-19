//! Minimal InnerTube client for music.youtube.com.
//!
//! Blocking HTTP on worker threads. No async runtime, which keeps both the
//! binary and the idle memory small.

pub mod challenge;
pub mod model;
pub mod parse;
pub mod player;

use model::*;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ORIGIN: &str = "https://music.youtube.com";
const BASE: &str = "https://music.youtube.com/youtubei/v1/";
const CLIENT_VERSION: &str = "1.20261101.01.00";
/// The iOS YouTube app version the loudness lookup speaks as.
const IOS_VERSION: &str = "20.10.4";
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

/// The `Authorization` header signed-in requests to music.youtube.com carry.
pub(crate) fn sapisid_hash(sapisid: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let digest = Sha1::digest(format!("{now} {sapisid} {ORIGIN}"));
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("SAPISIDHASH {now}_{hex}")
}

/// A client playback nonce: 16 characters the player makes up for each
/// play, from the 64 that are safe in a URL.
fn playback_nonce(video_id: &str) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut seed = now ^ video_id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    (0..16)
        .map(|_| {
            // xorshift, enough to make each play's nonce its own.
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            CHARS[(seed & 63) as usize] as char
        })
        .collect()
}

/// The SAPISID a cookie header signs requests with, if it has one.
pub(crate) fn sapisid(cookie: &str) -> Option<&str> {
    cookie_value(cookie, "__Secure-3PAPISID").or_else(|| cookie_value(cookie, "SAPISID"))
}

/// Accepts what people paste: the raw value or a whole `Cookie: ...` line.
/// A music video rather than a song: search labels it, and its subtitle
/// counts views where a song names its album or plays.
pub fn is_video(item: &Item) -> bool {
    item.video_id.is_some()
        && item
            .subtitle
            .split(" • ")
            .any(|p| p == "Video" || p.ends_with(" views"))
}

/// A title with what marks it as a video taken off, lowercased:
/// "Wisp - If not winter (Official Lyric Video)" becomes "if not winter".
fn bare_title(title: &str, artist: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let mut out = out.trim().to_lowercase();
    let artist = artist.to_lowercase();
    for sep in [" - ", " – ", " — "] {
        if let Some(rest) = out.strip_prefix(&format!("{artist}{sep}")) {
            out = rest.to_string();
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

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
            request = request
                .set("Cookie", &auth.cookie)
                .set("Authorization", &sapisid_hash(&auth.sapisid))
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

    /// Adds a song to the account's YouTube Music history, as the web
    /// player does when a song plays: the player endpoint gives the song's
    /// playback stats address, and a ping to it records the play. Does
    /// nothing when signed out. Blocks, so call it from a worker.
    pub fn add_to_history(&self, video_id: &str) -> Result<(), String> {
        let Some((cookie, sapisid)) = self
            .auth
            .read()
            .unwrap()
            .as_ref()
            .map(|a| (a.cookie.clone(), a.sapisid.clone()))
        else {
            return Ok(());
        };
        let response = self.post("player", json!({"videoId": video_id}))?;
        let url = response
            .pointer("/playbackTracking/videostatsPlaybackUrl/baseUrl")
            .and_then(Value::as_str)
            .ok_or("YouTube Music gave no address to record the play at")?;
        self.agent
            .get(url)
            .query("ver", "2")
            .query("c", "WEB_REMIX")
            .query("cpn", &playback_nonce(video_id))
            .set("User-Agent", USER_AGENT)
            .set("Origin", ORIGIN)
            .set("Cookie", &cookie)
            .set("Authorization", &sapisid_hash(&sapisid))
            .set("X-Goog-AuthUser", "0")
            .call()
            .map_err(|e| format!("Recording the play: {e}"))?;
        Ok(())
    }

    pub fn browse(&self, id: &str, params: Option<&str>) -> Result<Page, String> {
        let mut body = json!({"browseId": id});
        if let Some(p) = params {
            body["params"] = json!(p);
        }
        let mut page = parse::page(&self.post("browse", body)?);
        // A podcast's header names no type, only its author. Its id does.
        if let Some(h) = page.header.as_mut().filter(|h| h.kind == PageKind::Other) {
            if id.starts_with("MPSP") {
                h.kind = PageKind::Podcast;
                h.label = "Podcast".into();
            }
        }
        // The user's own playlists have no save button to name their
        // playlist. The browse id is the playlist id with "VL" in front.
        if let Some(h) = page.header.as_mut().filter(|h| h.playlist_id.is_none()) {
            if let Some(list) = id.strip_prefix("VL") {
                h.playlist_id = Some(list.to_string());
            }
        }
        // Album tracks do not name their album, since the page is the
        // album. Name it, so "Go to album" works on them in the queue.
        let album = page
            .header
            .as_ref()
            .filter(|h| matches!(h.kind, PageKind::Album | PageKind::Single))
            .filter(|_| id.starts_with("MPREb_"))
            .map(|h| (h.title.clone(), id.to_string()));
        // Nor their artist: the column holds a play count instead. Credit
        // the album's artist, or the player and scrobbles show "2M plays".
        let artist = page
            .header
            .as_ref()
            .filter(|h| !h.byline.is_empty())
            .map(|h| (h.byline.clone(), h.byline_browse_id.clone()));
        if let Some(album) = album {
            let tracks = page.shelves.iter_mut().filter(|s| !s.carousel);
            for item in tracks.flat_map(|s| s.items.iter_mut()) {
                if item.video_id.is_none() {
                    continue;
                }
                if item.album.is_none() {
                    item.album = Some(album.clone());
                }
                if item.artists.is_empty() {
                    if let Some(artist) = &artist {
                        item.artists = vec![artist.clone()];
                        item.subtitle = format!("{} • {}", artist.0, album.0);
                    }
                }
            }
        }
        Ok(page)
    }

    pub fn more(&self, token: &str) -> Result<Page, String> {
        Ok(parse::page(
            &self.post("browse", json!({"continuation": token}))?,
        ))
    }

    /// The song a music video is the video of, so a video started from a
    /// search or a radio plays the album track, and its radio is songs.
    /// None when the item is no video, or no song clearly matches.
    pub fn song_for(&self, item: &Item) -> Option<Item> {
        if !is_video(item) {
            return None;
        }
        let artist = item
            .artists
            .first()
            .map(|(name, _)| name.clone())
            .or_else(|| item.subtitle.split(" • ").find(|p| *p != "Video").map(str::to_string))?;
        let title = bare_title(&item.title, &artist);
        if title.is_empty() {
            return None;
        }
        let page = self.search(&format!("{title} {artist}")).ok()?;
        page.shelves
            .into_iter()
            .flat_map(|s| s.items)
            .filter(|i| i.kind == Kind::Song && i.video_id.is_some() && !is_video(i))
            .find(|i| {
                bare_title(&i.title, &artist) == title
                    && i.artists.iter().any(|(n, _)| n.eq_ignore_ascii_case(&artist))
            })
    }

    pub fn search(&self, query: &str) -> Result<Page, String> {
        let mut page = parse::page(&self.post("search", json!({"query": query}))?);
        page.title = format!("Results for \u{201c}{query}\u{201d}");
        Ok(page)
    }

    /// Search results of one kind, for the tabs on the results page: 1
    /// songs, 2 albums, 3 artists, 4 playlists, 5 podcasts. 0 is all.
    pub fn search_in(&self, query: &str, tab: usize) -> Result<Page, String> {
        // YouTube Music's filter codes, as its own search page sends them.
        let filter = match tab {
            1 => "II",
            2 => "IY",
            3 => "Ig",
            4 => "Io",
            5 => "JQ",
            _ => return self.search(query),
        };
        let params = format!("EgWKAQ{filter}AWoMEA4QChADEAQQCRAF");
        let mut page = parse::page(&self.post("search", json!({"query": query, "params": params}))?);
        page.title = format!("Results for \u{201c}{query}\u{201d}");
        Ok(page)
    }

    /// Playlists made by YouTube Music itself, such as its charts.
    pub fn search_featured(&self, query: &str) -> Result<Page, String> {
        let params = "EgeKAQQoADgBagwQDhAKEAMQBBAJEAU=";
        Ok(parse::page(&self.post("search", json!({"query": query, "params": params}))?))
    }

    /// The next page of search results. Search pages continue through the
    /// search endpoint, not browse.
    pub fn search_more(&self, token: &str) -> Result<Page, String> {
        Ok(parse::page(
            &self.post("search", json!({"continuation": token}))?,
        ))
    }

    /// What YouTube Music suggests while a search is typed: completed
    /// queries first, then a few songs, artists and albums.
    pub fn suggest(&self, input: &str) -> Result<(Vec<String>, Vec<Item>), String> {
        let response = self.post("music/get_search_suggestions", json!({"input": input}))?;
        Ok(parse::suggestions(&response))
    }

    /// The play queue for a song, a playlist, or both. With only a song we
    /// ask for its radio, which is what the web client does on a bare click.
    pub fn queue(
        &self,
        video_id: Option<&str>,
        playlist_id: Option<&str>,
    ) -> Result<(Vec<Item>, Option<String>), String> {
        self.queue_with(video_id, playlist_id, None)
    }

    /// As `queue`, with the params of a watch endpoint. They carry flags
    /// such as shuffle for an artist's shuffle button.
    pub fn queue_with(
        &self,
        video_id: Option<&str>,
        playlist_id: Option<&str>,
        params: Option<&str>,
    ) -> Result<(Vec<Item>, Option<String>), String> {
        let mut body = json!({
            "isAudioOnly": true,
            "enablePersistentPlaylistPanel": true,
            "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
        });
        if let Some(id) = video_id {
            body["videoId"] = json!(id);
        }
        if let Some(p) = params {
            body["params"] = json!(p);
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

    /// The song's measured loudness in LKFS, for volume levelling. Asked of
    /// YouTube's iOS player, the one client that states it without a sign
    /// in or a proof of being a browser. None when it gives no figure.
    pub fn loudness(&self, video_id: &str) -> Result<Option<f32>, String> {
        let body = json!({
            "videoId": video_id,
            "context": {"client": {
                "clientName": "IOS", "clientVersion": IOS_VERSION, "hl": "en"
            }},
        });
        let response: Value = self
            .agent
            .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
            .send_json(body)
            .map_err(|e| format!("Network error: {e}"))?
            .into_json()
            .map_err(|e| format!("Bad response: {e}"))?;
        let config = &response["playerConfig"]["audioConfig"];
        Ok(config["trackAbsoluteLoudnessLkfs"]
            .as_f64()
            .or(config["perceptualLoudnessDb"].as_f64())
            .map(|v| v as f32))
    }

    /// A song's credits: each role, such as "Written by", with its names
    /// joined. Empty when YouTube Music has none for it.
    pub fn credits(&self, video_id: &str) -> Result<Vec<(String, String)>, String> {
        let response = self.post("browse", json!({"browseId": format!("MPTC{video_id}")}))?;
        let text = |v: &Value| -> String {
            v["runs"]
                .as_array()
                .map(|runs| {
                    runs.iter()
                        .filter_map(|r| r["text"].as_str())
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        };
        let sections = response["onResponseReceivedActions"][0]["openPopupAction"]["popup"]
            ["dismissableDialogRenderer"]["sections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(sections
            .iter()
            .map(|s| &s["dismissableDialogContentSectionRenderer"])
            .map(|s| (text(&s["title"]), text(&s["subtitle"])))
            .filter(|(role, names)| !role.is_empty() && !names.is_empty())
            .collect())
    }

    pub fn rate(&self, video_id: &str, like: bool) -> Result<(), String> {
        let endpoint = if like { "like/like" } else { "like/removelike" };
        self.post(endpoint, json!({"target": {"videoId": video_id}}))
            .map(|_| ())
    }

    /// Saves an album or playlist to the library, or takes it out.
    pub fn rate_playlist(&self, playlist_id: &str, like: bool) -> Result<(), String> {
        let endpoint = if like { "like/like" } else { "like/removelike" };
        self.post(endpoint, json!({"target": {"playlistId": playlist_id}}))
            .map(|_| ())
    }

    /// Follows an artist's channel, or stops following it.
    pub fn subscribe(&self, channel_id: &str, follow: bool) -> Result<(), String> {
        let endpoint = if follow {
            "subscription/subscribe"
        } else {
            "subscription/unsubscribe"
        };
        self.post(endpoint, json!({"channelIds": [channel_id]}))
            .map(|_| ())
    }

    /// The playlists in the library, for "Add to playlist": title and
    /// playlist id. Liked Music and the episodes list are left out, as
    /// songs cannot be added to them by hand.
    pub fn my_playlists(&self) -> Result<Vec<(String, String)>, String> {
        let page = self.browse("FEmusic_liked_playlists", None)?;
        Ok(page
            .shelves
            .iter()
            .flat_map(|s| s.items.iter())
            .filter_map(|i| {
                let list = i.browse_id.as_deref()?.strip_prefix("VL")?;
                (list != "LM" && list != "SE").then(|| (i.title.clone(), list.to_string()))
            })
            .collect())
    }

    /// Makes a playlist, private, with these songs in it. Gives its id.
    pub fn create_playlist(&self, title: &str, video_ids: &[String]) -> Result<String, String> {
        let mut body = json!({"title": title, "description": "", "privacyStatus": "PRIVATE"});
        if !video_ids.is_empty() {
            body["videoIds"] = json!(video_ids);
        }
        let response = self.post("playlist/create", body)?;
        response
            .get("playlistId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "YouTube Music did not make the playlist".into())
    }

    pub fn delete_playlist(&self, playlist_id: &str) -> Result<(), String> {
        self.post("playlist/delete", json!({"playlistId": playlist_id}))
            .map(|_| ())
    }

    pub fn rename_playlist(&self, playlist_id: &str, title: &str) -> Result<(), String> {
        self.edit_playlist(
            playlist_id,
            json!([{"action": "ACTION_SET_PLAYLIST_NAME", "playlistName": title}]),
        )
    }

    /// Adds a song to a playlist. A song already in it is not added twice.
    pub fn add_to_playlist(&self, playlist_id: &str, video_id: &str) -> Result<(), String> {
        self.edit_playlist(
            playlist_id,
            json!([{
                "action": "ACTION_ADD_VIDEO",
                "addedVideoId": video_id,
                "dedupeOption": "DEDUPE_OPTION_SKIP"
            }]),
        )
    }

    /// Takes a song out of a playlist. `set_video_id` says which copy.
    pub fn remove_from_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
        set_video_id: &str,
    ) -> Result<(), String> {
        self.edit_playlist(
            playlist_id,
            json!([{
                "action": "ACTION_REMOVE_VIDEO",
                "removedVideoId": video_id,
                "setVideoId": set_video_id
            }]),
        )
    }

    fn edit_playlist(&self, playlist_id: &str, actions: Value) -> Result<(), String> {
        let response = self.post(
            "browse/edit_playlist",
            json!({"playlistId": playlist_id, "actions": actions}),
        )?;
        match response.get("status").and_then(Value::as_str) {
            Some("STATUS_SUCCEEDED") | None => Ok(()),
            Some(_) => Err("YouTube Music did not change the playlist".into()),
        }
    }

    /// The signed in account's name, picture and handle.
    pub fn account(&self) -> Result<Option<parse::Account>, String> {
        Ok(parse::account(
            &self.post("account/account_menu", json!({}))?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_titles_drop_video_marks() {
        assert_eq!(bare_title("If not winter (Official Lyric Video)", "Wisp"), "if not winter");
        assert_eq!(bare_title("Wisp - Your face [Official Video]", "Wisp"), "your face");
        assert_eq!(bare_title("Your face", "Wisp"), "your face");
    }

    #[test]
    fn videos_are_told_from_songs() {
        let video = Item {
            video_id: Some("a".into()),
            subtitle: "Wisp • 531K views • 6.2K likes".into(),
            ..Item::default()
        };
        let song = Item {
            video_id: Some("b".into()),
            subtitle: "Wisp • 2.7M plays".into(),
            ..Item::default()
        };
        assert!(is_video(&video));
        assert!(!is_video(&song));
    }
}
