//! (M11, SPEC §14.6) "업데이트 확인": the latest GitHub release vs. the
//! running version. Read-only network call with a short timeout; callers
//! treat any error as "could not check" and never block on it.

use std::cmp::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Latest release of the Tidy repository.
pub const DEFAULT_URL: &str = "https://api.github.com/repos/sjh714252-prog/tidy/releases/latest";
/// Where `DEFAULT_URL` is replaced (headless tests point it at a fake).
pub const URL_ENV: &str = "TIDY_UPDATE_URL";
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("network: {0}")]
    Http(String),
    #[error("unexpected answer: {0}")]
    Format(String),
}

/// `major.minor.patch[-pre]`. A pre-release sorts before its release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    /// `"v0.4.1"`, `"0.4.1"`, `"0.4"`, `"0.5.0-beta.1"`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p.to_string())),
            None => (s.split('+').next().unwrap_or(s), None),
        };
        let mut it = core.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next().map_or(Some(0), |x| x.parse().ok())?;
        let patch = it.next().map_or(Some(0), |x| x.parse().ok())?;
        if it.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(o.major, o.minor, o.patch))
            .then_with(|| match (&self.pre, &o.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

/// Result of one check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    /// Latest release version without the `v`.
    pub latest: String,
    pub newer: bool,
    /// Installer for this OS, or the release page.
    pub url: String,
    pub page: String,
}

/// Installer asset for `os` ("windows" | "macos" | "linux"), by name.
pub fn pick_asset(names: &[(String, String)], os: &str) -> Option<String> {
    let want: &[&str] = match os {
        "windows" => &["-setup.exe", ".msi", ".exe"],
        "macos" => &[".dmg"],
        _ => &[".appimage", ".deb"],
    };
    want.iter().find_map(|suffix| {
        names
            .iter()
            .find(|(n, _)| n.to_lowercase().ends_with(suffix))
            .map(|(_, u)| u.clone())
    })
}

/// Compare a release JSON (`releases/latest`) with `current`.
pub fn evaluate(json: &str, current: &str, os: &str) -> Result<UpdateInfo, UpdateError> {
    let r: ApiRelease =
        serde_json::from_str(json).map_err(|e| UpdateError::Format(e.to_string()))?;
    let latest = Version::parse(&r.tag_name)
        .ok_or_else(|| UpdateError::Format(format!("tag {}", r.tag_name)))?;
    let cur = Version::parse(current)
        .ok_or_else(|| UpdateError::Format(format!("current version {current}")))?;
    let assets: Vec<(String, String)> = r
        .assets
        .into_iter()
        .map(|a| (a.name, a.browser_download_url))
        .collect();
    Ok(UpdateInfo {
        current: current.to_string(),
        latest: r.tag_name.trim_start_matches(['v', 'V']).to_string(),
        newer: latest > cur,
        url: pick_asset(&assets, os).unwrap_or_else(|| r.html_url.clone()),
        page: r.html_url,
    })
}

/// `TIDY_UPDATE_URL` or the GitHub API.
pub fn api_url() -> String {
    std::env::var(URL_ENV)
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string())
}

/// This build's OS name for [`pick_asset`].
pub fn this_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Ask `url` for the latest release (one GET, [`TIMEOUT`]).
pub async fn check(url: &str, current: &str) -> Result<UpdateInfo, UpdateError> {
    let http = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("tidy/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    let resp = http
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(UpdateError::Http(format!("HTTP {}", resp.status())));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    evaluate(&text, current, this_os())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_like_semver() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("v0.4.1") > v("0.4.0"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("0.4") == v("0.4.0"));
        assert!(Version::parse("abc").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
    }

    #[test]
    fn release_json_to_info() {
        let json = r#"{"tag_name":"v0.4.1","html_url":"https://x/releases/v0.4.1",
            "assets":[{"name":"Tidy_0.4.1_x64_ko-KR.msi","browser_download_url":"https://x/a.msi"},
                      {"name":"Tidy_0.4.1_x64-setup.exe","browser_download_url":"https://x/a-setup.exe"},
                      {"name":"tidy_0.4.1_amd64.AppImage","browser_download_url":"https://x/a.AppImage"}]}"#;
        let i = evaluate(json, "0.4.0", "windows").unwrap();
        assert!(i.newer);
        assert_eq!(i.latest, "0.4.1");
        assert_eq!(i.url, "https://x/a-setup.exe");
        assert_eq!(
            evaluate(json, "0.4.0", "linux").unwrap().url,
            "https://x/a.AppImage"
        );
        assert_eq!(
            evaluate(json, "0.4.0", "macos").unwrap().url,
            "https://x/releases/v0.4.1"
        );
        assert!(!evaluate(json, "0.4.1", "windows").unwrap().newer);
        assert!(!evaluate(json, "0.5.0", "windows").unwrap().newer);
        assert!(evaluate("{}", "0.4.0", "windows").is_err());
    }
}
