//! Checking GitHub for a newer release, and installing one.
//!
//! # Why this is not silent
//!
//! "Automatic update" is two things, and only one of them should happen without
//! being asked. Checking is harmless and happens on its own. Downloading an
//! executable and running it is neither harmless nor reversible, so it needs a
//! person to press something — the same standard every other write in this app
//! is held to. An app that silently replaces its own binary is indistinguishable
//! from one that has been compromised into doing so.
//!
//! # What is trusted
//!
//! The repository is a compile-time constant, the asset must come from GitHub's
//! own hosts, and the downloaded size must match what the release metadata
//! promised. That is not a substitute for a code signature — this build is
//! unsigned, and it is worth saying so plainly rather than implying a chain of
//! trust that does not exist.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Where releases come from. A constant, so a redirected or user-supplied URL
/// cannot become the source of something this app executes.
const REPO: &str = "christianOrona/wtfault-scanner";

/// Hosts a release asset may be served from.
const ALLOWED_HOSTS: [&str; 2] = ["api.github.com", "objects.githubusercontent.com"];

/// The version this build reports.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// What a check found.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    /// The version running now.
    pub current: String,
    /// The newest release found, when the check succeeded.
    pub latest: Option<String>,
    /// Whether `latest` is newer than `current`.
    pub update_available: bool,
    /// Release notes, so a person can see what they would be installing.
    pub notes: Option<String>,
    /// Direct link to the release page.
    pub url: Option<String>,
    /// Size of the installer in bytes.
    pub size: Option<u64>,
    /// Why the check could not be completed, when it could not.
    pub error: Option<String>,
}

impl UpdateStatus {
    fn failed(reason: impl Into<String>) -> Self {
        UpdateStatus {
            current: current_version().to_string(),
            latest: None,
            update_available: false,
            notes: None,
            url: None,
            size: None,
            error: Some(reason.into()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize, Clone)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Compare two versions the way releases are actually numbered.
///
/// Deliberately not a string comparison: `0.10.0` is newer than `0.9.0` and
/// sorts before it alphabetically, which is exactly the bug that makes an
/// updater stop offering updates at version 10.
fn is_newer(latest: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.trim_start_matches(['v', 'V'])
            // A pre-release suffix is ignored for ordering rather than guessed at.
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    }
    let (a, b) = (parts(latest), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// The Windows installer among a release's assets.
fn installer(assets: &[Asset]) -> Option<Asset> {
    assets
        .iter()
        .find(|a| a.name.ends_with("-setup.exe"))
        .or_else(|| assets.iter().find(|a| a.name.ends_with(".msi")))
        .cloned()
}

fn host_allowed(url: &str) -> bool {
    let rest = match url.strip_prefix("https://") {
        Some(r) => r,
        None => return false,
    };
    let host = rest.split('/').next().unwrap_or("");
    ALLOWED_HOSTS.contains(&host) || host.ends_with(".githubusercontent.com")
}

/// Ask GitHub whether there is a newer release.
///
/// Never returns `Err`: a failed check is a status with a reason in it, because
/// "we could not reach GitHub" is something to show a person calmly rather than
/// an error that colours the whole screen red.
pub async fn check() -> UpdateStatus {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(format!("wtfault-scanner/{}", current_version()))
        .build()
    {
        Ok(c) => c,
        Err(e) => return UpdateStatus::failed(format!("could not build an HTTP client: {e}")),
    };

    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let response = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => return UpdateStatus::failed(format!("could not reach GitHub: {e}")),
    };

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return UpdateStatus::failed(
            "GitHub returned 404 for the releases endpoint. That is what a private repository \
             looks like to an unauthenticated request: the releases have to be public for the \
             app to see them.",
        );
    }
    if !response.status().is_success() {
        return UpdateStatus::failed(format!("GitHub answered {}", response.status()));
    }

    let release: Release = match response.json().await {
        Ok(r) => r,
        Err(e) => return UpdateStatus::failed(format!("could not read the release: {e}")),
    };

    let asset = installer(&release.assets);
    UpdateStatus {
        current: current_version().to_string(),
        update_available: is_newer(&release.tag_name, current_version()) && asset.is_some(),
        latest: Some(release.tag_name.clone()),
        notes: release.body.clone(),
        url: release.html_url.clone(),
        size: asset.as_ref().map(|a| a.size),
        error: asset.is_none().then(|| {
            "the newest release has no Windows installer attached, so there is nothing to install"
                .to_string()
        }),
    }
}

/// Download the newest installer and start it.
///
/// Returns the path it was written to. The installer runs as a separate
/// process: this app cannot replace its own running binary, so it hands over
/// and exits rather than pretending to update itself in place.
pub async fn download_and_launch() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .user_agent(format!("wtfault-scanner/{}", current_version()))
        .build()
        .map_err(|e| e.to_string())?;

    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let release: Release = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("could not reach GitHub: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GitHub refused the request: {e}"))?
        .json()
        .await
        .map_err(|e| format!("could not read the release: {e}"))?;

