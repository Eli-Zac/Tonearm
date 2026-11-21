//! A Tonearm account: sign in with an emailed code, see and remove this
//! account's devices. The server is `server/` in this repository.
//!
//! The server's address is one constant here, `DEFAULT_SERVER`. A server that
//! moves says so in `GET /v1/config`, and every app that talks to it after
//! that follows; `TONEARM_ACCOUNT_SERVER` points a test copy elsewhere.

use crate::innertube::agent;
use crate::store;
use serde_json::{json, Value};
use std::time::Duration;

pub const DEFAULT_SERVER: &str = "https://tonearm.eli-zac.dev";

/// Where the account server is: the environment's choice, else the address
/// the server last gave, else `DEFAULT_SERVER`.
pub fn server() -> String {
    if let Ok(url) = std::env::var("TONEARM_ACCOUNT_SERVER") {
        return url.trim_end_matches('/').to_string();
    }
    store::load("account-server")
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| url.starts_with("https://"))
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

/// Who this device is signed in as.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub email: String,
    pub account_id: String,
    pub device_id: String,
}

/// One of the account's devices.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Milliseconds since 1970.
    pub created: i64,
    pub last_seen: i64,
    /// This device.
    pub current: bool,
}

/// The account this device is signed in to, if any.
pub fn current() -> Option<Account> {
    let value: Value = serde_json::from_str(&store::load("account")?).ok()?;
    store::load("account-token").filter(|t| !t.is_empty())?;
    Some(Account {
        email: value["email"].as_str()?.to_string(),
        account_id: value["account_id"].as_str()?.to_string(),
        device_id: value["device_id"].as_str()?.to_string(),
    })
}

/// A short name for this device in the account's list.
pub fn device_name() -> String {
    let kind = if cfg!(target_os = "android") { "Phone" } else { "Computer" };
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    if host.is_empty() || cfg!(target_os = "android") {
        kind.to_string()
    } else {
        format!("{kind} ({host})")
    }
}

/// Asks for a sign-in code to be emailed.
pub fn request_code(email: &str) -> Result<(), String> {
    call("POST", "/v1/auth/code", None, Some(json!({ "email": email.trim() }))).map(|_| ())
}

/// Swaps the emailed code for this device's sign-in. `device` names it in the
/// account's list (`device_name()` if empty).
pub fn sign_in(email: &str, code: &str, device: &str) -> Result<Account, String> {
    let device = if device.trim().is_empty() { device_name() } else { device.trim().to_string() };
    let answer = call(
        "POST",
        "/v1/auth/verify",
        None,
        Some(json!({ "email": email.trim(), "code": code.trim(), "device": device })),
    )?;
    keep(&answer)
}

/// A code for a new device to join this account with, shown as `XXXX-XXXX`
/// (and as a QR code by the apps). Works once, for `expires_in` seconds.
pub struct PairCode {
    pub code: String,
    pub expires_in: u32,
}

/// Makes a pairing code. This device must be signed in.
pub fn pair_start() -> Result<PairCode, String> {
    let token = store::load("account-token").ok_or("Not signed in")?;
    let answer = call("POST", "/v1/pair/start", Some(&token), Some(json!({})))?;
    Ok(PairCode {
        code: answer["code"].as_str().ok_or("Unexpected answer from the server")?.to_string(),
        expires_in: answer["expires_in"].as_u64().unwrap_or(300) as u32,
    })
}

/// Joins an account with a code another device made, typed or scanned.
pub fn pair_claim(code: &str, device: &str) -> Result<Account, String> {
    let device = if device.trim().is_empty() { device_name() } else { device.trim().to_string() };
    keep(&call("POST", "/v1/pair/claim", None, Some(json!({ "code": code.trim(), "device": device })))?)
}

/// Saves a sign-in the server gave.
fn keep(answer: &Value) -> Result<Account, String> {
    let text = |key: &str| answer[key].as_str().map(str::to_string).ok_or("Unexpected answer from the server");
    let account = Account {
        email: text("email")?,
        account_id: text("account_id")?,
        device_id: text("device_id")?,
    };
    store::save("account-token", &text("token")?)?;
    store::save(
        "account",
        &json!({ "email": account.email, "account_id": account.account_id, "device_id": account.device_id }).to_string(),
    )?;
    Ok(account)
}

/// Signs this device out, here and at the server (which can fail silently:
/// being signed out here is what matters).
pub fn sign_out() {
    if let Some(token) = store::load("account-token") {
        let _ = call("POST", "/v1/auth/logout", Some(&token), Some(json!({})));
    }
    forget();
}

/// The account's devices, this one marked.
pub fn devices() -> Result<Vec<Device>, String> {
    let token = store::load("account-token").ok_or("Not signed in")?;
    follow_move();
    let answer = call("GET", "/v1/devices", Some(&token), None)?;
    let current = answer["current"].as_str().unwrap_or_default().to_string();
    Ok(answer["devices"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|d| {
                    let id = d["id"].as_str()?.to_string();
                    Some(Device {
                        current: id == current,
                        id,
                        name: d["name"].as_str().unwrap_or("Device").to_string(),
                        created: d["created"].as_i64().unwrap_or(0),
                        last_seen: d["last_seen"].as_i64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Signs another device out of the account. Removing this one signs it out.
pub fn remove_device(id: &str) -> Result<(), String> {
    let token = store::load("account-token").ok_or("Not signed in")?;
    call("DELETE", &format!("/v1/devices/{id}"), Some(&token), None)?;
    if current().is_some_and(|a| a.device_id == id) {
        forget();
    }
    Ok(())
}

/// The sign-in token, for the other account calls (settings sync).
pub(crate) fn token() -> Option<String> {
    store::load("account-token").filter(|t| !t.is_empty())
}

/// An authorised or open call to the server, answering its JSON. A 401 on an
/// authorised call means the account dropped this device: sign out here too.
pub(crate) fn call(method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> Result<Value, String> {
    let url = format!("{}{path}", server());
    let mut request = agent(Duration::from_secs(20)).request(method, &url);
    if let Some(token) = token {
        request = request.set("authorization", &format!("Bearer {token}"));
    }
    let result = match body {
        Some(body) => request.send_json(body),
        None => request.call(),
    };
    match result {
        Ok(response) => response.into_json::<Value>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, response)) => {
            if code == 401 && token.is_some() {
                forget();
            }
            let message = response
                .into_json::<Value>()
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string));
            Err(message.unwrap_or_else(|| format!("The server answered {code}")))
        }
        Err(e) => Err(format!("Couldn't reach the Tonearm server: {e}")),
    }
}

/// Forgets the account here only.
fn forget() {
    store::clear("account-token");
    store::clear("account");
}

/// If the server says it lives elsewhere now, remembers that.
fn follow_move() {
    let Ok(config) = call("GET", "/v1/config", None, None) else { return };
    let Some(host) = config["host"].as_str() else { return };
    let url = format!("https://{}", host.trim_matches('/'));
    let valid = host.contains('.') && !host.contains(['/', ' ', '@', ':']);
    if valid && url != server() {
        let _ = store::save("account-server", &url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_defaults_to_the_constant() {
        if std::env::var("TONEARM_ACCOUNT_SERVER").is_err() {
            assert!(server().starts_with("https://"));
        }
    }

    #[test]
    fn a_device_name_says_what_it_is() {
        assert!(device_name().starts_with("Computer") || device_name() == "Phone");
    }
}
