//! Module packages: the registry (`modules/index.json` on the `packages` branch of the ByteForge repo),
//! installing, updating and uninstalling modules in the user's modules folder, and the preinstalled
//! modules that ship with ByteForge (copied into the user's folder once, then ordinary packages).
//!
//! Downloads use `curl` (part of every Arch install, and of Windows 10+), so there's no HTTP stack here.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

/// Where the registry lives; `"modules.registry"` in settings overrides it (also `file://` URLs).
pub const DEFAULT_REGISTRY: &str = "https://raw.githubusercontent.com/DevStarByte/ByteForge/packages/modules";

/// One package in the registry index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default, rename = "authorUrl")]
    pub author_url: Option<String>,
    /// Folder of the package in the registry, also its folder name when installed.
    pub dir: String,
    /// Files to download, relative to `dir` (`module.json` among them).
    pub files: Vec<String>,
    /// Programs the module needs on PATH (e.g. `dotnet`, `git`, `docker`), shown before installing.
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Deserialize)]
struct Index {
    modules: Vec<PackageInfo>,
}

pub fn parse_index(text: &str) -> anyhow::Result<Vec<PackageInfo>> {
    let index: Index = serde_json::from_str(text).context("invalid registry index")?;
    for p in &index.modules {
        check_relative(&p.dir).with_context(|| format!("package {}", p.id))?;
        for f in &p.files {
            check_relative(f).with_context(|| format!("package {}", p.id))?;
        }
    }
    Ok(index.modules)
}

/// Downloads and parses the registry index.
pub fn fetch_index(base: &str) -> anyhow::Result<Vec<PackageInfo>> {
    // Read from curl's output: a temp file (named per process) was shared by fetches running at the same time.
    let url = format!("{}/index.json", base.trim_end_matches('/'));
    let output = crate::process::command("curl")
        .args(["--fail", "--silent", "--show-error", "--location", "--max-time", "60"])
        .arg(&url)
        .output()
        .context("could not run curl")?;
    if !output.status.success() {
        bail!("{url}: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    parse_index(&String::from_utf8_lossy(&output.stdout))
}

/// Installs (or updates) a package into `modules_dir/<dir>`: downloads every file into a staging folder,
/// checks its `module.json`, then swaps it into place. Returns the installed folder.
pub fn install(base: &str, package: &PackageInfo, modules_dir: &Path) -> anyhow::Result<PathBuf> {
    check_relative(&package.dir)?;
    if !package.files.iter().any(|f| f == "module.json") {
        bail!("package {} has no module.json", package.id);
    }
    std::fs::create_dir_all(modules_dir)?;
    let staging = modules_dir.join(format!(".staging-{}", package.dir));
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| {
        for file in &package.files {
            check_relative(file)?;
            let dest = staging.join(file);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            download(&format!("{}/{}/{}", base.trim_end_matches('/'), package.dir, file), &dest)?;
        }
        let manifest: serde_json::Value = json5::from_str(&std::fs::read_to_string(staging.join("module.json"))?).context("invalid module.json")?;
        if manifest.get("id").and_then(|v| v.as_str()) != Some(package.id.as_str()) {
            bail!("module.json of {} has a different id", package.id);
        }
        // Swap by renaming, never by deleting first: deleting can stop halfway (a file in use on Windows)
        // and leave a broken module. A rename either happens or changes nothing.
        let target = modules_dir.join(&package.dir);
        // The themes a module made (its themes/ folder, not part of the package) stay with the update: they're
        // the module's, and a theme module that found none would take this for a fresh install.
        let made = target.join("themes");
        if made.is_dir() && !staging.join("themes").exists() {
            copy_dir(&made, &staging.join("themes"))?;
        }
        let old = modules_dir.join(format!(".old-{}", package.dir));
        let _ = std::fs::remove_dir_all(&old);
        let had_old = target.exists();
        if had_old {
            rename_retrying(&target, &old).with_context(|| {
                format!("{} is in use (is the module, or a program it started, still running?); the installed version is unchanged", target.display())
            })?;
        }
        if let Err(e) = rename_retrying(&staging, &target) {
            if had_old {
                let _ = std::fs::rename(&old, &target); // the old version back
            }
            return Err(e).with_context(|| format!("moving the new version to {}", target.display()));
        }
        // Best effort: a leftover (a file still held) goes with the next install; discover skips it.
        let _ = std::fs::remove_dir_all(&old);
        Ok(target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// Renames `from` to `to`, trying again for up to two seconds: a module that was just stopped (and a helper it
/// started) can hold its folder for a moment on Windows.
fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if tries < 20 && held_by_a_process(&e) => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            result => return result,
        }
    }
}

/// Windows' "used by another process" (ERROR_SHARING_VIOLATION, 32) or "access denied" (5): a process still
/// has the folder (as its working directory, or a file in it) open.
fn held_by_a_process(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32) | Some(5)) || e.kind() == std::io::ErrorKind::PermissionDenied
}

