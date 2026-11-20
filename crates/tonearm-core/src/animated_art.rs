//! Apple Music's moving album covers ("motion artwork").
//!
//! Some albums on Apple Music have a short looping video in place of their
//! cover. Apple's web player reads it from its catalogue API, which wants
//! the web player's own token. That token is not secret: it sits in the
//! player's main script on music.apple.com, so we read it from there, the
//! way the page itself does, and keep it for a week.
//!
//! Finding the album takes two steps. The iTunes search, which needs no
//! token, finds albums by the artist and title YouTube Music gave us. Only
//! albums whose artist and title both match are kept, with "(Deluxe
//! Version)" and the like allowed on either side. Then one catalogue call
//! asks for those albums with their videos, and the first that has a
//! square one wins. A wrong guess would show another album's cover, which
//! is worse than the still one, so anything unsure counts as none.
//!
//! The video is an HLS stream in many sizes and two codecs. Windows decodes
//! H.264 out of the box but HEVC only with a paid add on, so we take an
//! H.264 one no larger than 1000 pixels a side. Its segments are byte
//! ranges of one fragmented MP4, which we download whole and keep on disk.
//! Canvas's player loops it.

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
const WEB_PLAYER: &str = "https://music.apple.com/us/browse";
const CATALOG: &str = "https://amp-api.music.apple.com/v1/catalog/us/albums";
/// The largest side we take. The cover is shown no bigger than this.
const MAX_SIDE: u32 = 1000;
/// The clip is decoded at this width, or its own when smaller: a little
/// over the largest the window draws the cover, about 600 pixels. Each frame
/// is copied to the window, so a bigger one costs time for nothing.
pub const WIDTH: u32 = 640;
/// A clip larger than this is passed over for a smaller size.
const MAX_CLIP: u64 = 24 * 1024 * 1024;
/// Clips kept on disk. They run to a few megabytes each.
const KEEP_CLIPS: usize = 25;
/// How long "this album has no moving cover" holds before we ask again.
const MISS_DAYS: u64 = 30;
/// How long a token from the web player is used before we read a new one.
/// They last for months; Apple refusing one also makes us read a new one.
const TOKEN_DAYS: u64 = 7;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn clips_dir() -> Option<PathBuf> {
    Some(crate::store::data_dir()?.join("motion"))
}

fn read_all(response: ureq::Response, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn get_text(http: &ureq::Agent, url: &str, limit: u64) -> Result<String, String> {
    let response = http
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| e.to_string())?;
    String::from_utf8(read_all(response, limit)?).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ token

/// The web player token and when we read it. Kept on disk too, as reading
/// it means fetching a few megabytes of script.
static TOKEN: Mutex<Option<(String, u64)>> = Mutex::new(None);

fn token_file() -> Option<PathBuf> {
    Some(clips_dir()?.join("token.json"))
}

fn token(http: &ureq::Agent) -> Result<String, String> {
    let mut slot = TOKEN.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = token_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| Some((v.get(0)?.as_str()?.to_string(), v.get(1)?.as_u64()?)));
    }
    if let Some((token, at)) = slot.as_ref() {
        if unix_now().saturating_sub(*at) < TOKEN_DAYS * 86_400 {
            return Ok(token.clone());
        }
    }
    let token = read_token(http)?;
    let now = unix_now();
    *slot = Some((token.clone(), now));
    if let Some(path) = token_file() {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, serde_json::json!([token, now]).to_string());
    }
    Ok(token)
}

/// Drops the token, after Apple refused it.
fn forget_token() {
    *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Some(path) = token_file() {
        let _ = std::fs::remove_file(path);
    }
}

