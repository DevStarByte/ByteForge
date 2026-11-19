//! Microsoft's Roslyn language server (the one VS Code's C# extension uses), as an alternative to
//! csharp-ls: Microsoft's `roslyn-language-server` .NET tool from nuget.org, checked against Microsoft's
//! package signature before it is unpacked, and kept in ByteForge's cache.
//!
//! The tool comes as one package per platform (`roslyn-language-server.win-x64`, `.linux-x64`, …). Next to
//! the server it carries the Razor extension, which the server loads from its own folder, so `.razor` and
//! `.cshtml` files get IntelliSense too. The server DLL is framework-dependent (it runs with `dotnet`) and
//! needs the .NET 10 runtime or newer.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, bail};

/// nuget.org's package content address (lowercase package ids).
const FEED: &str = "https://api.nuget.org/v3-flatcontainer";
const TOOL: &str = "roslyn-language-server";
pub const DLL: &str = "Microsoft.CodeAnalysis.LanguageServer.dll";

/// The platform's package: `roslyn-language-server.<runtime id>`.
fn package_id() -> anyhow::Result<String> {
    Ok(format!("{TOOL}.{}", runtime_id()?))
}

/// The .NET runtime id of this machine, as the tool's packages are named.
fn runtime_id() -> anyhow::Result<&'static str> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => bail!("the Roslyn language server has no package for {other} processors"),
    };
    let os = match std::env::consts::OS {
        "windows" => "win",
        "macos" => "osx",
        // Alpine and other musl systems have their own packages.
        "linux" if Path::new(&format!("/lib/ld-musl-{}.so.1", std::env::consts::ARCH)).exists() => "linux-musl",
        "linux" => "linux",
        other => bail!("the Roslyn language server has no package for {other}"),
    };
    Ok(match (os, arch) {
        ("win", "x64") => "win-x64",
        ("win", _) => "win-arm64",
        ("osx", "x64") => "osx-x64",
        ("osx", _) => "osx-arm64",
        ("linux-musl", "x64") => "linux-musl-x64",
        ("linux-musl", _) => "linux-musl-arm64",
        ("linux", "x64") => "linux-x64",
        _ => "linux-arm64",
    })
}

fn curl(url: &str) -> Command {
    let mut c = crate::process::command("curl");
    c.args(["--silent", "--show-error", "--fail", "--location", "--max-time", "600"]).arg(url).stdin(Stdio::null());
    c
}

