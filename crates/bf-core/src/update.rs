//! Updates from GitHub Releases: finding a newer release, downloading its installer and applying it.
//!
//! - Windows (installed with the setup): runs the new setup silently after ByteForge exits; it restarts
//!   ByteForge when done (`/RELAUNCH=1`, see packaging/windows/byteforge.iss). The portable zip can't
//!   update itself.
//! - macOS (running from ByteForge.app): a helper script waits for ByteForge to exit, swaps the app
//!   bundle for the one in the new .dmg and opens it.
//! - Linux: packages come from pacman, so updates are only reported.
//!
//! HTTP goes through `curl`, which ships with Windows 10+ and macOS, so no TLS stack is built in.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, bail};
use serde::Deserialize;

const RELEASES_API: &str = "https://api.github.com/repos/DevStarByte/ByteForge/releases?per_page=20";
pub const RELEASES_PAGE: &str = "https://github.com/DevStarByte/ByteForge/releases";

/// A version like `0.3.4` or `0.3.5-beta.2` (a pre-release sorts before its release).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub numbers: [u64; 3],
    /// Pre-release parts after "-", e.g. `["beta", "2"]`; empty for a release.
    pub pre: Vec<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let (core, pre) = match text.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(str::to_owned).collect()),
            None => (text, Vec::new()),
        };
        let mut numbers = [0u64; 3];
        let mut parts = core.split('.');
        for n in &mut numbers {
            *n = parts.next()?.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Self { numbers, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers.cmp(&other.numbers).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        _ => a.cmp(b),
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c] = self.numbers;
        write!(f, "{a}.{b}.{c}")?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

/// A release newer than the running version, with the download for this platform (if any).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub page: String,
    pub notes: String,
    pub asset: Option<Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// The file name suffix of this platform's download.
fn asset_suffix() -> Option<&'static str> {
    if cfg!(windows) {
        Some("-windows-x64-setup.exe")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("-macos-arm64.dmg")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("-macos-x64.dmg")
    } else {
        None
    }
}

/// Picks the newest release above `current` from the GitHub API's JSON. Pre-releases count only with
/// `include_prereleases` (or when the running version is one itself, so beta testers hear of the release).
pub fn newest_release(json: &str, current: &Version, include_prereleases: bool) -> anyhow::Result<Option<Release>> {
    let releases: Vec<ApiRelease> = serde_json::from_str(json).context("unexpected answer from GitHub")?;
    let betas = include_prereleases || current.is_prerelease();
    let best = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| Some((Version::parse(&r.tag_name)?, r)))
        .filter(|(v, _)| v > current && (betas || !v.is_prerelease()))
        .max_by(|a, b| a.0.cmp(&b.0));
    Ok(best.map(|(version, r)| {
        let asset = asset_suffix().and_then(|suffix| {
            r.assets.into_iter().find(|a| a.name.ends_with(suffix)).map(|a| Asset { name: a.name, url: a.browser_download_url, size: a.size })
        });
        Release { version, page: r.html_url, notes: r.body.unwrap_or_default(), asset }
    }))
}

fn curl() -> Command {
    let mut command = Command::new("curl");
    command.args(["--silent", "--show-error", "--fail", "--location", "--max-time", "600"]).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// Asks GitHub for a release newer than `current`. Blocking; run it off the UI thread.
pub fn check(current: &Version, include_prereleases: bool) -> anyhow::Result<Option<Release>> {
    let out = curl()
        .args(["--max-time", "20", "-H", "Accept: application/vnd.github+json", RELEASES_API])
        .output()
        .context("could not run curl")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    newest_release(&String::from_utf8_lossy(&out.stdout), current, include_prereleases)
}

/// Downloads `asset` into the temp folder and returns the file. Blocking.
pub fn download(asset: &Asset) -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join("byteforge-update");
    std::fs::create_dir_all(&dir)?;
    // Only a plain file name from the API, never a path.
    let name = Path::new(&asset.name).file_name().context("bad asset name")?;
    let file = dir.join(name);
    let partial = file.with_extension("part");
    let out = curl().arg("--output").arg(&partial).arg(&asset.url).output().context("could not run curl")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&partial);
        bail!("download failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let size = std::fs::metadata(&partial)?.len();
    if asset.size > 0 && size != asset.size {
        let _ = std::fs::remove_file(&partial);
        bail!("download incomplete ({size} of {} bytes)", asset.size);
    }
    std::fs::rename(&partial, &file)?;
    Ok(file)
}

/// How this copy of ByteForge can be updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// Installed by the Windows setup (the uninstaller sits next to the exe).
    WindowsSetup,
    /// Running from an app bundle on macOS; the path is the `.app`.
    MacBundle(PathBuf),
    /// Portable zip, `cargo run`, a Linux package, …: report updates, don't install them.
    Manual,
}