/// Reads the token out of the web player's main script.
fn read_token(http: &ureq::Agent) -> Result<String, String> {
    let page = get_text(http, WEB_PLAYER, 8 * 1024 * 1024)?;
    let scripts = script_paths(&page);
    if scripts.is_empty() {
        return Err("The Apple Music page links no script".into());
    }
    for path in scripts {
        let Ok(script) = get_text(http, &format!("https://music.apple.com{path}"), 16 * 1024 * 1024)
        else {
            continue;
        };
        if let Some(token) = find_token(&script) {
            return Ok(token);
        }
    }
    Err("No token in the Apple Music script".into())
}

/// The page's main scripts, `/assets/index~….js`.
fn script_paths(page: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (at, _) in page.match_indices("/assets/") {
        let rest = &page[at..];
        let end = rest.find(['"', '\'']).unwrap_or(rest.len());
        let path = &rest[..end];
        if path.ends_with(".js") && path.contains("index") && !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
    }
    out
}

/// The first quoted JWT in the script: three base64url parts, the first
/// starting with `eyJ`, which is `{"` encoded. The script holds a few;
/// the one Apple issued to the web player says so in its payload.
fn find_token(script: &str) -> Option<String> {
    let is_part = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=');
    let mut found = Vec::new();
    for (at, _) in script.match_indices("eyJ") {
        let Some(quote) = script[..at].chars().last().filter(|c| matches!(c, '"' | '\'')) else {
            continue;
        };
        let rest = &script[at..];
        let Some(end) = rest.find(quote) else { continue };
        let candidate = &rest[..end];
        let parts: Vec<&str> = candidate.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(is_part)) {
            found.push(candidate.to_string());
        }
    }
    found
        .iter()
        .find(|t| {
            t.split('.')
                .nth(1)
                .and_then(base64url)
                .is_some_and(|payload| String::from_utf8_lossy(&payload).contains("AMPWebPlay"))
        })
        .or(found.first())
        .cloned()
}

