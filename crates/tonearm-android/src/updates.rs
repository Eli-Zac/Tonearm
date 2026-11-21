//! The phone's own updates, from the GitHub releases page like the
//! desktop's: the latest release's APK, checked against the SHA-256 GitHub
//! reports. Android's installer does the installing (the app hands it the
//! file); this only finds and fetches it. Both calls block on the network.

use crate::{Tonearm, TonearmError};
use std::time::Duration;
use tonearm_core::{innertube, releases};

const REPO: &str = "Eli-Zac/tonearm";

/// The release file that is the phone app: `Tonearm-1.2.3.apk`.
fn is_apk(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("tonearm-") && name.ends_with(".apk")
}

#[uniffi::export]
impl Tonearm {
    /// The version of the latest release when it is newer than `current`
    /// (this app's version name), else an empty string.
    pub fn update_available(&self, current: String) -> Result<String, TonearmError> {
        let http = innertube::agent(Duration::from_secs(20));
        let release = releases::latest_matching(&http, REPO, is_apk)?;
        Ok(if releases::newer(&release.version, &current) { release.version } else { String::new() })
    }

    /// Downloads the latest release's APK to `path`, checked against the
    /// digest GitHub gives, and returns its version.
    pub fn download_update(&self, path: String) -> Result<String, TonearmError> {
        let http = innertube::agent(Duration::from_secs(60));
        let release = releases::latest_matching(&http, REPO, is_apk)?;
        let bytes = releases::download(&http, &release)?;
        std::fs::write(&path, bytes).map_err(|e| format!("Cannot save the update: {e}"))?;
        Ok(release.version)
    }
}

#[cfg(test)]
mod tests {
    use super::is_apk;

    #[test]
    fn picks_the_apk() {
        assert!(is_apk("Tonearm-0.11.0.apk"));
        assert!(!is_apk("Tonearm-0.11.0-setup.exe"));
        assert!(!is_apk("source.zip"));
    }
}
