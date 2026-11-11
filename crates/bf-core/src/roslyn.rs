//! Microsoft's Roslyn language server (the one VS Code's C# extension uses), as an alternative to
//! csharp-ls: installed from Microsoft's public `vs-impl` NuGet feed (it isn't on nuget.org), checked
//! against Microsoft's package signature before it is unpacked, and kept in ByteForge's cache.
//!
//! The `neutral` package is framework-dependent (it runs with `dotnet` on every OS) and needs the .NET 10
//! runtime or newer.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, bail};

/// The feed's service index (VS Code's C# extension downloads from the same feed).
const FEED: &str = "https://pkgs.dev.azure.com/azure-public/vside/_packaging/vs-impl/nuget/v3/index.json";
const PACKAGE: &str = "microsoft.codeanalysis.languageserver.neutral";
/// The server's files inside the package.
const CONTENT: &str = "content/LanguageServer/neutral/";
pub const DLL: &str = "Microsoft.CodeAnalysis.LanguageServer.dll";

fn curl(url: &str) -> Command {
    let mut c = Command::new("curl");
    c.args(["--silent", "--show-error", "--fail", "--location", "--max-time", "600"]).arg(url).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn get_json(url: &str) -> anyhow::Result<serde_json::Value> {
    let out = curl(url).output().context("could not run curl")?;
    if !out.status.success() {
        bail!("{url}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("unexpected answer from {url}"))
}

/// The installed server's folder for each version, newest first: `<cache>/roslyn/<version>/`.
pub fn installed(cache: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(cache.join("roslyn")) else { return Vec::new() };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(DLL).is_file())
        .filter_map(|p| Some((p.file_name()?.to_string_lossy().into_owned(), p.join(DLL))))
        .collect();
    out.sort_by(|a, b| crate::nuget::compare_versions(&b.0, &a.0));
    out
}

/// The newest version on the feed: where its packages are, and the version.
pub fn latest() -> anyhow::Result<(String, String)> {
    let index = get_json(FEED)?;
    let base = index
        .get("resources")
        .and_then(|r| r.as_array())
        .into_iter()
        .flatten()
        .find(|r| r.get("@type").and_then(|t| t.as_str()).is_some_and(|t| t.starts_with("PackageBaseAddress")))
        .and_then(|r| r.get("@id").and_then(|i| i.as_str()))
        .context("the feed has no package address")?
        .trim_end_matches('/')
        .to_owned();
    let versions = get_json(&format!("{base}/{PACKAGE}/index.json"))?;
    let newest = newest(versions.get("versions").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()))
        .context("the feed lists no versions")?;
    Ok((base, newest))
}

/// The highest version (the feed doesn't list them in order).
pub fn newest<'a>(versions: impl Iterator<Item = &'a str>) -> Option<String> {
    versions.max_by(|a, b| crate::nuget::compare_versions(a, b)).map(str::to_owned)
}

/// Whether `dotnet nuget verify` output says Microsoft signed the package and it is intact.
pub fn signed_by_microsoft(output: &str) -> bool {
    output.contains("Successfully verified package") && output.contains("Subject Name: CN=Microsoft Corporation")
}

/// Downloads `version` from `base`, checks Microsoft's signature (with the installed .NET SDK) and unpacks
/// the server into the cache. Returns the server's DLL.
pub fn install(cache: &Path, base: &str, version: &str) -> anyhow::Result<PathBuf> {
    let dir = cache.join("roslyn");
    std::fs::create_dir_all(&dir)?;
    let package = dir.join(format!("{version}.nupkg"));
    let url = format!("{base}/{PACKAGE}/{version}/{PACKAGE}.{version}.nupkg");
    let out = curl(&url).arg("--output").arg(&package).output().context("could not run curl")?;
    if !out.status.success() {
        bail!("downloading the Roslyn language server failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let result = verify_and_unpack(&package, &dir.join(version));
    let _ = std::fs::remove_file(&package);
    result
}

fn verify_and_unpack(package: &Path, target: &Path) -> anyhow::Result<PathBuf> {
    let out = Command::new("dotnet")
        .args(["nuget", "verify", "--all", "-v", "normal"])
        .arg(package)
        .env("DOTNET_NOLOGO", "1")
        .stdin(Stdio::null())
        .output()
        .context("could not run dotnet to check the package signature (is the .NET SDK installed?)")?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() || !signed_by_microsoft(&text) {
        bail!("the downloaded package isn't signed by Microsoft, so it wasn't installed:\n{}", text.trim());
    }
    let _ = std::fs::remove_dir_all(target);
    let partial = target.with_extension("partial");
    let _ = std::fs::remove_dir_all(&partial);
    unpack(package, &partial)?;
    std::fs::rename(&partial, target)?;
    let dll = target.join(DLL);
    if !dll.is_file() {
        bail!("the package has no {DLL}");
    }
    Ok(dll)
}

/// Unpacks the package's server folder (and nothing outside it) into `target`.
fn unpack(package: &Path, target: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(package)?;
    let mut zip = zip::ZipArchive::new(file).context("the download isn't a NuGet package")?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(name) = entry.enclosed_name() else { continue }; // no "../" escapes
        let Ok(rel) = name.strip_prefix(CONTENT) else { continue };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = target.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut file)?;
    }
    Ok(())
}

/// Whether the .NET runtime the server needs (10 or newer) is installed.
pub fn runtime_available() -> bool {
    let Ok(out) = Command::new("dotnet").arg("--list-runtimes").stdin(Stdio::null()).output() else { return false };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("Microsoft.NETCore.App "))
        .filter_map(|v| v.split('.').next()?.parse::<u32>().ok())
        .any(|major| major >= 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_newest_version() {
        let listed = ["4.8.0-7.25569.21", "5.4.0-2.26179.14", "4.8.0-7.25324.2", "5.0.0-1.25277.114"];
        assert_eq!(newest(listed.into_iter()).as_deref(), Some("5.4.0-2.26179.14"));
    }

    #[test]
    fn needs_microsofts_signature() {
        let ok = "Signature type: Author\n  Subject Name: CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond\nSuccessfully verified package 'X'.";
        assert!(signed_by_microsoft(ok));
        assert!(!signed_by_microsoft("Subject Name: CN=Someone Else\nSuccessfully verified package 'X'."));
        assert!(!signed_by_microsoft("Subject Name: CN=Microsoft Corporation\nerror: NU3008: The package integrity check failed."));
    }

    #[test]
    fn unpacks_only_the_server_folder() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("bf-roslyn-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let package = dir.join("p.nupkg");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&package).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            for (name, body) in [(format!("{CONTENT}{DLL}"), "dll"), (format!("{CONTENT}sub/x.json"), "{}"), ("lib/net10.0/other.dll".to_owned(), "no")] {
                zip.start_file(name, options).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        let target = dir.join("out");
        unpack(&package, &target).unwrap();
        assert!(target.join(DLL).is_file() && target.join("sub/x.json").is_file());
        assert!(!target.join("other.dll").exists() && !dir.join("lib").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