pub fn install_kind() -> InstallKind {
    let Ok(exe) = std::env::current_exe() else { return InstallKind::Manual };
    let exe = exe.canonicalize().unwrap_or(exe);
    if cfg!(windows) && exe.parent().is_some_and(|d| d.join("unins000.exe").is_file()) {
        return InstallKind::WindowsSetup;
    }
    if cfg!(target_os = "macos") {
        // …/ByteForge.app/Contents/MacOS/byteforge
        if let Some(bundle) = exe.ancestors().nth(3).filter(|b| b.extension().is_some_and(|e| e == "app")) {
            return InstallKind::MacBundle(bundle.to_path_buf());
        }
    }
    InstallKind::Manual
}

/// Starts applying a downloaded update; call it right before ByteForge exits. The installer (Windows)
/// or helper script (macOS) waits for this process to end, installs, and starts the new version.
pub fn apply(kind: &InstallKind, file: &Path) -> anyhow::Result<()> {
    match kind {
        InstallKind::WindowsSetup => {
            let mut command = Command::new(file);
            // /CLOSEAPPLICATIONS lets the setup wait for (or close) ByteForge before replacing its files.
            command.args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/RELAUNCH=1"]);
            command.spawn().context("could not start the installer")?;
        }
        InstallKind::MacBundle(app) => {
            let script = file.with_extension("sh");
            std::fs::write(&script, MAC_SCRIPT)?;
            Command::new("/bin/sh")
                .arg(&script)
                .arg(std::process::id().to_string())
                .arg(file)
                .arg(app)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("could not start the update helper")?;
        }
        InstallKind::Manual => bail!("this copy of ByteForge can't update itself"),
    }
    Ok(())
}

/// macOS helper: `sh update.sh <pid> <dmg> <app>`. Keeps the old app until the new one is copied, and
/// puts it back if copying fails.
const MAC_SCRIPT: &str = r#"#!/bin/sh
pid="$1"; dmg="$2"; app="$3"
while kill -0 "$pid" 2>/dev/null; do sleep 0.3; done
mnt="$(mktemp -d)"
if hdiutil attach -nobrowse -noautoopen -quiet -mountpoint "$mnt" "$dmg"; then
    rm -rf "$app.old"
    if mv "$app" "$app.old" && ditto "$mnt/ByteForge.app" "$app"; then
        rm -rf "$app.old"
        xattr -dr com.apple.quarantine "$app" 2>/dev/null
    else
        rm -rf "$app"
        mv "$app.old" "$app"
    fi
    hdiutil detach "$mnt" -quiet
fi
rmdir "$mnt" 2>/dev/null
rm -f "$dmg"
open "$app"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn orders_versions_and_prereleases() {
        assert!(v("0.3.5") > v("0.3.4"));
        assert!(v("v0.4.0") > v("0.3.10"));
        assert!(v("0.3.4") > v("0.3.4-beta.9"));
        assert!(v("0.3.4-beta.10") > v("0.3.4-beta.2"));
        assert_eq!(v("0.3.4-beta.1").to_string(), "0.3.4-beta.1");
        assert!(Version::parse("0.3").is_none());
        assert!(Version::parse("nightly").is_none());
    }

    const JSON: &str = r#"[
        {"tag_name": "v0.4.0-beta.1", "draft": false, "html_url": "b", "body": "beta", "assets": []},
        {"tag_name": "v0.3.5", "draft": false, "html_url": "https://x/v0.3.5", "body": "notes", "assets": [
            {"name": "ByteForge-0.3.5-windows-x64-setup.exe", "browser_download_url": "https://x/setup.exe", "size": 10},
            {"name": "ByteForge-0.3.5-macos-arm64.dmg", "browser_download_url": "https://x/arm64.dmg", "size": 20},
            {"name": "ByteForge-0.3.5-macos-x64.dmg", "browser_download_url": "https://x/x64.dmg", "size": 20}
        ]},
        {"tag_name": "v0.3.6", "draft": true, "assets": []},
        {"tag_name": "v0.3.4", "draft": false, "assets": []}
    ]"#;

    #[test]
    fn finds_the_newest_release_for_the_channel() {
        let stable = newest_release(JSON, &v("0.3.4"), false).unwrap().unwrap();
        assert_eq!(stable.version, v("0.3.5"));
        assert_eq!(stable.page, "https://x/v0.3.5");
        if cfg!(windows) {
            assert_eq!(stable.asset.unwrap().url, "https://x/setup.exe");
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(stable.asset.unwrap().url, "https://x/arm64.dmg");
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            assert_eq!(stable.asset.unwrap().url, "https://x/x64.dmg");
        } else {
            assert!(stable.asset.is_none());
        }
        assert_eq!(newest_release(JSON, &v("0.3.4"), true).unwrap().unwrap().version, v("0.4.0-beta.1"));
        // Beta testers hear about betas without opting in.
        assert_eq!(newest_release(JSON, &v("0.3.5-beta.1"), false).unwrap().unwrap().version, v("0.4.0-beta.1"));
        assert!(newest_release(JSON, &v("0.4.0"), true).unwrap().is_none());
    }
}
