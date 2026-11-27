//! Stream links straight from InnerTube's `player` endpoint.
//!
//! We ask a short list of clients that need no proof-of-origin token, in
//! order, and take the first that hands over a link that opens. Anything
//! odd is a `Miss`, and the song then cannot play.
//!
//! The visionOS client goes first. It states plain links, with neither a
//! signature nor an `n` challenge to solve, and asks for no token. The
//! Samsung TV client comes next, for songs visionOS will not play, such as
//! those made for kids. Its links need the player script's challenges
//! solved, which `challenge.rs` does. Signed in, YouTube Music's web client
//! comes last: its links open only for Premium accounts, as anyone else
//! needs a PO token for them. These follow yt-dlp's choices as of
//! 2026.11.23. Android VR is left out, as it now wants sign in for every
//! video, and so is the web embedded client, which wants a PO token.

use super::challenge::{self, PlayerJs, Solved};
use crate::resolver::Stream;
use serde_json::{json, Value};

/// Why a resolve gave up. The kind is for logs and the debug command, and
/// a network failure is asked again on a fresh connection.
#[derive(Debug)]
pub enum Miss {
    /// The network failed under a request. Worth asking again on a fresh
    /// connection, as a change of network leaves kept ones dead.
    Network(String),
    /// The video wants something Tonearm cannot do: a PO token, SABR
    /// only formats, sign in for age, and so on.
    Unsupported(String),
    /// Network trouble, an odd reply, a challenge that would not solve.
    Failed(String),
}

impl std::fmt::Display for Miss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Miss::Network(why) => write!(f, "network: {why}"),
            Miss::Unsupported(why) => write!(f, "unsupported: {why}"),
            Miss::Failed(why) => write!(f, "failed: {why}"),
        }
    }
}

// ----------------------------------------------------------------- clients

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// The visionOS YouTube app. Plain links, no token, no sign in.
    Visionos,
    /// music.youtube.com's own web client, signed in.
    WebMusic,
    /// The Samsung TV app. Its links need the player's challenges solved.
    TvSamsung,
}

const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0";
const VISIONOS_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
const TV_SAMSUNG_UA: &str = "Mozilla/5.0 (SMART-TV; Linux; Tizen 2.4.0) AppleWebKit/538.1 (KHTML, like Gecko) Version/2.4.0 TV Safari/538.1";

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Visionos => "visionos",
            Kind::WebMusic => "web_music",
            Kind::TvSamsung => "tv_samsung",
        }
    }

    fn user_agent(self) -> &'static str {
        match self {
            Kind::Visionos => VISIONOS_UA,
            Kind::TvSamsung => TV_SAMSUNG_UA,
            Kind::WebMusic => BROWSER_UA,
        }
    }

    /// Whether the links come with ciphers from the web player script, so
    /// the request must name that script's `signatureTimestamp`.
    fn needs_player_js(self) -> bool {
        matches!(self, Kind::WebMusic | Kind::TvSamsung)
    }

    fn endpoint(self) -> &'static str {
        match self {
            Kind::WebMusic => "https://music.youtube.com/youtubei/v1/player?prettyPrint=false",
            _ => "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
        }
    }

    fn context(self) -> Value {
        match self {
            Kind::WebMusic => json!({"client": {
                "clientName": "WEB_REMIX",
                "clientVersion": super::CLIENT_VERSION,
                "hl": "en",
            }}),
            Kind::Visionos => json!({"client": {
                "clientName": "VISIONOS",
                "clientVersion": "1.02",
                "deviceMake": "Apple",
                "deviceModel": "RealityDevice17,1",
                "osName": "visionOS",
                "osVersion": "26.5.23O471",
                "hl": "en",
            }}),
            Kind::TvSamsung => json!({"client": {
                "clientName": "TVHTML5",
                "clientVersion": "5.20260707",
                "deviceMake": "Samsung",
                "deviceModel": "UKS9800",
                "osName": "Tizen",
                "osVersion": "2.4.0",
                "hl": "en",
            }}),
        }
    }
}

/// The clients to ask, in order.
fn order(signed_in: bool) -> Vec<Kind> {
    if signed_in {
        vec![Kind::Visionos, Kind::TvSamsung, Kind::WebMusic]
    } else {
        vec![Kind::Visionos, Kind::TvSamsung]
    }
}

// ----------------------------------------------------------------- resolve