fn base64url(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::new();
    let (mut bits, mut count) = (0u32, 0u32);
    for &c in text.trim_end_matches('=').as_bytes() {
        bits = (bits << 6) | value(c)?;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Some(out)
}

// --------------------------------------------------------------- matching

/// Letters and digits only, words split by single spaces, lower case.
fn plain(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a note like "Deluxe Version" or "Remastered" names an edition
/// of the same album. A live recording is a different album, even when it
/// calls itself an edition.
fn is_edition_note(note: &str) -> bool {
    // Not "version" alone: "(Taylor's Version)" is a new recording.
    const WORDS: [&str; 12] = [
        "deluxe", "edition", "remaster", "remastered", "expanded", "anniversary", "bonus",
        "reissue", "ep", "single", "explicit", "clean",
    ];
    let note = plain(note);
    let words: Vec<&str> = note.split(' ').collect();
    words.iter().any(|w| WORDS.contains(w)) && !words.contains(&"live")
}

/// The title without its edition notes: "Plastic Beach (Deluxe Version)"
/// and "Plastic Beach - EP" are both "Plastic Beach". Other brackets stay,
/// so "Time" and "Time (Live)" are told apart.
fn base_title(title: &str) -> String {
    let mut current = title.trim().to_string();
    loop {
        let before = current.clone();
        // A bracketed note at the end.
        if let Some(close) = current.chars().last().filter(|c| matches!(c, ')' | ']')) {
            let open = if close == ')' { '(' } else { '[' };
            if let Some(at) = current.rfind(open) {
                if is_edition_note(&current[at..]) {
                    current = current[..at].trim_end().to_string();
                }
            }
        }
        // A trailing " - EP" or " - Single".
        if let Some(at) = current.rfind(" - ") {
            if is_edition_note(&current[at + 3..]) {
                current = current[..at].trim_end().to_string();
            }
        }
        if current == before {
            break;
        }
    }
    // A title that is nothing but an edition note keeps it.
    if plain(&current).is_empty() {
        title.to_string()
    } else {
        current
    }
}

/// How well Apple's album title matches ours: 0 for the same title, 1 for
/// another edition of it, None for a different album.
fn title_match(found: &str, wanted: &str) -> Option<u8> {
    let (found_plain, wanted_plain) = (plain(found), plain(wanted));
    if found_plain.is_empty() || wanted_plain.is_empty() {
        return None;
    }
    if found_plain == wanted_plain {
        return Some(0);
    }
    (plain(&base_title(found)) == plain(&base_title(wanted))).then_some(1)
}

/// Whether Apple's artist is ours. YouTube Music lists every artist on the
/// song, "A, B & C", where Apple may name only the first, or the other way
/// round, so one may hold the other. Too short a name holds too much.
fn artist_match(found: &str, wanted: &str) -> bool {
    let squash = |s: &str| plain(s).replace(' ', "");
    let (found, wanted) = (squash(found), squash(wanted));
    if found.is_empty() || wanted.is_empty() {
        return false;
    }
    if found == wanted {
        return true;
    }
    let shorter = found.len().min(wanted.len());
    shorter >= 4 && (found.contains(&wanted) || wanted.contains(&found))
}

/// Apple's albums that are surely ours, best first, by their ids.
fn candidates(http: &ureq::Agent, album: &str, artist: &str) -> Result<Vec<String>, String> {
    let term = format!("{} {}", first_artist(artist), base_title(album));
    let response: Value = http
        .get("https://itunes.apple.com/search")
        .query("term", &term)
        .query("entity", "album")
        .query("limit", "25")
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("iTunes search: {e}"))?
        .into_json()
        .map_err(|e| e.to_string())?;
    let mut found: Vec<(u8, String)> = response
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| {
            r.get("artistName")
                .and_then(Value::as_str)
                .is_some_and(|name| artist_match(name, artist))
        })
        .filter_map(|r| {
            let rank = title_match(r.get("collectionName")?.as_str()?, album)?;
            let id = r.get("collectionId")?.as_u64()?;
            Some((rank, id.to_string()))
        })
        .collect();
    // Stable, so Apple's own order holds within each rank.
    found.sort_by_key(|(rank, _)| *rank);
    let mut ids: Vec<String> = Vec::new();
    for (_, id) in found {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.truncate(10);
    Ok(ids)
}

/// A video's title as a song's: no "(Official Video)" or "[HD]", and no
/// "Artist - " in front.
fn song_title(title: &str, artist: &str) -> String {
    // A note saying it is another recording (live, acoustic, a remix) stays:
    // without it "Everlong (Live at the Roundhouse)" would find the studio
    // "Everlong" and its album's moving cover. Notes about the upload
    // ("Official Video", "Lyrics") go.
    let mut out = String::new();
    let mut note = String::new();
    let mut depth = 0i32;
    for c in title.chars() {
        match c {
            '(' | '[' => {
                if depth == 0 {
                    note.clear();
                }
                depth += 1;
                note.push(c);
            }
            ')' | ']' => {
                depth = (depth - 1).max(0);
                note.push(c);
                if depth == 0 && is_other_recording(&note) {
                    out.push_str(&note);
                }
            }
            _ if depth == 0 => out.push(c),
            _ => note.push(c),
        }
    }
    if let Some((left, right)) = out.split_once(" - ") {
        if artist_match(left.trim(), artist) || artist_match(left.trim(), first_artist(artist)) {
            out = right.to_string();
        }
    }
    out.trim().to_string()
}

/// Whether a bracketed note in a title marks a different recording of the
/// song, which has its own album (or none on Apple), not the studio one's.
fn is_other_recording(note: &str) -> bool {
    let words = plain(note);
    let words: Vec<&str> = words.split(' ').collect();
    const MARKS: [&str; 12] = [
        "live", "acoustic", "remix", "demo", "unplugged", "instrumental",
        "session", "sessions", "cover", "rehearsal", "orchestral", "piano",
    ];
    words.iter().any(|w| MARKS.contains(w))
}

/// The albums Apple has the song on, by name and id, when sure of them.
/// Albums first, then singles, as a single's moving cover is rare.
fn song_albums(http: &ureq::Agent, title: &str, artist: &str) -> Result<Vec<(String, String)>, String> {
    let name = song_title(title, artist);
    if plain(&name).is_empty() {
        return Ok(Vec::new());
    }
    let response: Value = http
        .get("https://itunes.apple.com/search")
        .query("term", &format!("{} {name}", first_artist(artist)))
        .query("entity", "song")
        .query("limit", "25")
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("iTunes search: {e}"))?
        .into_json()
        .map_err(|e| e.to_string())?;
    let mut albums: Vec<(bool, String, String)> = response
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| {
            r.get("artistName").and_then(Value::as_str).is_some_and(|a| artist_match(a, first_artist(artist)))
                && r.get("trackName").and_then(Value::as_str).and_then(|t| title_match(t, &name)).is_some()
        })
        .filter_map(|r| {
            let album = r.get("collectionName")?.as_str()?.to_string();
            let id = r.get("collectionId")?.as_u64()?.to_string();
            let single = album.ends_with(" - Single") || plain(&base_title(&album)) == plain(&name);
            Some((single, album, id))
        })
        .collect();
    // Stable, so Apple's own order holds among albums.
    albums.sort_by_key(|(single, _, _)| *single);
    let mut out: Vec<(String, String)> = Vec::new();
    for (_, album, id) in albums {
        if !out.iter().any(|(_, known)| *known == id) {
            out.push((album, id));
        }
    }
    out.truncate(10);
    Ok(out)
}

