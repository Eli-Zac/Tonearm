//! The Spotify web player token, which the private endpoints need: the
//! library, playlist edits and Canvas.
//!
//! Spotify only trades the `sp_dc` cookie for a token alongside a TOTP
//! code. The secret behind that code is in the web player's script and
//! changes every few months. A community repository republishes each
//! version as it comes out, and we read the newest from there, so a change
//! needs no new build.
//!
//! Also the public Web API's token from a client ID and secret, which
//! search uses.

use serde_json::Value;
use sha1::{Digest, Sha1};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SECRETS_URL: &str =
    "https://raw.githubusercontent.com/xyloflake/spot-secrets-go/main/secrets/secretDict.json";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";

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

pub(crate) fn search_token(http: &ureq::Agent, id: &str, secret: &str) -> Result<String, String> {
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

/// A web player token for the signed in account, from the `sp_dc` cookie.
pub(crate) fn web_token(http: &ureq::Agent, cookie: &str) -> Result<String, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