/// Resolves a video id to an M4A audio stream. `cookie` is the raw
/// `Cookie:` header when signed in.
pub fn resolve(
    http: &ureq::Agent,
    cookie: Option<&str>,
    video_id: &str,
) -> Result<Stream, Miss> {
    let cookie = cookie.filter(|c| super::sapisid(c).is_some());
    let mut unsupported = Vec::new();
    let mut failed = Vec::new();
    let mut network = false;
    for kind in order(cookie.is_some()) {
        match attempt(http, kind, cookie, video_id) {
            Ok(stream) => return Ok(stream),
            Err(Miss::Unsupported(why)) => unsupported.push(format!("{}: {why}", kind.label())),
            Err(Miss::Failed(why)) => failed.push(format!("{}: {why}", kind.label())),
            Err(Miss::Network(why)) => {
                network = true;
                failed.push(format!("{}: network: {why}", kind.label()));
            }
        }
    }
    if failed.is_empty() {
        return Err(Miss::Unsupported(unsupported.join("; ")));
    }
    failed.extend(unsupported);
    let why = failed.join("; ");
    Err(if network { Miss::Network(why) } else { Miss::Failed(why) })
}

/// Asks one client and checks that the link it gives opens.
fn attempt(
    http: &ureq::Agent,
    kind: Kind,
    cookie: Option<&str>,
    video_id: &str,
) -> Result<Stream, Miss> {
    let player = if kind.needs_player_js() {
        Some(challenge::current_player(http).map_err(|e| Miss::Failed(format!("player script: {e}")))?)
    } else {
        None
    };
    let response = ask(http, kind, cookie, video_id, player.as_ref())?;
    let mut player = player;
    let (url, duration) = build(&response, &mut |sig, n| {
        // Ciphers in a reply from a client we sent no timestamp to are
        // rare, but they need a player all the same.
        if player.is_none() {
            player = Some(challenge::current_player(http)?);
        }
        challenge::solve(http, player.as_ref().unwrap(), sig, n)
    })?;
    let headers = vec![("User-Agent".to_string(), kind.user_agent().to_string())];
    verify(http, &url, &headers)?;
    Ok(Stream {
        url,
        duration,
        headers,
    })
}