/// The first of the song's artists, for the search: more names only make
/// the search miss.
fn first_artist(artist: &str) -> &str {
    let mut end = artist.len();
    for sep in [", ", " & ", " feat. ", " ft. ", " x "] {
        if let Some(at) = artist.find(sep) {
            end = end.min(at);
        }
    }
    artist[..end].trim()
}

/// The square video's stream for the first of the albums that has one.
fn video_url(http: &ureq::Agent, ids: &[String]) -> Result<Option<String>, String> {
    let albums = album_videos(http, ids)?;
    // Apple answers in its own order. Keep ours, best match first.
    for id in ids {
        let Some((_, videos)) = albums.iter().find(|(found, _)| found == id) else {
            continue;
        };
        let url = ["motionSquareVideo1x1", "motionDetailSquare"].iter().find_map(|kind| {
            videos.get(*kind)?.get("video")?.as_str().map(str::to_string)
        });
        if url.is_some() {
            return Ok(url);
        }
    }
    Ok(None)
}

/// Each of the albums Apple found, by id, with its `editorialVideo`: the
/// moving covers it has, by kind.
fn album_videos(http: &ureq::Agent, ids: &[String]) -> Result<Vec<(String, serde_json::Map<String, Value>)>, String> {
    for attempt in 0..2 {
        let token = token(http)?;
        let result = http
            .get(CATALOG)
            .query("ids", &ids.join(","))
            .query("extend", "editorialVideo")
            .query("platform", "web")
            .set("Authorization", &format!("Bearer {token}"))
            .set("Origin", "https://music.apple.com")
            .set("User-Agent", USER_AGENT)
            .call();
        let response: Value = match result {
            Ok(response) => response.into_json().map_err(|e| e.to_string())?,
            Err(ureq::Error::Status(401 | 403, _)) if attempt == 0 => {
                // The script has a newer token by now.
                forget_token();
                continue;
            }
            Err(e) => return Err(format!("Apple Music catalogue: {e}")),
        };
        let albums = response.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
        return Ok(albums
            .iter()
            .filter_map(|album| {
                let id = album.get("id")?.as_str()?.to_string();
                let videos = album
                    .pointer("/attributes/editorialVideo")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                Some((id, videos))
            })
            .collect());
    }
    Err("Apple Music refused its own token".into())
}