    if !is_newer(&release.tag_name, current_version()) {
        return Err(format!(
            "the newest release is {} and this build is {}, so there is nothing to install",
            release.tag_name,
            current_version()
        ));
    }

    let asset = installer(&release.assets)
        .ok_or_else(|| String::from("that release has no Windows installer attached"))?;

    // The download URL comes from GitHub's own response, but it is still
    // checked: this is the one place in the app that writes an executable and
    // runs it, and "the server told us to" is not a reason to skip looking.
    if !host_allowed(&asset.browser_download_url) {
        return Err(format!(
            "the installer is hosted at an unexpected address ({}), so it was not downloaded",
            asset.browser_download_url
        ));
    }

    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|e| format!("could not download the installer: {e}"))?
        .error_for_status()
        .map_err(|e| format!("the download was refused: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("the download did not complete: {e}"))?;

    if bytes.len() as u64 != asset.size {
        return Err(format!(
            "the installer downloaded as {} bytes but the release says it is {}. It was not run.",
            bytes.len(),
            asset.size
        ));
    }

    let dir = std::env::temp_dir().join("wtfault-scanner-update");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&asset.name);
    std::fs::write(&path, &bytes).map_err(|e| format!("could not save the installer: {e}"))?;

    std::process::Command::new(&path)
        .spawn()
        .map_err(|e| format!("could not start the installer: {e}"))?;

    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug every hand-rolled updater has: string comparison stops offering
    /// updates the moment a version number reaches double digits.
    #[test]
    fn version_ordering_is_numeric_not_alphabetical() {
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        // A shorter version is not automatically older.
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(is_newer("1.0.1", "1.0"));
    }

    #[test]
    fn a_prerelease_suffix_does_not_break_the_comparison() {
        assert!(is_newer("0.2.0-beta1", "0.1.0"));
        assert!(!is_newer("0.1.0-beta1", "0.1.0"));
    }

    #[test]
    fn only_github_hosts_are_accepted() {
        assert!(host_allowed("https://objects.githubusercontent.com/x"));
        assert!(host_allowed("https://release-assets.githubusercontent.com/y"));
        assert!(!host_allowed("https://example.com/evil.exe"));
        assert!(!host_allowed("http://objects.githubusercontent.com/x"));
        assert!(!host_allowed("https://githubusercontent.com.evil.com/x"));
    }

    #[test]
    fn the_windows_installer_is_preferred_over_the_msi() {
        let assets = vec![
            Asset { name: "app.msi".into(), browser_download_url: "https://objects.githubusercontent.com/m".into(), size: 2 },
            Asset { name: "app-setup.exe".into(), browser_download_url: "https://objects.githubusercontent.com/e".into(), size: 1 },
        ];
        assert_eq!(installer(&assets).unwrap().name, "app-setup.exe");
    }

    #[test]
    fn a_release_with_no_installer_offers_nothing() {
        let assets = vec![Asset {
            name: "notes.txt".into(),
            browser_download_url: "https://objects.githubusercontent.com/n".into(),
            size: 1,
        }];
        assert!(installer(&assets).is_none());
    }
}