fn ask(
    http: &ureq::Agent,
    kind: Kind,
    cookie: Option<&str>,
    video_id: &str,
    player: Option<&PlayerJs>,
) -> Result<Value, Miss> {
    let mut body = json!({
        "videoId": video_id,
        "context": kind.context(),
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    if let Some(player) = player {
        body["playbackContext"] = json!({"contentPlaybackContext": {
            "signatureTimestamp": player.sts,
        }});
    }
    let mut request = http
        .post(kind.endpoint())
        .set("User-Agent", kind.user_agent())
        .set("Accept-Language", "en");
    if kind == Kind::WebMusic {
        // Reached only when signed in, see `resolve`.
        let cookie = cookie.ok_or_else(|| Miss::Failed("no sign in".into()))?;
        let sapisid = super::sapisid(cookie).ok_or_else(|| Miss::Failed("no SAPISID".into()))?;
        request = request
            .set("Origin", super::ORIGIN)
            .set("X-Origin", super::ORIGIN)
            .set("Cookie", cookie)
            .set("Authorization", &super::sapisid_hash(sapisid))
            .set("X-Goog-AuthUser", "0");
    }
    match request.send_json(body) {
        Ok(response) => response
            .into_json()
            .map_err(|e| Miss::Failed(format!("bad response: {e}"))),
        Err(ureq::Error::Status(code, _)) => Err(Miss::Failed(format!("player answered HTTP {code}"))),
        Err(e) => Err(Miss::Network(e.to_string())),
    }
}

/// A tiny ranged GET, so a link that will not open sends us on to the next
/// client now instead of failing at playback.
fn verify(http: &ureq::Agent, url: &str, headers: &[(String, String)]) -> Result<(), Miss> {
    let mut request = http.get(url).set("Range", "bytes=0-1023");
    for (key, value) in headers {
        request = request.set(key, value);
    }
    match request.call() {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) => Err(Miss::Failed(format!("link answered HTTP {code}"))),
        Err(e) => Err(Miss::Network(format!("link check: {e}"))),
    }
}

// ----------------------------------------------------------------- parsing

/// A chosen format, before its challenges are solved.
#[derive(Debug)]
struct Pick {
    /// The link, or for a cipher the link it wraps.
    url: String,
    /// For a `signatureCipher`: the query parameter the signature goes in,
    /// and the scrambled signature.
    cipher: Option<(String, String)>,
    seconds: f32,
}

/// Reads a player reply and turns it into a ready link and its length.
/// `solve` is given the scrambled signatures and `n` values and returns
/// what they become.
fn build(
    response: &Value,
    solve: &mut dyn FnMut(&[String], &[String]) -> Result<Solved, String>,
) -> Result<(String, f32), Miss> {
    let pick = pick(response)?;
    let mut url = pick.url;
    let sigs: Vec<String> = pick.cipher.iter().map(|(_, s)| s.clone()).collect();
    let ns: Vec<String> = query_get(&url, "n").into_iter().collect();
    if !sigs.is_empty() || !ns.is_empty() {
        let solved = solve(&sigs, &ns).map_err(|e| Miss::Failed(format!("challenge: {e}")))?;
        if let Some((param, scrambled)) = &pick.cipher {
            let sig = solved
                .sig
                .get(scrambled)
                .ok_or_else(|| Miss::Failed("signature was not solved".into()))?;
            url = query_set(&url, param, sig);
        }
        if let Some(n) = ns.first() {
            // Left as is, the link is throttled to a crawl.
            let better = solved
                .n
                .get(n)
                .ok_or_else(|| Miss::Failed("n challenge was not solved".into()))?;
            url = query_set(&url, "n", better);
        }
    }
    Ok((url, pick.seconds))
}

/// M4A only, as the audio engine decodes AAC and not Opus: itag 140, then
/// the best other AAC in an MP4.
fn pick(response: &Value) -> Result<Pick, Miss> {
    let status = &response["playabilityStatus"];
    match status["status"].as_str() {
        Some("OK") => {}
        Some(other) => {
            let reason = status["reason"].as_str().unwrap_or("no reason given");
            return Err(Miss::Unsupported(format!("{other}: {reason}")));
        }
        None => return Err(Miss::Failed("reply has no playability status".into())),
    }
    let data = &response["streamingData"];
    if data.is_null() {
        return Err(Miss::Unsupported("no streaming data".into()));
    }
    let formats: Vec<&Value> = ["adaptiveFormats", "formats"]
        .iter()
        .filter_map(|k| data[*k].as_array())
        .flatten()
        .collect();
    let aac = |f: &&Value| {
        let mime = f["mimeType"].as_str().unwrap_or("");
        mime.starts_with("audio/mp4") && mime.contains("mp4a")
    };
    let openable = |f: &&Value| f["url"].is_string() || f["signatureCipher"].is_string();
    let candidates: Vec<&Value> = formats.iter().copied().filter(aac).collect();
    let chosen = candidates
        .iter()
        .copied()
        .filter(openable)
        .find(|f| f["itag"].as_u64() == Some(140))
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .filter(openable)
                .max_by_key(|f| f["bitrate"].as_u64().unwrap_or(0))
        });
    let Some(format) = chosen else {
        return Err(Miss::Unsupported(if !candidates.is_empty() || data["serverAbrStreamingUrl"].is_string() {
            "formats come as SABR only".into()
        } else {
            "no M4A audio format".into()
        }));
    };
    let seconds = format["approxDurationMs"]
        .as_str()
        .and_then(|ms| ms.parse::<f32>().ok())
        .map(|ms| ms / 1000.0)
        .or_else(|| {
            response["videoDetails"]["lengthSeconds"]
                .as_str()
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0.0);
    if let Some(url) = format["url"].as_str() {
        return Ok(Pick {
            url: url.to_string(),
            cipher: None,
            seconds,
        });
    }
    let cipher = format["signatureCipher"].as_str().unwrap_or_default();
    let (mut url, mut s, mut sp) = (None, None, None);
    for pair in cipher.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = pct_decode(value);
        match key {
            "url" => url = Some(value),
            "s" => s = Some(value),
            "sp" => sp = Some(value),
            _ => {}
        }
    }
    match (url, s) {
        (Some(url), Some(s)) => Ok(Pick {
            url,
            cipher: Some((sp.unwrap_or_else(|| "signature".into()), s)),
            seconds,
        }),
        _ => Err(Miss::Failed("odd signatureCipher".into())),
    }
}

// ------------------------------------------------------------ query strings

fn pct_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(b);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pct_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// One query parameter's decoded value.
fn query_get(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| pct_decode(value))
    })
}