// -------------------------------------------------------------------- HLS

struct Variant {
    url: String,
    side: u32,
    bandwidth: u64,
}

/// `KEY=value` from an HLS tag's attribute list, quotes taken off.
fn attribute<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = line.split_once(':')?.1;
    while !rest.is_empty() {
        let (name, after) = rest.split_once('=')?;
        let (value, next) = if let Some(quoted) = after.strip_prefix('"') {
            let end = quoted.find('"')?;
            (&quoted[..end], quoted[end + 1..].trim_start_matches(','))
        } else {
            let end = after.find(',').unwrap_or(after.len());
            (&after[..end], after[end..].trim_start_matches(','))
        };
        if name.trim() == key {
            return Some(value);
        }
        rest = next;
    }
    None
}

/// A link in a playlist, against the playlist's own address.
fn join(base: &str, link: &str) -> String {
    if link.contains("://") {
        return link.to_string();
    }
    let base = base.split(['?', '#']).next().unwrap_or(base);
    if let Some(path) = link.strip_prefix('/') {
        let host_end = base.find("://").map(|i| i + 3).unwrap_or(0);
        let host_end = base[host_end..].find('/').map(|i| i + host_end).unwrap_or(base.len());
        return format!("{}/{path}", &base[..host_end]);
    }
    match base.rfind('/') {
        Some(at) => format!("{}/{link}", &base[..at]),
        None => link.to_string(),
    }
}

/// The H.264 streams in a master playlist, the ones we would rather play
/// first: the largest no larger than `MAX_SIDE`, and of those the leanest.
/// Larger ones come last, smallest first, for when nothing else fits.
fn variants(master: &str, master_url: &str) -> Vec<Variant> {
    let lines: Vec<&str> = master.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // I-frame streams are for scrubbing. They have no motion.
        if !line.starts_with("#EXT-X-STREAM-INF:") {
            continue;
        }
        let Some(link) = lines.get(i + 1).filter(|l| !l.starts_with('#')) else {
            continue;
        };
        let codecs = attribute(line, "CODECS").unwrap_or("");
        let h264 = !codecs.is_empty()
            && codecs.split(',').map(str::trim).all(|c| c.starts_with("avc1") || c.starts_with("mp4a"));
        if !h264 {
            continue;
        }
        let side = attribute(line, "RESOLUTION")
            .and_then(|r| r.split_once(['x', 'X']))
            .and_then(|(w, h)| Some(w.parse::<u32>().ok()?.max(h.parse::<u32>().ok()?)))
            .unwrap_or(0);
        let bandwidth = attribute(line, "AVERAGE-BANDWIDTH")
            .or_else(|| attribute(line, "BANDWIDTH"))
            .and_then(|b| b.parse().ok())
            .unwrap_or(0);
        out.push(Variant { url: join(master_url, link), side, bandwidth });
    }
    out.sort_by(|a, b| {
        let fits = |v: &Variant| v.side > 0 && v.side <= MAX_SIDE;
        fits(b)
            .cmp(&fits(a))
            .then_with(|| if fits(a) { b.side.cmp(&a.side) } else { a.side.cmp(&b.side) })
            .then(a.bandwidth.cmp(&b.bandwidth))
    });
    out
}

/// One piece of a media playlist: a link, and a byte range of it when the
/// playlist gives one.
struct Piece {
    url: String,
    range: Option<(u64, u64)>,
}

