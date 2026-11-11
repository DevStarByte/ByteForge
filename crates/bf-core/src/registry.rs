//! Module packages: the registry (`modules/index.json` on the `packages` branch of the ByteForge repo),
//! installing, updating and uninstalling modules in the user's modules folder, and the preinstalled
//! modules that ship with ByteForge (copied into the user's folder once, then ordinary packages).
//!
//! Downloads use `curl` (part of every Arch install, and of Windows 10+), so there's no HTTP stack here.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

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
    /// Programs the module needs on PATH (e.g. `python3`, `dotnet`), shown before installing.
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
    let tmp = std::env::temp_dir().join(format!("byteforge-index-{}.json", std::process::id()));
    download(&format!("{}/index.json", base.trim_end_matches('/')), &tmp)?;
    let text = std::fs::read_to_string(&tmp);
    let _ = std::fs::remove_file(&tmp);
    parse_index(&text?)
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
        let target = modules_dir.join(&package.dir);
        if target.exists() {
            std::fs::remove_dir_all(&target).with_context(|| format!("removing the old {}", target.display()))?;
        }
        std::fs::rename(&staging, &target)?;
        Ok(target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
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
pub const PREINSTALLED: &[&str] = &["byteforge.csharpier"];

/// Copies the [`PREINSTALLED`] modules bundled with ByteForge (`<install dir>/modules`) into the user's
/// modules folder, once per module: ids in `done` were preinstalled before (and may have been uninstalled
/// since). Returns the ids preinstalled now.
pub fn preinstall(bundled: &Path, modules_dir: &Path, done: &[String]) -> Vec<String> {
    let mut installed = Vec::new();
    let Ok(entries) = std::fs::read_dir(bundled) else { return installed };
    for entry in entries.flatten() {
        let src = entry.path();
        let Some(id) = std::fs::read_to_string(src.join("module.json"))
            .ok()
            .and_then(|t| json5::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_owned))
        else {
            continue;
        };
        if !PREINSTALLED.contains(&id.as_str()) || done.contains(&id) {
            continue;
        }
        let dest = modules_dir.join(entry.file_name());
        if !dest.exists() && copy_dir(&src, &dest).is_err() {
            let _ = std::fs::remove_dir_all(&dest);
            continue;
        }
        installed.push(id);
    }
    installed
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
    let output = Command::new("curl")
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
