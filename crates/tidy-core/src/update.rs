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
/// Downloading an installer (~10 MB) may take a while on a slow line.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("network: {0}")]
    Http(String),
    #[error("unexpected answer: {0}")]
    Format(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("downloaded file does not match the release (sha256 {got}, expected {want})")]
    Mismatch { got: String, want: String },
    #[error("could not save the installer: {0}")]
    Io(String),
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
    /// `"sha256:<hex>"` (GitHub computes it on upload).
    #[serde(default)]
    digest: Option<String>,
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
    /// SHA-256 (hex) of the installer at `url`, from the release.
    #[serde(default)]
    pub sha256: Option<String>,
    /// "지금 업데이트" can download, verify and run the installer itself:
    /// a Windows setup `.exe` with a known SHA-256.
    #[serde(default)]
    pub auto_install: bool,
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
        .iter()
        .map(|a| (a.name.clone(), a.browser_download_url.clone()))
        .collect();
    let picked = pick_asset(&assets, os);
    let sha256 = picked.as_ref().and_then(|u| {
        r.assets
            .iter()
            .find(|a| &a.browser_download_url == u)
            .and_then(|a| a.digest.as_deref())
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase)
    });
    let auto_install = os == "windows"
        && sha256.is_some()
        && picked
            .as_deref()
            .is_some_and(|u| u.to_lowercase().ends_with("-setup.exe"));
    Ok(UpdateInfo {
        current: current.to_string(),
        latest: r.tag_name.trim_start_matches(['v', 'V']).to_string(),
        newer: latest > cur,
        url: picked.unwrap_or_else(|| r.html_url.clone()),
        page: r.html_url,
        sha256,
        auto_install,
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

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Download the installer of `info` into `dir` and check it against the
/// release's SHA-256. Only for [`UpdateInfo::auto_install`]. Returns the
/// saved file (`dir/<asset name>`).
pub async fn download_installer(
    info: &UpdateInfo,
    dir: &std::path::Path,
) -> Result<std::path::PathBuf, UpdateError> {
    let want = match (&info.sha256, info.auto_install) {
        (Some(h), true) => h.clone(),
        _ => {
            return Err(UpdateError::Unsupported(
                "this release has no installer that can be installed automatically".into(),
            ))
        }
    };
    let name = info
        .url
        .rsplit('/')
        .next()
        .filter(|n| {
            !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
        .ok_or_else(|| UpdateError::Format(format!("installer url {}", info.url)))?
        .to_string();
    let http = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(concat!("tidy/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    let resp = http
        .get(&info.url)
        .send()
        .await
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(UpdateError::Http(format!("HTTP {}", resp.status())));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    let got = sha256_hex(&bytes);
    if got != want {
        return Err(UpdateError::Mismatch { got, want });
    }
    std::fs::create_dir_all(dir).map_err(|e| UpdateError::Io(e.to_string()))?;
    let path = dir.join(name);
    std::fs::write(&path, &bytes).map_err(|e| UpdateError::Io(e.to_string()))?;
    Ok(path)
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
        // No digest: still a download link, but not installable in place.
        assert!(!i.auto_install && i.sha256.is_none());
    }

    #[test]
    fn digest_enables_auto_install_on_windows_only() {
        let h = "AB".repeat(32);
        let json = format!(
            r#"{{"tag_name":"v0.7.3","html_url":"https://x/r",
            "assets":[{{"name":"Tidy_0.7.3_x64-setup.exe","browser_download_url":"https://x/a-setup.exe","digest":"sha256:{h}"}},
                      {{"name":"Tidy_0.7.3_universal.dmg","browser_download_url":"https://x/a.dmg","digest":"sha256:{h}"}}]}}"#
        );
        let w = evaluate(&json, "0.7.2", "windows").unwrap();
        assert!(w.auto_install);
        assert_eq!(w.sha256.as_deref(), Some("ab".repeat(32).as_str()));
        let m = evaluate(&json, "0.7.2", "macos").unwrap();
        assert!(!m.auto_install);
        assert_eq!(m.url, "https://x/a.dmg");
        // A malformed digest is ignored.
        let bad = json.replace(&h, "zz");
        assert!(!evaluate(&bad, "0.7.2", "windows").unwrap().auto_install);
    }

    #[test]
    fn sha256_of_known_input() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn download_refuses_without_auto_install() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let info = UpdateInfo {
            current: "0.7.2".into(),
            latest: "0.7.3".into(),
            newer: true,
            url: "https://x/r".into(),
            page: "https://x/r".into(),
            sha256: None,
            auto_install: false,
        };
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            rt.block_on(download_installer(&info, dir.path())),
            Err(UpdateError::Unsupported(_))
        ));
    }

    /// Serve `body` once per connection on 127.0.0.1 (plain HTTP/1.1).
    async fn serve(body: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = l.accept().await {
                let mut buf = [0u8; 2048];
                let _ = c.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = c.write_all(head.as_bytes()).await;
                let _ = c.write_all(body).await;
            }
        });
        format!("http://{addr}/Tidy_9.9.9_x64-setup.exe")
    }

    #[test]
    fn download_checks_sha256_before_saving() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let url = serve(b"installer bytes").await;
            let dir = tempfile::tempdir().unwrap();
            let mut info = UpdateInfo {
                current: "0.7.2".into(),
                latest: "9.9.9".into(),
                newer: true,
                url,
                page: "https://x/r".into(),
                sha256: Some(sha256_hex(b"installer bytes")),
                auto_install: true,
            };
            let p = download_installer(&info, dir.path()).await.unwrap();
            assert_eq!(p.file_name().unwrap(), "Tidy_9.9.9_x64-setup.exe");
            assert_eq!(std::fs::read(&p).unwrap(), b"installer bytes");

            // Wrong hash: nothing is saved.
            let dir2 = tempfile::tempdir().unwrap();
            info.sha256 = Some("0".repeat(64));
            assert!(matches!(
                download_installer(&info, dir2.path()).await,
                Err(UpdateError::Mismatch { .. })
            ));
            assert_eq!(std::fs::read_dir(dir2.path()).unwrap().count(), 0);
        });
    }
}