/// The link with one query parameter set, in place when it is there and
/// on the end when not. The rest is left byte for byte as it was, since
/// parts of the query are signed.
fn query_set(url: &str, name: &str, value: &str) -> String {
    let encoded = format!("{name}={}", pct_encode(value));
    let Some((base, query)) = url.split_once('?') else {
        return format!("{url}?{encoded}");
    };
    let mut found = false;
    let mut pairs: Vec<String> = query
        .split('&')
        .map(|pair| {
            if pair.split_once('=').map(|(k, _)| k) == Some(name) {
                found = true;
                encoded.clone()
            } else {
                pair.to_string()
            }
        })
        .collect();
    if !found {
        pairs.push(encoded);
    }
    format!("{base}?{}", pairs.join("&"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/innertube/fixtures")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn never(_: &[String], _: &[String]) -> Result<Solved, String> {
        Err("the solver should not have been asked".into())
    }

    #[test]
    fn plain_url_takes_itag_140() {
        let (url, seconds) = build(&fixture("plain_url.json"), &mut never).unwrap();
        assert!(url.contains("itag=140"), "{url}");
        assert!((seconds - 213.089).abs() < 0.01, "{seconds}");
    }

    #[test]
    fn cipher_is_solved_into_the_link() {
        let mut asked = 0;
        let (url, _) = build(&fixture("signature_cipher.json"), &mut |sig, n| {
            asked += 1;
            assert_eq!(sig, ["SCRAMBLED_SIG=="]);
            assert_eq!(n, ["NVALUE_SCRAMBLED"]);
            Ok(Solved {
                sig: HashMap::from([("SCRAMBLED_SIG==".to_string(), "REAL/SIG".to_string())]),
                n: HashMap::from([("NVALUE_SCRAMBLED".to_string(), "fast_n".to_string())]),
            })
        })
        .unwrap();
        assert_eq!(asked, 1);
        assert!(url.contains("itag=140"));
        assert!(url.contains("&sig=REAL%2FSIG"), "{url}");
        assert!(url.contains("&n=fast_n"), "{url}");
        assert!(!url.contains("NVALUE_SCRAMBLED"));
        // Everything else is as it came.
        assert!(url.contains("&sparams=expire%2Cei%2Cip"), "{url}");
    }

    #[test]
    fn unsolved_n_is_a_failure() {
        let result = build(&fixture("signature_cipher.json"), &mut |_, _| {
            Ok(Solved {
                sig: HashMap::from([("SCRAMBLED_SIG==".to_string(), "x".to_string())]),
                n: HashMap::new(),
            })
        });
        assert!(matches!(result, Err(Miss::Failed(_))), "{result:?}");
        let result = build(&fixture("signature_cipher.json"), &mut |_, _| Err("no player".into()));
        assert!(matches!(result, Err(Miss::Failed(_))), "{result:?}");
    }

    #[test]
    fn login_required_is_unsupported() {
        let result = build(&fixture("login_required.json"), &mut never);
        match result {
            Err(Miss::Unsupported(why)) => assert!(why.contains("LOGIN_REQUIRED"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sabr_only_is_unsupported() {
        let result = build(&fixture("sabr_only.json"), &mut never);
        match result {
            Err(Miss::Unsupported(why)) => assert!(why.contains("SABR"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn query_edits_keep_the_rest() {
        assert_eq!(query_set("https://h/p?a=1&n=old&b=2", "n", "new"), "https://h/p?a=1&n=new&b=2");
        assert_eq!(query_set("https://h/p?a=1", "sig", "a/b"), "https://h/p?a=1&sig=a%2Fb");
        assert_eq!(query_get("https://h/p?a=1&n=x%20y", "n").as_deref(), Some("x y"));
        assert_eq!(pct_decode("a%2Fb%"), "a/b%");
    }

    /// Needs the network: `cargo test resolves_a_song -- --ignored --nocapture`.
    /// The visionOS client needs no challenge, so this passes without
    /// `challenge.rs`; the web clients behind it do need it.
    #[test]
    #[ignore]
    fn resolves_a_song() {
        let http = crate::innertube::agent(std::time::Duration::from_secs(20));
        let stream = resolve(&http, None, "dQw4w9WgXcQ").unwrap();
        eprintln!("{} {:?}", stream.url, stream.headers);
        assert!(stream.url.starts_with("https://"));
        assert!(stream.duration > 200.0 && stream.duration < 230.0);
        assert!(!stream.headers.is_empty());
    }
}
