//! Whether a newer Voi is out: the latest release of the public releases
//! repository, compared with the build this one is.
//!
//! The repository is named once, in `config/releases-repo` at the root of the
//! source, which `scripts/release.sh` publishes to as well; another
//! deployment changes that file and nothing else. Its releases are tagged
//! `v<version>.<build>` and carry `Voi-<version>.<build>.dmg` and `.exe`, the
//! build being the number `CFBundleVersion` and the Windows build carry:
//! commits plus `config/build-base`.
//!
//! Only asked, never acted on: the menu offers the download, the person
//! installs it. A self-replacing app on the Mac would lose its microphone
//! permission with every update.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// `owner/name` of the public repository the builds are published to.
pub fn releases_repo() -> &'static str {
    include_str!("../../../config/releases-repo").trim()
}

/// A newer build, and where to get it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub build: u64,
    /// As people see it: `0.1.0 (65)`.
    pub label: String,
    /// The file for this system, which the browser downloads straight away.
    pub download: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// `v0.1.0.65` → (`0.1.0`, 65).
fn version_of_tag(tag: &str) -> Option<(&str, u64)> {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    let (version, build) = tag.rsplit_once('.')?;
    Some((version, build.parse().ok()?))
}

/// The update in a GitHub release `json`, if it is newer than `mine` and has a
/// file ending in `extension` (`.dmg`, `.exe`).
pub fn newer_in(json: &str, mine: u64, extension: &str) -> Result<Option<Update>> {
    let release: Release = serde_json::from_str(json).context("not a release")?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let Some((version, build)) = version_of_tag(&release.tag_name) else {
        bail!("a release tag without a build: {}", release.tag_name);
    };
    if build <= mine {
        return Ok(None);
    }
    let extension = extension.to_ascii_lowercase();
    let Some(asset) = release.assets.iter().find(|a| a.name.to_ascii_lowercase().ends_with(&extension)) else {
        return Ok(None);
    };
    Ok(Some(Update { build, label: format!("{version} ({build})"), download: asset.browser_download_url.clone() }))
}

/// Ask GitHub. `mine` 0 — a build that does not know its number, run from the
/// source tree — never asks: it has nothing to compare.
///
/// `VOI_UPDATE_FEED` points it at another URL, for tests.
pub async fn check(mine: u64, extension: &str) -> Result<Option<Update>> {
    if mine == 0 {
        return Ok(None);
    }
    let url = std::env::var("VOI_UPDATE_FEED")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{}/releases/latest", releases_repo()));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        // GitHub refuses requests without one.
        .user_agent(concat!("Voi/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let response = client.get(&url).header("Accept", "application/vnd.github+json").send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // No release yet, or no repository.
        return Ok(None);
    }
    let json = response.error_for_status()?.text().await?;
    newer_in(&json, mine, extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"{
        "tag_name": "v0.1.0.65",
        "draft": false,
        "prerelease": false,
        "assets": [
            {"name": "Voi-0.1.0.65.dmg", "browser_download_url": "https://example.com/Voi-0.1.0.65.dmg"},
            {"name": "Voi-0.1.0.65.exe", "browser_download_url": "https://example.com/Voi-0.1.0.65.exe"}
        ]
    }"#;

    #[test]
    fn a_newer_build_offers_this_systems_file() {
        let mac = newer_in(RELEASE, 64, ".dmg").unwrap().unwrap();
        assert_eq!(mac.label, "0.1.0 (65)");
        assert_eq!(mac.download, "https://example.com/Voi-0.1.0.65.dmg");
        let windows = newer_in(RELEASE, 64, ".exe").unwrap().unwrap();
        assert_eq!(windows.download, "https://example.com/Voi-0.1.0.65.exe");
    }

    #[test]
    fn the_same_or_an_older_build_offers_nothing() {
        assert_eq!(newer_in(RELEASE, 65, ".dmg").unwrap(), None);
        assert_eq!(newer_in(RELEASE, 70, ".exe").unwrap(), None);
    }

    #[test]
    fn a_release_without_this_systems_file_offers_nothing() {
        assert_eq!(newer_in(RELEASE, 1, ".AppImage").unwrap(), None);
    }

    #[test]
    fn drafts_and_prereleases_are_not_updates() {
        let draft = RELEASE.replace(r#""draft": false"#, r#""draft": true"#);
        assert_eq!(newer_in(&draft, 1, ".dmg").unwrap(), None);
        let pre = RELEASE.replace(r#""prerelease": false"#, r#""prerelease": true"#);
        assert_eq!(newer_in(&pre, 1, ".dmg").unwrap(), None);
    }

    #[test]
    fn tags_read_as_version_and_build() {
        assert_eq!(version_of_tag("v0.1.0.65"), Some(("0.1.0", 65)));
        assert_eq!(version_of_tag("0.2.0.100"), Some(("0.2.0", 100)));
        assert_eq!(version_of_tag("v1"), None);
        assert_eq!(version_of_tag("v0.1.0.beta"), None);
    }

    #[test]
    fn the_repository_is_named_once() {
        assert!(releases_repo().contains('/'), "{}", releases_repo());
        assert!(!releases_repo().contains(char::is_whitespace));
    }

    #[test]
    fn a_build_without_a_number_never_asks() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert_eq!(rt.block_on(check(0, ".dmg")).unwrap(), None);
    }
}
