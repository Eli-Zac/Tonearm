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

pub const DEFAULT_SERVER: &str = "https://api.tonearm.eli-zac.dev";

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
    /// "phone", "tablet" or "computer".
    pub kind: String,
    /// Make and model, as the device gave it. Empty when unknown.
    pub model: String,
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

/// What an app knows about the device it runs on and the core can't: its name
/// and model. The Android app gives them at start (`Build`); a computer's are
/// read from Windows.
static DEVICE: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

/// Tells the core this device's name and model, as the account's list shows
/// them. Call once, before signing in. Later calls change nothing.
pub fn set_device(name: &str, model: &str) {
    let _ = DEVICE.set((name.trim().to_string(), model.trim().to_string()));
}

/// "phone" or "computer", for the list's icon.
pub fn device_kind() -> &'static str {
    if cfg!(target_os = "android") { "phone" } else { "computer" }
}

/// A short name for this device in the account's list.
pub fn device_name() -> String {
    if let Some((name, _)) = DEVICE.get().filter(|(name, _)| !name.is_empty()) {
        return name.clone();
    }
    if cfg!(target_os = "android") {
        return "Phone".to_string();
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "Computer".to_string())
}

/// This device's make and model ("Samsung SM-S908E", "LENOVO 82JW"), or empty
/// when it can't be found.
pub fn device_model() -> String {
    if let Some((_, model)) = DEVICE.get().filter(|(_, model)| !model.is_empty()) {
        return model.clone();
    }
    #[cfg(windows)]
    {
        static MODEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        return MODEL.get_or_init(windows_model).clone();
    }
    #[cfg(not(windows))]
    String::new()
}

/// The computer's maker and model from its BIOS entry in the registry.
#[cfg(windows)]
fn windows_model() -> String {
    use std::os::windows::process::CommandExt;
    let read = |name: &str| -> String {
        std::process::Command::new("reg")
            .args(["query", r"HKLM\HARDWARE\DESCRIPTION\System\BIOS", "/v", name])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.split_once("REG_SZ").map(|(_, value)| value.trim().to_string()))
            })
            .unwrap_or_default()
    };
    let (maker, model) = (read("SystemManufacturer"), read("SystemProductName"));
    // Boards that never filled these in say so.
    let junk = |text: &str| {
        let lower = text.to_lowercase();
        text.is_empty() || lower.contains("o.e.m") || lower.contains("system product") || lower.contains("system manufacturer")
    };
    match (junk(&maker), junk(&model)) {
        (_, true) => String::new(),
        (true, false) => model,
        (false, false) if model.to_lowercase().starts_with(&maker.to_lowercase()) => model,
        (false, false) => format!("{maker} {model}"),
    }
}

/// What the server is told about this device when it signs in.
fn device_json(device: &str) -> Value {
    let name = if device.trim().is_empty() { device_name() } else { device.trim().to_string() };
    json!({ "device": name, "kind": device_kind(), "model": device_model() })
}

/// Asks for a sign-in code to be emailed.
pub fn request_code(email: &str) -> Result<(), String> {
    call("POST", "/v1/auth/code", None, Some(json!({ "email": email.trim() }))).map(|_| ())
}

/// Swaps the emailed code for this device's sign-in. `device` names it in the
/// account's list (`device_name()` if empty).
pub fn sign_in(email: &str, code: &str, device: &str) -> Result<Account, String> {
    let mut body = device_json(device);
    body["email"] = json!(email.trim());
    body["code"] = json!(code.trim());
    keep(&call("POST", "/v1/auth/verify", None, Some(body))?)
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
    let mut body = device_json(device);
    body["code"] = json!(code.trim());
    keep(&call("POST", "/v1/pair/claim", None, Some(body))?)
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
                        kind: d["kind"].as_str().unwrap_or("computer").to_string(),
                        model: d["model"].as_str().unwrap_or_default().to_string(),
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
        assert!(!device_name().is_empty());
        assert!(["phone", "computer"].contains(&device_kind()));
    }
}