fn get_json(url: &str) -> anyhow::Result<serde_json::Value> {
    let out = curl(url).output().context("could not run curl")?;
    if !out.status.success() {
        bail!("{url}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("unexpected answer from {url}"))
}

/// Whether the server at `dll` came with the Razor extension (installs from nuget.org do; the older
/// `vs-impl` feed's packages didn't), so `.razor` and `.cshtml` files can be served by it.
pub fn has_razor(dll: &Path) -> bool {
    dll.with_file_name("Microsoft.VisualStudioCode.RazorExtension.dll").is_file()
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

/// The newest version on nuget.org: where its packages are, and the version.
pub fn latest() -> anyhow::Result<(String, String)> {
    let versions = get_json(&format!("{FEED}/{}/index.json", package_id()?))?;
    let newest = newest(versions.get("versions").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()))
        .context("nuget.org lists no versions")?;
    Ok((FEED.to_owned(), newest))
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
    let id = package_id()?;
    let url = format!("{base}/{id}/{version}/{id}.{version}.nupkg");
    let out = curl(&url).arg("--output").arg(&package).output().context("could not run curl")?;
    if !out.status.success() {
        bail!("downloading the Roslyn language server failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let result = verify_and_unpack(&package, &dir.join(version));
    let _ = std::fs::remove_file(&package);
    result
}

fn verify_and_unpack(package: &Path, target: &Path) -> anyhow::Result<PathBuf> {
    let out = crate::process::command("dotnet")
        .args(["nuget", "verify", "--all", "-v", "normal"])
        .arg(package)
        .env("DOTNET_NOLOGO", "1")
        // signed_by_microsoft reads the English output (a German system would say "erfolgreich überprüft")
        .env("DOTNET_CLI_UI_LANGUAGE", "en")
        .stdin(Stdio::null())
        .output()
        .context("could not run dotnet to check the package signature (is the .NET SDK installed?)")?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() || !signed_by_microsoft(&text) {
        bail!("the downloaded package isn't signed by Microsoft, so it wasn't installed:\n{}", text.trim());
    }
    if target.exists() && std::fs::remove_dir_all(target).is_err() {
        bail!("{} is in use (a server from it is still running); restart ByteForge and try again", target.display());
    }
    // Not with_extension: that would cut the version's last part ("5.4.0-2.26179.14" → "5.4.0-2.26179.partial").
    let partial = partial_dir(target);
    let _ = std::fs::remove_dir_all(&partial);
    unpack(package, &partial)?;
    std::fs::rename(&partial, target).with_context(|| format!("could not move the server into {}", target.display()))?;
    let dll = target.join(DLL);
    if !dll.is_file() {
        bail!("the package has no {DLL}");
    }
    Ok(dll)
}

/// Where a version is unpacked before it's moved into place: `<version>.partial` next to it.
fn partial_dir(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    target.with_file_name(name)
}

/// Whether `latest` is newer than every installed version (an update never goes back to an older one).
pub fn is_newer_than_installed(latest: &str, installed: &[(String, PathBuf)]) -> bool {
    installed.iter().all(|(v, _)| crate::nuget::compare_versions(latest, v) == std::cmp::Ordering::Greater)
}

/// Removes what interrupted installs left behind (`*.partial` folders).
pub fn remove_partial_installs(cache: &Path) {
    for entry in std::fs::read_dir(cache.join("roslyn")).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().ends_with(".partial") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The path of a package entry inside the tool's folder, `tools/<framework>/<rid>/` (whatever the
/// framework is), or None for entries outside it.
fn tool_path(name: &Path, rid: &str) -> Option<PathBuf> {
    let mut parts = name.components();
    let tools = parts.next()?.as_os_str();
    let (_framework, in_rid) = (parts.next()?, parts.next()?.as_os_str());
    let rel: PathBuf = parts.collect();
    (tools == "tools" && in_rid == rid && !rel.as_os_str().is_empty()).then_some(rel)
}

/// Unpacks the package's tool folder for this platform (and nothing outside it) into `target`.
fn unpack(package: &Path, target: &Path) -> anyhow::Result<()> {
    unpack_for(package, target, runtime_id()?)
}

fn unpack_for(package: &Path, target: &Path, rid: &str) -> anyhow::Result<()> {
    let file = std::fs::File::open(package)?;
    let mut zip = zip::ZipArchive::new(file).context("the download isn't a NuGet package")?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(name) = entry.enclosed_name() else { continue }; // no "../" escapes
        let Some(rel) = tool_path(&name, rid) else { continue };
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
    let Ok(out) = crate::process::command("dotnet").arg("--list-runtimes").stdin(Stdio::null()).output() else { return false };
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
    fn installs_only_go_forward() {
        let installed = vec![("5.13.0-1.26514.10".to_owned(), PathBuf::new()), ("5.4.0-2.26179.14".to_owned(), PathBuf::new())];
        assert!(!is_newer_than_installed("5.13.0-1.26514.10", &installed), "same version");
        assert!(!is_newer_than_installed("5.4.0-2.26179.14", &installed), "older than one installed");
        assert!(is_newer_than_installed("5.14.0-1.26600.1", &installed));
        assert!(is_newer_than_installed("5.4.0-2.26179.14", &[]));
        assert_eq!(partial_dir(Path::new("/c/roslyn/5.4.0-2.26179.14")), PathBuf::from("/c/roslyn/5.4.0-2.26179.14.partial"));
    }

    #[test]
    fn this_platform_has_a_package() {
        let id = package_id().unwrap();
        assert!(id.starts_with("roslyn-language-server.") && (id.ends_with("-x64") || id.ends_with("-arm64")), "{id}");
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
            let tool = "tools/net10.0/win-x64/";
            for (name, body) in [(format!("{tool}{DLL}"), "dll"), (format!("{tool}Targets/x.targets"), "<Project/>"), ("lib/net10.0/other.dll".to_owned(), "no"), ("tools/any/any/DotnetToolSettings.xml".to_owned(), "")] {
                zip.start_file(name, options).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        let target = dir.join("out");
        unpack_for(&package, &target, "win-x64").unwrap();
        assert!(target.join(DLL).is_file() && target.join("Targets/x.targets").is_file());
        assert!(!target.join("DotnetToolSettings.xml").exists(), "only this platform's tool folder");
        assert!(!target.join("other.dll").exists() && !dir.join("lib").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