/// Deletes an installed module's folder (only folders directly inside `modules_dir`).
pub fn uninstall(modules_dir: &Path, module_dir: &Path) -> anyhow::Result<()> {
    if module_dir.parent() != Some(modules_dir) {
        bail!("{} is not in the modules folder", module_dir.display());
    }
    std::fs::remove_dir_all(module_dir)?;
    Ok(())
}

/// The bundled modules that are preinstalled; the others are installed from the Modules view.
/// Modules installed from the registry the first time ByteForge starts (once: one that is uninstalled later
/// stays uninstalled).
pub const PREINSTALLED: &[&str] = &["byteforge.resharper-format"];

/// What to install from the registry at start: the [`PREINSTALLED`] modules not installed before (`done`), and
/// the installed modules of the old kind (Python), whose .NET versions replace them.
pub fn to_install_at_start(installed: &[crate::plugins::ModuleManifest], done: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = PREINSTALLED
        .iter()
        .filter(|id| !done.iter().any(|d| d == *id) && !installed.iter().any(|m| m.id == **id))
        .map(|id| (*id).to_owned())
        .collect();
    ids.extend(installed.iter().filter(|m| m.is_legacy()).map(|m| m.id.clone()));
    ids.dedup();
    ids
}

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let to = dest.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Whether version `a` is newer than `b` (dotted numbers; missing parts count as 0).
pub fn is_newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| v.split(['.', '-', '+']).map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// The registry is remote input: paths must stay inside the package folder.
fn check_relative(path: &str) -> anyhow::Result<()> {
    let p = Path::new(path);
    if path.is_empty() || p.is_absolute() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        bail!("unsafe path in registry: {path:?}");
    }
    Ok(())
}

fn download(url: &str, dest: &Path) -> anyhow::Result<()> {
    let output = crate::process::command("curl")
        .args(["--fail", "--silent", "--show-error", "--location", "--max-time", "60", "--output"])
        .arg(dest)
        .arg(url)
        .output()
        .context("could not run curl")?;
    if !output.status.success() {
        bail!("{url}: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

/// Where a module's author name links to: its `authorUrl`, else the GitHub page of an author name that is
/// a valid GitHub user name (letters, digits and single hyphens).
pub fn author_link(author: Option<&str>, author_url: Option<&str>) -> Option<String> {
    if let Some(url) = author_url.map(str::trim).filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
        return Some(url.to_owned());
    }
    let name = author?.trim();
    let valid = !name.is_empty()
        && name.len() <= 39
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--");
    valid.then(|| format!("https://github.com/{name}"))
}

#[cfg(test)]
mod tests {

    #[test]
    fn author_links() {
        assert_eq!(author_link(Some("DevStarByte"), None).as_deref(), Some("https://github.com/DevStarByte"));
        assert_eq!(author_link(Some("Jane Doe"), None), None);
        assert_eq!(author_link(Some("x"), Some("https://x.dev")).as_deref(), Some("https://x.dev"));
        assert_eq!(author_link(Some("DevStarByte"), Some("javascript:alert(1)")).as_deref(), Some("https://github.com/DevStarByte"));
        assert_eq!(author_link(None, None), None);
    }
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(is_newer("1.10", "1.9.9"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("0.9", "1.0"));
    }

    #[test]
    fn rejects_paths_leaving_the_package() {
        let ok = r#"{"modules":[{"id":"a","name":"A","version":"1","dir":"a","files":["module.json","lib/x.py"]}]}"#;
        assert_eq!(parse_index(ok).unwrap()[0].files.len(), 2);
        for bad in ["../x", "/etc/passwd", "a/../../b"] {
            let text = format!(r#"{{"modules":[{{"id":"a","name":"A","version":"1","dir":"a","files":["{bad}"]}}]}}"#);
            assert!(parse_index(&text).is_err(), "{bad}");
        }
    }
}
