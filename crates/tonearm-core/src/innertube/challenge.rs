//! Solves the signature and `n` challenges in YouTube's stream links by
//! running the player's own JavaScript.
//!
//! The solving script is yt-dlp's `ejs` (https://github.com/yt-dlp/ejs),
//! release 0.8.0, vendored under `ejs/` (Unlicense, see `ejs/LICENSE`). It
//! is the minified `yt.solver.lib.min.js` and `yt.solver.core.min.js` from
//! that release's assets, unchanged. It finds the cipher functions in the
//! player script and runs them. We run it in an embedded QuickJS, the same
//! way yt-dlp does with an external `quickjs` binary. When YouTube changes
//! its player in a way 0.8.0 cannot read, update those two files.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::USER_AGENT;

const LIB_JS: &str = include_str!("ejs/yt.solver.lib.min.js");
const CORE_JS: &str = include_str!("ejs/yt.solver.core.min.js");

/// How long the current player id is trusted before asking again.
const PLAYER_TTL: Duration = Duration::from_secs(3600);
/// How many downloaded player scripts to keep on disk.
const KEEP_PLAYERS: usize = 2;
/// Stops a runaway script.
const JS_TIMEOUT: Duration = Duration::from_secs(90);
const JS_MEMORY: usize = 1 << 30;
const JS_STACK: usize = 8 << 20;

/// The web player script in use right now.
#[derive(Clone, Debug)]
pub struct PlayerJs {
    /// The player's version id, as in `/s/player/<id>/...`.
    pub id: String,
    /// The `signatureTimestamp` to send in a `player` request, so the
    /// ciphers in the reply match this player.
    pub sts: u32,
}

/// What a solve turned each challenge into.
#[derive(Default, Debug)]
pub struct Solved {
    /// Scrambled `s` value to the real signature.
    pub sig: HashMap<String, String>,
    /// `n` value to the one that lifts throttling.
    pub n: HashMap<String, String>,
}

/// The current player script. Cached; blocks on the network the first time.
pub fn current_player(http: &ureq::Agent) -> Result<PlayerJs, String> {
    static CURRENT: Mutex<Option<(Instant, PlayerJs)>> = Mutex::new(None);
    let mut slot = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, player)) = slot.as_ref() {
        if at.elapsed() < PLAYER_TTL {
            return Ok(player.clone());
        }
    }
    let result = fetch_current(http);
    match result {
        Ok(player) => {
            *slot = Some((Instant::now(), player.clone()));
            Ok(player)
        }
        // A stale id still beats none when the network hiccups.
        Err(e) => match slot.as_ref() {
            Some((_, player)) => Ok(player.clone()),
            None => Err(e),
        },
    }
}

fn fetch_current(http: &ureq::Agent) -> Result<PlayerJs, String> {
    let page = get_text(http, "https://www.youtube.com/iframe_api")?;
    let id = player_id_in(&page).ok_or("no player id in iframe_api")?;
    let js = load_js(http, &id, player_dir().as_deref())?;
    let sts = sts_in(&js).ok_or("no signatureTimestamp in player")?;
    Ok(PlayerJs { id, sts })
}

/// Solves every given challenge with that player. Blocks, so run it on a
/// worker. Results are cached per player version.
pub fn solve(
    http: &ureq::Agent,
    player: &PlayerJs,
    sig: &[String],
    n: &[String],
) -> Result<Solved, String> {
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    // One solve at a time: each holds the whole player in memory, and a
    // second caller can then reuse the first one's results.
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(Cache::default);
    if cache.id != player.id {
        *cache = Cache { id: player.id.clone(), ..Cache::default() };
    }
    let missing = |known: &HashMap<String, String>, wanted: &[String]| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for value in wanted {
            if !known.contains_key(value) && !out.contains(value) {
                out.push(value.clone());
            }
        }
        out
    };
    let need_sig = missing(&cache.sig, sig);
    let need_n = missing(&cache.n, n);
    if !need_sig.is_empty() || !need_n.is_empty() {
        let out = solve_with_player(http, &player.id, &need_sig, &need_n)?;
        cache.sig.extend(out.sig);
        cache.n.extend(out.n);
    }
    let pick = |known: &HashMap<String, String>, wanted: &[String]| {
        wanted
            .iter()
            .filter_map(|k| known.get(k).map(|v| (k.clone(), v.clone())))
            .collect()
    };
    Ok(Solved { sig: pick(&cache.sig, sig), n: pick(&cache.n, n) })
}

