//! In-app updates from the GitHub releases page.
//!
//! We ask GitHub for the latest release, download its `tonearm.exe`, check
//! its SHA-256 against the digest GitHub reports, then swap it in. Windows
//! lets a running exe be renamed but not overwritten, so the running file
//! moves aside to `tonearm.old` and the next start deletes it.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// The release workflow sets `TONEARM_VERSION` to the tag it builds.
/// Local builds fall back to the version in Cargo.toml.
pub const VERSION: &str = match option_env!("TONEARM_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

const REPO: &str = "Eli-Zac/tonearm";
const ASSET: &str = "tonearm.exe";

/// Set once an update is in place. `main` starts the new exe on the way out.
static RESTART: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub struct Release {
    pub version: String,
    url: String,
    size: u64,
    sha256: Option<String>,
}

/// `v1.2.3` to `[1, 2, 3]`. Anything after a `-` or `+` is ignored.
fn parts(version: &str) -> Vec<u64> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// Whether version `candidate` comes after `current`.
pub fn newer(candidate: &str, current: &str) -> bool {
    let (mut a, mut b) = (parts(candidate), parts(current));
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

/// The latest release when it is newer than this build.
pub fn check(http: &ureq::Agent) -> Result<Option<Release>, String> {
    let release = latest(http, REPO, ASSET)?;
    Ok(newer(&release.version, VERSION).then_some(release))
}

/// The latest GitHub release of `repo` and its file called `asset`.
pub fn latest(http: &ureq::Agent, repo: &str, asset: &str) -> Result<Release, String> {
    let body: serde_json::Value = http
        .get(&format!(
            "https://api.github.com/repos/{repo}/releases/latest"
        ))
        .set("User-Agent", "tonearm-updater")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "No releases are published yet.".to_string(),
            ureq::Error::Status(code, _) => format!("GitHub answered with HTTP {code}"),
            e => format!("Could not reach GitHub: {e}"),
        })?
        .into_json()
        .map_err(|e| format!("Bad answer from GitHub: {e}"))?;
    let tag = body["tag_name"].as_str().ok_or("The release has no tag")?;
    let file = body["assets"]
        .as_array()
        .and_then(|list| list.iter().find(|a| a["name"].as_str() == Some(asset)))
        .ok_or_else(|| format!("Release {tag} has no {asset}"))?;
    let asset = file;
    Ok(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: asset["browser_download_url"]
            .as_str()
            .ok_or("The release has no download link")?
            .to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
        sha256: asset["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_ascii_lowercase),
    })
}

fn paths() -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot find the running exe: {e}"))?;
    Ok((exe.with_extension("new"), exe.with_extension("old"), exe))
}

/// Downloads a release file and checks it against GitHub's digest.
pub fn download(http: &ureq::Agent, release: &Release) -> Result<Vec<u8>, String> {
    let response = http
        .get(&release.url)
        .set("User-Agent", "tonearm-updater")
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;
    let mut bytes = Vec::with_capacity(release.size as usize);
    response
        .into_reader()
        .take(256 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Download failed: {e}"))?;
    if release.size > 0 && bytes.len() as u64 != release.size {
        return Err("The download was cut short. Try again.".into());
    }
    if let Some(expected) = &release.sha256 {
        let actual: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if &actual != expected {
            return Err("The download does not match the release. Nothing was changed.".into());
        }
    }
    Ok(bytes)
}

/// Downloads and verifies the release, then puts it where the running exe
/// was. Takes effect after `restart`.
pub fn install(http: &ureq::Agent, release: &Release) -> Result<(), String> {
    let bytes = download(http, release)?;
    let (new, old, exe) = paths()?;
    let denied = |e: std::io::Error| format!("Cannot write next to the app: {e}");
    std::fs::write(&new, &bytes).map_err(denied)?;
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(&exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(denied(e));
    }
    if let Err(e) = std::fs::rename(&new, &exe) {
        // Put the old exe back so the app still starts.
        let _ = std::fs::rename(&old, &exe);
        let _ = std::fs::remove_file(&new);
        return Err(denied(e));
    }
    RESTART.store(true, Ordering::SeqCst);
    Ok(())
}

/// Closes the window. `main` then starts the new exe.
pub fn restart() {
    let _ = slint::quit_event_loop();
}

/// Called by `main` after the event loop ends.
pub fn relaunch_if_updated() {
    if !RESTART.load(Ordering::SeqCst) {
        return;
    }
    if let Ok((_, _, exe)) = paths() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

/// Deletes the exe an earlier update moved aside.
pub fn clean_up() {
    if let Ok((new, old, _)) = paths() {
        let _ = std::fs::remove_file(old);
        let _ = std::fs::remove_file(new);
    }
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn compares_versions() {
        assert!(newer("v0.1.1", "0.1.0"));
        assert!(newer("v0.2.0", "v0.1.9"));
        assert!(newer("v1.0", "0.9.9"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("v0.1.0", "v0.1.1"));
        assert!(!newer("v0.1.0", "0.1.0-dev"));
    }
}
