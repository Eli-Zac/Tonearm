//! Keeps the settings both apps share, and the pins in Quick access, the same
//! on every device signed in to the Tonearm account.
//!
//! Each setting (and each pin) is one key on the server, and the newest
//! change to a key wins. This device remembers what it last agreed with the
//! server (`sync-state`), so a key whose local value differs from that is a
//! change made here since. A device that has never synced pulls first, so
//! joining an account takes its settings instead of overwriting them.

use crate::account;
use crate::innertube::model::Item;
use crate::quick_access::{self, QuickAccess};
use crate::store;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Settings kept in the store under their own name, the same on desktop and
/// phone. Things about one device (close to tray, menu collapsed) stay out.
const KEYS: [&str; 4] = ["crossfade", "levelling", "translate", "cover-choices"];
/// A pin is the key `pin/<page id>` holding the item as JSON.
const PIN: &str = "pin/";
const STATE: &str = "sync-state";

static LOCK: Mutex<()> = Mutex::new(());

/// Where this device and the server last agreed.
#[derive(Default)]
struct State {
    since: i64,
    base: BTreeMap<String, String>,
}

fn load_state() -> Option<State> {
    let value: Value = serde_json::from_str(&store::load(STATE)?).ok()?;
    Some(State {
        since: value["since"].as_i64().unwrap_or(0),
        base: value["base"]
            .as_object()?
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect(),
    })
}

fn save_state(state: &State) {
    let _ = store::save(STATE, &json!({ "since": state.since, "base": state.base }).to_string());
}

/// Forgets what was agreed, as signing out does.
pub(crate) fn forget() {
    store::clear(STATE);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Everything that syncs, as it is on this device.
fn read_local() -> BTreeMap<String, String> {
    let mut all = BTreeMap::new();
    for key in KEYS {
        if let Some(value) = store::load(key) {
            all.insert(key.to_string(), value);
        }
    }
    for item in QuickAccess::load().items() {
        if let Some(id) = &item.browse_id {
            all.insert(format!("{PIN}{id}"), item.to_json().to_string());
        }
    }
    all
}

/// Puts the server's value for `key` on this device (None removes it).
fn write_local(key: &str, value: Option<&str>) {
    if let Some(id) = key.strip_prefix(PIN) {
        let mut list = QuickAccess::load();
        match value.and_then(|v| serde_json::from_str::<Value>(v).ok()) {
            Some(json) => {
                let item = Item::from_json(&json);
                if quick_access::can_pin(&item) {
                    list.put(item);
                }
            }
            None => list.remove(id),
        }
        QuickAccess::save(list.serialize());
    } else if KEYS.contains(&key) {
        match value {
            Some(v) => {
                let _ = store::save(key, v);
            }
            None => store::clear(key),
        }
    }
}

/// What the server answers about a key.
struct Remote {
    key: String,
    value: Option<String>,
    updated: i64,
}

fn remotes(answer: &Value) -> Vec<Remote> {
    answer["settings"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|e| {
                    Some(Remote {
                        key: e["key"].as_str()?.to_string(),
                        value: e["value"].as_str().map(str::to_string),
                        updated: e["updated"].as_i64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Makes this device's settings and the account's the same. Returns the keys
/// that changed here, so the app can reload what it shows ("pin/..." for pins).
/// Safe to call from any thread; calls run one after another.
pub fn sync() -> Result<Vec<String>, String> {
    let token = account::token().ok_or("Not signed in")?;
    account::follow_move();
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut changed: Vec<String> = Vec::new();

    let first = load_state();
    let mut state = first.unwrap_or_default();
    let first = state.since == 0 && state.base.is_empty();

    // Apply server entries here, remembering each as agreed.
    let mut apply = |state: &mut State, entries: Vec<Remote>| {
        for remote in entries {
            if state.base.get(&remote.key) == remote.value.as_ref() && !first {
                continue;
            }
            // A new device keeps what it has when the account has deleted it.
            if first && remote.value.is_none() {
                continue;
            }
            let local = read_local();
            if local.get(&remote.key) != remote.value.as_ref() {
                write_local(&remote.key, remote.value.as_deref());
                changed.push(remote.key.clone());
            }
            match remote.value {
                Some(_) => {
                    // What it reads back as, which can differ in form from what was sent.
                    if let Some(v) = read_local().remove(&remote.key) {
                        state.base.insert(remote.key, v);
                    }
                }
                None => {
                    state.base.remove(&remote.key);
                }
            }
        }
    };

    if first {
        let answer = account::call("GET", "/v1/settings?since=0", Some(&token), None)?;
        apply(&mut state, remotes(&answer));
        state.since = answer["now"].as_i64().unwrap_or(0);
    }

    // Push what changed here.
    let local = read_local();
    let now = now_ms();
    let mut sent: BTreeMap<String, Option<String>> = BTreeMap::new();
    for key in local.keys().chain(state.base.keys()) {
        let here = local.get(key);
        if here != state.base.get(key) {
            sent.insert(key.clone(), here.cloned());
        }
    }
    if !sent.is_empty() {
        let changes: Vec<Value> = sent
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value, "updated": now }))
            .collect();
        let answer = account::call("PUT", "/v1/settings", Some(&token), Some(json!({ "changes": changes })))?;
        for remote in remotes(&answer) {
            let ours = sent.get(&remote.key);
            if ours == Some(&remote.value) && remote.updated >= now {
                // Ours stands.
                match remote.value {
                    Some(v) => state.base.insert(remote.key, v),
                    None => state.base.remove(&remote.key),
                };
            } else if ours.is_some() {
                // Someone else's change to the key is newer: theirs wins.
                state.base.remove(&remote.key);
                apply(&mut state, vec![remote]);
            }
        }
    }

    // Pull what changed elsewhere.
    let answer = account::call("GET", &format!("/v1/settings?since={}", state.since), Some(&token), None)?;
    apply(&mut state, remotes(&answer));
    state.since = answer["now"].as_i64().unwrap_or(state.since);

    save_state(&state);
    changed.sort();
    changed.dedup();
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_read() {
        let list = remotes(&json!({"settings": [
            {"key": "crossfade", "value": "4", "updated": 7},
            {"key": "levelling", "value": null, "updated": 9}
        ]}));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].value.as_deref(), Some("4"));
        assert_eq!(list[1].value, None);
        assert_eq!(list[1].updated, 9);
    }
}