/// Fetches the current player and gets the solver ready for it, so the
/// first real solve is quick. Blocks; call it from a worker at startup.
pub fn warm(http: &ureq::Agent) -> Result<(), String> {
    let player = current_player(http)?;
    if let Some(dir) = player_dir() {
        if dir.join(format!("{}.pre.js", player.id)).is_file() {
            return Ok(());
        }
    }
    // Any `n` will do; solving it leaves the prepared script on disk.
    solve(http, &player, &[], &["tonearm_warm_up_n".to_string()]).map(|_| ())
}

/// Solves with the prepared player kept on disk when there is one, which
/// skips the slow part (parsing the whole player). Otherwise prepares it
/// from the raw script and keeps the result for next time.
fn solve_with_player(
    http: &ureq::Agent,
    id: &str,
    sig: &[String],
    n: &[String],
) -> Result<Solved, String> {
    let dir = player_dir();
    let pre_file = dir.as_ref().map(|d| d.join(format!("{id}.pre.js")));
    if let Some(file) = &pre_file {
        if let Ok(pre) = std::fs::read_to_string(file) {
            match run_solver(&pre, true, sig, n) {
                Ok((solved, _)) => return Ok(solved),
                // Kept by an older solver, perhaps. Redo it from the source.
                Err(_) => {
                    let _ = std::fs::remove_file(file);
                }
            }
        }
    }
    let js = load_js(http, id, dir.as_deref())?;
    let (solved, pre) = run_solver(&js, false, sig, n)?;
    if let (Some(file), Some(pre), Some(dir)) = (&pre_file, pre, &dir) {
        if std::fs::create_dir_all(dir).is_ok() {
            let tmp = file.with_extension("tmp");
            if std::fs::write(&tmp, pre).is_ok() && std::fs::rename(&tmp, file).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
            prune(dir);
        }
    }
    Ok(solved)
}

#[derive(Default)]
struct Cache {
    id: String,
    sig: HashMap<String, String>,
    n: HashMap<String, String>,
}

/// Runs the ejs solver in a fresh QuickJS. `script` is the raw player, or
/// with `prepared` the prepared one an earlier run returned. A raw player
/// also gets its prepared form back.
fn run_solver(
    script: &str,
    prepared: bool,
    sig: &[String],
    n: &[String],
) -> Result<(Solved, Option<String>), String> {
    use rquickjs::{Context, Runtime};

    let requests = serde_json::json!([
        {"type": "sig", "challenges": sig},
        {"type": "n", "challenges": n},
    ]);
    let input = if prepared {
        serde_json::json!({
            "type": "preprocessed", "preprocessed_player": script, "requests": requests,
        })
    } else {
        serde_json::json!({
            "type": "player", "player": script, "output_preprocessed": true,
            "requests": requests,
        })
    }
    .to_string();

    let runtime = Runtime::new().map_err(|e| format!("js runtime: {e}"))?;
    runtime.set_memory_limit(JS_MEMORY);
    runtime.set_max_stack_size(JS_STACK);
    let started = Instant::now();
    runtime.set_interrupt_handler(Some(Box::new(move || started.elapsed() > JS_TIMEOUT)));
    let context = Context::full(&runtime).map_err(|e| format!("js context: {e}"))?;
    // The lib script exports `lib`; the core script defines `jsc`.
    let script = format!(
        "{LIB_JS}\nObject.assign(globalThis, lib);\n{CORE_JS}\n\
         JSON.stringify(jsc(JSON.parse(__input)));"
    );
    let reply: String = context.with(|ctx| {
        ctx.globals().set("__input", input).map_err(|e| e.to_string())?;
        ctx.eval::<String, _>(script).map_err(|e| match e {
            rquickjs::Error::Exception => describe(&ctx.catch()),
            other => other.to_string(),
        })
    })?;
    let reply: serde_json::Value =
        serde_json::from_str(&reply).map_err(|e| format!("solver output: {e}"))?;
    if reply["type"] == "error" {
        return Err(format!("solver: {}", reply["error"]));
    }
    let mut solved = Solved::default();
    let responses = reply["responses"].as_array().ok_or("solver gave no responses")?;
    for (which, response) in ["sig", "n"].into_iter().zip(responses) {
        if response["type"] == "error" {
            return Err(format!("solver {which}: {}", response["error"]));
        }
        let target = if which == "sig" { &mut solved.sig } else { &mut solved.n };
        for (from, to) in response["data"].as_object().into_iter().flatten() {
            if let Some(to) = to.as_str() {
                target.insert(from.clone(), to.to_string());
            }
        }
    }
    let pre = reply["preprocessed_player"].as_str().map(str::to_string);
    Ok((solved, pre))
}