/// The pieces of a media playlist in order, its init section first, and
/// how many seconds it runs.
fn pieces(media: &str, media_url: &str) -> (Vec<Piece>, f64) {
    // "length@offset", or "length" alone to carry on from the last piece.
    let mut ends: Vec<(String, u64)> = Vec::new();
    let mut range_of = |url: &str, spec: &str| -> Option<(u64, u64)> {
        let (length, offset) = match spec.split_once('@') {
            Some((l, o)) => (l.trim().parse().ok()?, o.trim().parse().ok()?),
            None => {
                let start = ends.iter().find(|(u, _)| u == url).map(|(_, e)| *e).unwrap_or(0);
                (spec.trim().parse().ok()?, start)
            }
        };
        ends.retain(|(u, _)| u != url);
        ends.push((url.to_string(), offset + length));
        Some((offset, length))
    };
    let mut out = Vec::new();
    let mut seconds = 0.0;
    let mut pending: Option<String> = None;
    for line in media.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with("#EXT-X-MAP:") {
            if let Some(uri) = attribute(line, "URI") {
                let url = join(media_url, uri);
                let range = attribute(line, "BYTERANGE").and_then(|r| range_of(&url, r));
                out.push(Piece { url, range });
            }
        } else if let Some(spec) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            pending = Some(spec.to_string());
        } else if let Some(length) = line.strip_prefix("#EXTINF:") {
            seconds += length.split(',').next().and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
        } else if !line.starts_with('#') {
            let url = join(media_url, line);
            let range = pending.take().and_then(|r| range_of(&url, &r));
            out.push(Piece { url, range });
        }
    }
    (out, seconds)
}

/// Downloads a stream's pieces into one MP4. Apple's are byte ranges of a
/// single file, from its start, so that file is fetched once and whole.
fn download(http: &ureq::Agent, pieces: &[Piece]) -> Result<Vec<u8>, String> {
    let first = pieces.first().ok_or("The stream has no pieces")?;
    if pieces.iter().all(|p| p.url == first.url) {
        let end = pieces.iter().filter_map(|p| p.range).map(|(o, l)| o + l).max();
        let response = http.get(&first.url).call().map_err(|e| e.to_string())?;
        let bytes = read_all(response, MAX_CLIP + 1)?;
        if bytes.len() as u64 > MAX_CLIP {
            return Err("The clip is too large".into());
        }
        // The pieces must all be there, or the clip would stop short.
        if end.is_some_and(|end| (bytes.len() as u64) < end) {
            return Err("The clip came back short".into());
        }
        return Ok(bytes);
    }
    let mut out = Vec::new();
    for piece in pieces {
        let mut request = http.get(&piece.url);
        if let Some((offset, length)) = piece.range {
            request = request.set("Range", &format!("bytes={offset}-{}", offset + length.max(1) - 1));
        }
        let response = request.call().map_err(|e| e.to_string())?;
        let left = (MAX_CLIP + 1).saturating_sub(out.len() as u64);
        out.extend(read_all(response, left)?);
        if out.len() as u64 > MAX_CLIP {
            return Err("The clip is too large".into());
        }
    }
    Ok(out)
}

/// Picks a stream from the master playlist and downloads it.
fn fetch_video(http: &ureq::Agent, master_url: &str) -> Result<Option<Vec<u8>>, String> {
    let master = get_text(http, master_url, 1024 * 1024)?;
    let choices = variants(&master, master_url);
    if choices.is_empty() {
        // HEVC only, which Windows may not decode. As good as none.
        return Ok(None);
    }
    for choice in choices.iter().take(4) {
        let media = get_text(http, &choice.url, 1024 * 1024)?;
        let (pieces, seconds) = pieces(&media, &choice.url);
        // Skip a size that would run past the cap, for a smaller one.
        let guess = (choice.bandwidth as f64 * seconds / 8.0) as u64;
        if guess > MAX_CLIP {
            continue;
        }
        return download(http, &pieces).map(Some);
    }
    Ok(None)
}

// ------------------------------------------------------------------ cache

/// What each album turned out to have, kept in `lookups.json` beside the
/// clips: the clip's file name, or "" for none, and when we looked.
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

