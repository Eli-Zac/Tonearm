//! In-app updates from the GitHub releases page.
//!
//! We ask GitHub for the latest release, download its installer, check its
//! SHA-256 against the digest GitHub reports, and save it. Once the app has
//! closed and saved its session, `main` runs the installer silently. It waits
//! for the exe to free up, puts the new one in place, and starts it again.
//!
//! The installer is named after its version, `Tonearm-1.2.3-setup.exe`. 0.6.0
//! looked for a plain `tonearm-setup.exe`, which is still taken when a
//! release has no versioned one.

pub use crate::releases::Release;
use crate::releases::{download, latest_matching, newer};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// The release workflow sets `TONEARM_VERSION` to the tag it builds.
/// Local builds fall back to the version in Cargo.toml.
pub const VERSION: &str = match option_env!("TONEARM_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

const REPO: &str = "Eli-Zac/tonearm";

/// Whether a release file is the installer.
fn is_installer(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    (name.starts_with("tonearm-") && name.ends_with("-setup.exe")) || name == "tonearm-setup.exe"
}

/// Set once the installer is saved. `main` runs it on the way out.
static RESTART: AtomicBool = AtomicBool::new(false);

/// The latest release when it is newer than this build.
pub fn check(http: &ureq::Agent) -> Result<Option<Release>, String> {
    let release = latest_matching(http, REPO, is_installer)?;
    Ok(newer(&release.version, VERSION).then_some(release))
}

/// Where the downloaded installer waits to be run.
fn installer_path() -> PathBuf {
    std::env::temp_dir().join("tonearm-setup.exe")
}

fn paths() -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot find the running exe: {e}"))?;
    Ok((exe.with_extension("new"), exe.with_extension("old"), exe))
}

/// Downloads and verifies the installer. It runs after `restart`.
pub fn install(http: &ureq::Agent, release: &Release) -> Result<(), String> {
    let bytes = download(http, release)?;
    std::fs::write(installer_path(), &bytes)
        .map_err(|e| format!("Cannot save the installer: {e}"))?;
    RESTART.store(true, Ordering::SeqCst);
    Ok(())
}

/// Closes the window. `main` then runs the installer.
pub fn restart() {
    let _ = slint::quit_event_loop();
}

/// Called by `main` after the event loop ends and the session is saved.
/// The installer waits for this process to let go of the exe.
pub fn relaunch_if_updated() {
    if !RESTART.load(Ordering::SeqCst) {
        return;
    }
    let _ = std::process::Command::new(installer_path())
        .args(["/S", "/relaunch"])
        .spawn();
}

/// Deletes what an earlier update left behind: the exe moved aside by the
/// old updater, and the installer once it has finished.
pub fn clean_up() {
    let _ = std::fs::remove_file(installer_path());
    if let Ok((new, old, _)) = paths() {
        let _ = std::fs::remove_file(old);
        let _ = std::fs::remove_file(new);
    }
}