fn describe(value: &rquickjs::Value<'_>) -> String {
    if let Some(s) = value.as_string().and_then(|s| s.to_string().ok()) {
        return s;
    }
    if let Some(obj) = value.as_object() {
        let msg: Option<String> = obj.get("message").ok();
        let stack: Option<String> = obj.get("stack").ok();
        return format!("{} {}", msg.unwrap_or_default(), stack.unwrap_or_default());
    }
    "js error".into()
}

fn get_text(http: &ureq::Agent, url: &str) -> Result<String, String> {
    let response = http
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    let mut text = String::new();
    response
        .into_reader()
        .take(24 << 20)
        .read_to_string(&mut text)
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(text)
}

fn player_dir() -> Option<PathBuf> {
    Some(crate::store::data_dir()?.join("player"))
}

/// The player script for an id, from disk if kept there, else downloaded
/// (and kept when `dir` is given).
fn load_js(http: &ureq::Agent, id: &str, dir: Option<&Path>) -> Result<String, String> {
    if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("odd player id {id:?}"));
    }
    let file = dir.map(|d| d.join(format!("{id}.js")));
    if let Some(file) = &file {
        if let Ok(js) = std::fs::read_to_string(file) {
            if looks_like_player(&js) {
                return Ok(js);
            }
        }
    }
    let url = format!("https://www.youtube.com/s/player/{id}/player_ias.vflset/en_US/base.js");
    let js = get_text(http, &url)?;
    if !looks_like_player(&js) {
        return Err("player script looks wrong".into());
    }
    if let (Some(dir), Some(file)) = (dir, &file) {
        // Best effort. Not being able to keep it only costs a download.
        if std::fs::create_dir_all(dir).is_ok() {
            let tmp = file.with_extension("tmp");
            if std::fs::write(&tmp, &js).is_ok() && std::fs::rename(&tmp, file).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
            prune(dir);
        }
    }
    Ok(js)
}

fn looks_like_player(js: &str) -> bool {
    js.len() > 100_000 && js.contains("signatureTimestamp")
}