/// Finds the album's moving cover and keeps it on disk. Ok(None) means it
/// has none, or none we are sure of. Blocks, so call it from a worker.
/// Failures are not remembered, so the next song from the album tries again.
///
/// With no album, as for a song that came from a music video, the album is
/// the one Apple has `title` on.
pub fn fetch(http: &ureq::Agent, album: &str, title: &str, artist: &str) -> Result<Option<PathBuf>, String> {
    if plain(artist).is_empty() {
        return Ok(None);
    }
    let found_album;
    let album = if plain(album).is_empty() {
        match song_albums(http, title, artist)?.into_iter().next() {
            Some((name, _)) => {
                found_album = name;
                found_album.as_str()
            }
            None => return Ok(None),
        }
    } else {
        album
    };
    let key = format!("{}\u{1f}{}", plain(&base_title(album)), plain(artist));
    let known = with_seen(|seen| {
        let entry = seen.get(&key)?;
        Some((entry.get(0)?.as_str()?.to_string(), entry.get(1)?.as_u64()?))
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
    let found = look_up(http, album, title, artist)?;
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

fn look_up(http: &ureq::Agent, album: &str, title: &str, artist: &str) -> Result<Option<PathBuf>, String> {
    let mut ids = candidates(http, album, artist)?;
    if ids.is_empty() && !title.is_empty() {
        // Apple's album search is loose and can leave the album out. The
        // song's own entry names its album exactly.
        ids = song_albums(http, title, artist)?
            .into_iter()
            .filter(|(name, _)| title_match(name, album).is_some())
            .map(|(_, id)| id)
            .collect();
    }
    if ids.is_empty() {
        return Ok(None);
    }
    let Some(master) = video_url(http, &ids)? else {
        return Ok(None);
    };
    let dir = clips_dir().ok_or("No local data folder")?;
    // Named after the stream, so two albums sharing a video share a file.
    let name: String = master
        .rsplit('/')
        .next()
        .unwrap_or("clip")
        .trim_end_matches(".m3u8")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    let path = dir.join(format!("{name}.mp4"));
    if path.is_file() {
        return Ok(Some(path));
    }
    let Some(bytes) = fetch_video(http, &master)? else {
        return Ok(None);
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let partial = path.with_extension("part");
    std::fs::write(&partial, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    trim_clips(&dir);
    Ok(Some(path))
}

/// Keeps the newest clips and deletes the rest.
fn trim_clips(dir: &Path) {
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

/// For `--probe motion <album> <artist>`: looks the album up and reports
/// each step, without the cache.
pub fn probe(album: &str, artist: &str) {
    if let Some(title) = album.strip_prefix("song:") {
        let http = crate::innertube::agent(std::time::Duration::from_secs(20));
        let song = title.split_once('|').map_or(title, |(_, t)| t);
        println!("song {:?} -> {:?}", song_title(song, artist), song_albums(&http, song, artist));
        let (album, title) = title.split_once('|').map_or(("", title), |(a, t)| (a, t));
        println!("fetch -> {:?}", fetch(&http, album, title, artist));
        return;
    }
    let http = crate::innertube::agent(Duration::from_secs(20));
    println!("searching for {:?} by {:?}", base_title(album), first_artist(artist));
    let ids = match candidates(&http, album, artist) {
        Ok(ids) if ids.is_empty() => return println!("no matching album on Apple Music"),
        Ok(ids) => ids,
        Err(message) => return println!("search: {message}"),
    };
    println!("albums {ids:?}");
    match album_videos(&http, &ids) {
        Ok(albums) => {
            for (id, kinds) in albums {
                println!("album {id} has {:?}", kinds.keys().collect::<Vec<_>>());
            }
        }
        Err(message) => return println!("catalogue: {message}"),
    }
    let master = match video_url(&http, &ids) {
        Ok(Some(url)) => url,
        Ok(None) => return println!("no moving cover"),
        Err(message) => return println!("catalogue: {message}"),
    };
    println!("stream {master}");
    let Ok(text) = get_text(&http, &master, 1024 * 1024) else {
        return println!("could not read the stream");
    };
    match variants(&text, &master).first() {
        Some(v) => println!("would play {}px at {} bit/s: {}", v.side, v.bandwidth, v.url),
        None => println!("no H.264 stream"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn song_titles_keep_notes_of_another_recording() {
        assert_eq!(song_title("Everlong (Official HD Video)", "Foo Fighters"), "Everlong");
        assert_eq!(
            song_title("Everlong (Live at the Roundhouse, London)", "Foo Fighters"),
            "Everlong (Live at the Roundhouse, London)",
        );
        assert_eq!(song_title("Foo Fighters - Everlong (Acoustic)", "Foo Fighters"), "Everlong (Acoustic)");
        assert_eq!(song_title("Song [Lyrics]", "A"), "Song");
    }

    #[test]
    fn editions_match_but_other_albums_do_not() {
        assert_eq!(title_match("Plastic Beach", "Plastic Beach"), Some(0));
        assert_eq!(title_match("Plastic Beach (Deluxe Version)", "Plastic Beach"), Some(1));
        assert_eq!(title_match("Hello - Single", "Hello"), Some(1));
        assert_eq!(title_match("Time (Live)", "Time"), None);
        assert_eq!(title_match("Time Tour Live", "Time"), None);
        assert_eq!(title_match("Greatest Hits", "Hits"), None);
    }

    #[test]
    fn artists_match_loosely_but_not_too_loosely() {
        assert!(artist_match("Gorillaz", "Gorillaz, Snoop Dogg"));
        assert!(artist_match("ROSÉ & Bruno Mars", "ROSÉ"));
        assert!(!artist_match("Abba", "ABC"));
        assert!(!artist_match("X", "Xzibit"));
    }

    #[test]
    fn picks_h264_under_the_cap() {
        let master = "#EXTM3U\n\
            #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=900,BANDWIDTH=1000,CODECS=\"avc1.64001f\",RESOLUTION=960x960\nb.m3u8\n\
            #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=500,BANDWIDTH=600,CODECS=\"avc1.64001f\",RESOLUTION=960x960\na.m3u8\n\
            #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=300,BANDWIDTH=400,CODECS=\"hvc1.2.20000000.L123.B0\",RESOLUTION=960x960\nh.m3u8\n\
            #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=9000,BANDWIDTH=9900,CODECS=\"avc1.640020\",RESOLUTION=1080x1080\nbig.m3u8\n";
        let found = variants(master, "https://x.test/v/master.m3u8");
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].url, "https://x.test/v/a.m3u8");
        assert_eq!(found[2].url, "https://x.test/v/big.m3u8");
    }

    #[test]
    fn byte_ranges_read_back() {
        let media = "#EXTM3U\n#EXT-X-MAP:URI=\"c.mp4\",BYTERANGE=\"897@0\"\n#EXTINF:3.5,\t\n\
            #EXT-X-BYTERANGE:100@897\nc.mp4\n#EXTINF:3.5,\n#EXT-X-BYTERANGE:50\nc.mp4\n#EXT-X-ENDLIST\n";
        let (pieces, seconds) = pieces(media, "https://x.test/v/m.m3u8");
        assert_eq!(seconds, 7.0);
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0].range, Some((0, 897)));
        assert_eq!(pieces[2].range, Some((997, 50)));
        assert_eq!(pieces[2].url, "https://x.test/v/c.mp4");
    }

    #[test]
    fn finds_the_web_player_token() {
        // {"iss":"AMPWebPlay"} and {"iss":"other"}
        let script = r#"a="eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJvdGhlciJ9.c2ln";b="eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJBTVBXZWJQbGF5In0.c2ln""#;
        assert_eq!(
            find_token(script).as_deref(),
            Some("eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJBTVBXZWJQbGF5In0.c2ln")
        );
    }
}
