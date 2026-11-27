//! Releases on GitHub: the latest one of a repository, a file from it
//! checked against the SHA-256 GitHub reports, and comparing versions.
//! Tonearm's own updates use it, on the computer and the phone.

use sha2::{Digest, Sha256};
use std::io::Read;

#[derive(Clone)]
pub struct Release {
    pub version: String,
    url: String,
    size: u64,
    sha256: Option<String>,
}

impl Release {
    /// Whether GitHub gave a SHA-256 to check the download against.
    pub fn has_digest(&self) -> bool {
        self.sha256.is_some()
    }
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

/// The latest GitHub release of `repo` and its file called `asset`.
pub fn latest(http: &ureq::Agent, repo: &str, asset: &str) -> Result<Release, String> {
    latest_matching(http, repo, |name| name == asset)
}

/// The latest GitHub release of `repo` and the first of its files whose name
/// `wanted` picks.
pub fn latest_matching(
    http: &ureq::Agent,
    repo: &str,
    wanted: impl Fn(&str) -> bool,
) -> Result<Release, String> {
    let body: serde_json::Value = http
        .get(&format!(
            "https://api.github.com/repos/{repo}/releases/latest"
        ))
        .set("User-Agent", "tonearm-updater")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            // GitHub also answers 404 for a private repository.
            ureq::Error::Status(404, _) => "No releases are published yet.".to_string(),
            ureq::Error::Status(code, _) => format!("GitHub answered with HTTP {code}"),
            e => format!("Could not reach GitHub: {e}"),
        })?
        .into_json()
        .map_err(|e| format!("Bad answer from GitHub: {e}"))?;
    let tag = body["tag_name"].as_str().ok_or("The release has no tag")?;
    let file = body["assets"]
        .as_array()
        .and_then(|list| {
            list.iter()
                .find(|a| a["name"].as_str().is_some_and(&wanted))
        })
        .ok_or_else(|| format!("Release {tag} has no installer"))?;
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