/// Deletes all but the newest few players' kept files (`<id>.js`,
/// `<id>.pre.js`, leftover `.tmp`).
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut ids: HashMap<String, std::time::SystemTime> = HashMap::new();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let id = name.split('.').next().unwrap_or("").to_string();
        if let Ok(time) = entry.metadata().and_then(|m| m.modified()) {
            let newest = ids.entry(id.clone()).or_insert(time);
            *newest = (*newest).max(time);
        }
        files.push((id, entry.path()));
    }
    let mut order: Vec<_> = ids.into_iter().collect();
    order.sort_by(|a, b| b.1.cmp(&a.1));
    let keep: Vec<String> = order.into_iter().take(KEEP_PLAYERS).map(|(id, _)| id).collect();
    for (id, path) in files {
        if !keep.contains(&id) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The 8 hex digit id in `.../player\/abcd1234\/...`.
fn player_id_in(page: &str) -> Option<String> {
    let mut rest = page;
    while let Some(at) = rest.find("player") {
        rest = &rest[at + 6..];
        let tail = rest.trim_start_matches('\\');
        let Some(tail) = tail.strip_prefix('/') else { continue };
        let id: String = tail.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        let after = tail[id.len()..].trim_start_matches('\\');
        if id.len() == 8 && after.starts_with('/') {
            return Some(id);
        }
    }
    None
}

/// The `signatureTimestamp:12345` (or `sts:12345`) in a player script.
fn sts_in(js: &str) -> Option<u32> {
    for key in ["signatureTimestamp", "sts"] {
        let mut from = 0;
        while let Some(at) = js[from..].find(key) {
            let start = from + at;
            from = start + key.len();
            let before = js[..start].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') {
                continue;
            }
            let rest = js[from..].trim_start();
            let Some(rest) = rest.strip_prefix(':') else { continue };
            let digits: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if digits.len() == 5 {
                if let Ok(v) = digits.parse() {
                    return Some(v);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> ureq::Agent {
        crate::innertube::agent(Duration::from_secs(60))
    }

    #[test]
    fn finds_player_id() {
        let page = r#"var scriptUrl = 'https:\/\/www.youtube.com\/s\/player\/74edf1a3\/www-widgetapi.vflset\/www-widgetapi.js';"#;
        assert_eq!(player_id_in(page).as_deref(), Some("74edf1a3"));
        let plain = "https://www.youtube.com/s/player/901741ab/base.js";
        assert_eq!(player_id_in(plain).as_deref(), Some("901741ab"));
        assert_eq!(player_id_in("no ids here, player/short/"), None);
    }

    #[test]
    fn finds_sts() {
        assert_eq!(sts_in("a,{signatureTimestamp:20522,b:1}"), Some(20522));
        assert_eq!(sts_in("x={sts: 20523};"), Some(20523));
        assert_eq!(sts_in("tests:12345,sts:20524"), Some(20524));
        assert_eq!(sts_in("nothing"), None);
    }

    #[test]
    fn prune_keeps_newest() {
        let dir = std::env::temp_dir().join(format!("tonearm-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (i, name) in ["a.js", "b.js", "c.js", "c.pre.js", "d.js"].iter().enumerate() {
            let path = dir.join(name);
            std::fs::write(&path, "x").unwrap();
            let time = std::time::SystemTime::now() - Duration::from_secs(100 - i as u64 * 10);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(time).unwrap();
        }
        prune(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(left, ["c.js", "c.pre.js", "d.js"]);
    }

    #[test]
    fn bad_solver_input_is_an_error_not_a_panic() {
        assert!(run_solver("not a player", false, &["abc".into()], &[]).is_err());
    }

    /// Known answers from yt-dlp's own test suite (test_ejs_integration.py)
    /// for an old player, which YouTube still serves by id.
    #[test]
    #[ignore = "needs the network"]
    fn known_player_answers() {
        let js = load_js(&agent(), "74edf1a3", None).unwrap();
        assert_eq!(sts_in(&js), Some(20522));
        let sig = "NJAJEij0EwRgIhAI0KExTgjfPk-MPM9MAdzyyPRt=BM8-XO5tm5hlMCSVpAiEAv7eP3CURqZNSPow8BXXAoazVoXgeMP7gH9BdylHCwgw=gwzz";
        let want_sig = "NJAJEij0EwRgIhAI0KExTgjfPk-MPM9MAdzyyPRt=BM8-XO5tm5hzMCSVpAiEAv7eP3CURqZNSPow8BXXAoazVoXgeMP7gH9BdylHCwgw=gwzl";
        let ns = ["IlLiA21ny7gqA2m4p37", "eabGFpsUKuWHXGh6FR4", "eabGF/ps%UK=uWHXGh6FR4"];
        let want_n = ["9nRTxrbM1f0yHg", "izmYqDEY6kl7Sg", "LACmqlhaBpiPlgE-a"];
        let started = Instant::now();
        let (out, pre) = run_solver(
            &js,
            false,
            &[sig.to_string()],
            &ns.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap();
        eprintln!("cold solve took {:?}", started.elapsed());
        // The prepared script solves the same, and quicker.
        let started = Instant::now();
        let (again, _) = run_solver(
            &pre.unwrap(),
            true,
            &[sig.to_string()],
            &ns.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap();
        eprintln!("prepared solve took {:?}", started.elapsed());
        assert_eq!(again.sig[sig], want_sig);
        assert_eq!(again.n[ns[2]], want_n[2]);
        assert_eq!(out.sig[sig], want_sig);
        for (n, want) in ns.iter().zip(want_n) {
            assert_eq!(out.n[*n], want);
        }
    }

    #[test]
    #[ignore = "needs the network"]
    fn current_player_solves() {
        let http = agent();
        let player = current_player(&http).unwrap();
        assert_eq!(player.id.len(), 8);
        assert!(player.sts > 10_000);
        let sig = "NJAJEij0EwRgIhAI0KExTgjfPk-MPM9MAdzyyPRt=BM8-XO5tm5hlMCSVpAiEAv7eP3CURqZNSPow8BXXAoazVoXgeMP7gH9BdylHCwgw=gwzz".to_string();
        let n = "IlLiA21ny7gqA2m4p37".to_string();
        let first = solve(&http, &player, &[sig.clone()], &[n.clone()]).unwrap();
        assert_ne!(first.sig[&sig], sig);
        assert_ne!(first.n[&n], n);
        // Asked again, the answers must match (this one is from the cache).
        let again = solve(&http, &player, &[sig.clone()], &[n.clone()]).unwrap();
        assert_eq!(first.sig[&sig], again.sig[&sig]);
        assert_eq!(first.n[&n], again.n[&n]);
        // A fresh run, bypassing the cache, must agree too.
        let js = load_js(&http, &player.id, None).unwrap();
        let (fresh, _) = run_solver(&js, false, &[sig.clone()], &[n.clone()]).unwrap();
        assert_eq!(first.sig[&sig], fresh.sig[&sig]);
        assert_eq!(first.n[&n], fresh.n[&n]);
    }
}
