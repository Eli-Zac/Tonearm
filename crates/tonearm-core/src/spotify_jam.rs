//! Spotify Jams, as the web player's `social-connect` service has them.
//! Read only, for now: a Jam's details from its link, and the Jam the
//! signed in account is in.
//!
//! A Jam's share link (`spotify.link/…`) leads to
//! `open.spotify.com/socialsession/<join token>`. The service answers on
//! spclient under `social-connect/v2`, with the web player token from the
//! `sp_dc` cookie. Live changes to a Jam come over Spotify's dealer
//! websocket as `social-connect/v2/session_update` messages, and the song
//! playing in it as Connect `cluster` updates.
//!
//! None of this is a published API, so a change on Spotify's side shows up
//! as an error here.

use serde_json::Value;

const SOCIAL_CONNECT: &str = "https://spclient.wg.spotify.com/social-connect/v2";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";

/// The join token in a Jam link: a `spotify.link` short link, an
/// `open.spotify.com/socialsession/…` link, or the token itself. A short
/// link is followed to where it leads.
pub fn join_token(http: &ureq::Agent, link: &str) -> Result<String, String> {
    let link = link.trim();
    if let Some(token) = token_in(link) {
        return Ok(token);
    }
    if link.contains("spotify.link/") {
        let response = http
            .get(link)
            .set("User-Agent", USER_AGENT)
            .call()
            .map_err(|e| format!("Could not open the Jam link: {e}"))?;
        // Followed redirects end on the session page; a page that sends
        // the browser on with script names it in its text.
        if let Some(token) = token_in(response.get_url()) {
            return Ok(token);
        }
        let page = response.into_string().map_err(|e| e.to_string())?;
        return token_in(&page).ok_or_else(|| "The Jam link did not lead to a Jam".into());
    }
    if !link.is_empty() && link.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(link.to_string());
    }
    Err("That is not a Jam link".into())
}

/// The token after `socialsession/`, if the text has one.
fn token_in(text: &str) -> Option<String> {
    let start = text.find("socialsession/")? + "socialsession/".len();
    let token: String = text[start..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    (!token.is_empty()).then_some(token)
}

fn get(http: &ureq::Agent, cookie: &str, path: &str) -> Result<Value, String> {
    request(http, cookie, "GET", path)
}

fn request(http: &ureq::Agent, cookie: &str, method: &str, path: &str) -> Result<Value, String> {
    let token = crate::spotify_token::web_token(http, cookie)?;
    let response = http
        .request(method, &format!("{SOCIAL_CONNECT}{path}"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .set("App-Platform", "WebPlayer")
        .set("Origin", "https://open.spotify.com")
        .set("Referer", "https://open.spotify.com/")
        .set("User-Agent", USER_AGENT)
        .set("Content-Length", "0")
        .call();
    match response {
        Ok(r) => {
            let text = r.into_string().map_err(|e| e.to_string())?;
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            serde_json::from_str(&text).map_err(|e| format!("Spotify's answer was not JSON: {e}"))
        }
        Err(ureq::Error::Status(401, _)) => {
            crate::spotify_token::forget_token();
            Err("Spotify signed Tonearm out. Copy a fresh sp_dc cookie in Settings.".into())
        }
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            Err(format!("Spotify answered {code}: {}", body.chars().take(300).collect::<String>()))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// A Jam's details by its join token: who is in it and whose it is.
pub fn info(http: &ureq::Agent, cookie: &str, join_token: &str) -> Result<Value, String> {
    get(http, cookie, &format!("/sessions/info/{join_token}"))
}

/// The Jam the signed in account is in, if any.
pub fn current(http: &ureq::Agent, cookie: &str) -> Result<Value, String> {
    get(http, cookie, "/sessions/current")
}

/// Joins a Jam by its join token. Answers with the Jam as it now stands.
/// Spotify has no way to join only to listen: `playback_control` takes
/// `control` or `listen_and_control` and refuses anything else. Not yet
/// tried for real.
pub fn join(http: &ureq::Agent, cookie: &str, join_token: &str) -> Result<Value, String> {
    request(http, cookie, "POST", &format!("/sessions/join/{join_token}?playback_control=listen_and_control&join_type=deeplinking"))
}

/// Leaves a Jam the account is in, by its session id. The path is a guess,
/// not yet tried.
pub fn leave(http: &ureq::Agent, cookie: &str, session_id: &str) -> Result<Value, String> {
    request(http, cookie, "POST", &format!("/sessions/{session_id}/leave"))
}

/// A copy with anything that would let someone else join blanked out, for
/// printing.
pub fn redacted(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let secret = k.contains("token") || k.contains("url") || k.contains("uri") || k.contains("link");
                    let v = if secret && v.is_string() { Value::String("<redacted>".into()) } else { redacted(v) };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redacted).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_token_in_a_session_link() {
        assert_eq!(
            token_in("https://open.spotify.com/socialsession/4zS4B9KZvLDUALdNyXF85E?si=x&jam=1").as_deref(),
            Some("4zS4B9KZvLDUALdNyXF85E")
        );
        assert_eq!(token_in("https://open.spotify.com/track/abc"), None);
    }

    #[test]
    fn redacts_what_lets_others_join() {
        let v = serde_json::json!({"session_id": "s", "join_session_token": "t", "session_members": [{"image_url": "u", "username": "n"}]});
        let r = redacted(&v);
        assert_eq!(r["join_session_token"], "<redacted>");
        assert_eq!(r["session_members"][0]["image_url"], "<redacted>");
        assert_eq!(r["session_members"][0]["username"], "n");
        assert_eq!(r["session_id"], "s");
    }
}
