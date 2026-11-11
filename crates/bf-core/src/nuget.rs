//! NuGet through the dotnet CLI: searching the configured feeds (`dotnet package search`) and finding
//! outdated package references (`dotnet list package --outdated`). Adding, updating and removing run as
//! ordinary dotnet commands in the workbench (`dotnet add/remove package`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, bail};
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageInfo {
    pub id: String,
    pub version: String,
    pub downloads: u64,
    pub owners: String,
}

/// project → package id (lowercase) → latest version.
pub type Outdated = HashMap<PathBuf, HashMap<String, String>>;

fn dotnet(dir: &Path, args: &[&str]) -> anyhow::Result<(bool, String, String)> {
    let mut command = Command::new("dotnet");
    command
        .args(args)
        .current_dir(dir)
        .env("DOTNET_NOLOGO", "1")
        .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = command.output().context("could not start dotnet")?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

/// Searches the package feeds (nuget.org and whatever NuGet.config adds) for `query`.
pub fn search(dir: &Path, query: &str, prerelease: bool) -> anyhow::Result<Vec<PackageInfo>> {
    let mut args = vec!["package", "search", query, "--take", "30", "--format", "json"];
    if prerelease {
        args.push("--prerelease");
    }
    let (ok, stdout, stderr) = dotnet(dir, &args)?;
    if !ok && stdout.trim().is_empty() {
        bail!("{}", first_line(&stderr));
    }
    parse_search(&stdout)
}

pub fn parse_search(json: &str) -> anyhow::Result<Vec<PackageInfo>> {
    #[derive(Deserialize)]
    struct Root {
        #[serde(default)]
        problems: Vec<Problem>,
        #[serde(default, rename = "searchResult")]
        search_result: Vec<Source>,
    }
    #[derive(Deserialize)]
    struct Problem {
        #[serde(default)]
        text: String,
    }
    #[derive(Deserialize)]
    struct Source {
        #[serde(default)]
        packages: Vec<Package>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Package {
        id: String,
        #[serde(default)]
        latest_version: String,
        #[serde(default)]
        total_downloads: u64,
        #[serde(default)]
        owners: serde_json::Value,
    }
    let root: Root = serde_json::from_str(json.trim()).context("unexpected output from dotnet package search")?;
    let mut seen = std::collections::HashSet::new();
    let packages: Vec<PackageInfo> = root
        .search_result
        .into_iter()
        .flat_map(|s| s.packages)
        // Several feeds can list the same package; the first one wins.
        .filter(|p| seen.insert(p.id.to_lowercase()))
        .map(|p| PackageInfo {
            id: p.id,
            version: p.latest_version,
            downloads: p.total_downloads,
            owners: match p.owners {
                serde_json::Value::String(s) => s,
                serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "),
                _ => String::new(),
            },
        })
        .collect();
    if packages.is_empty()
        && let Some(problem) = root.problems.first()
    {
        bail!("{}", problem.text);
    }
    Ok(packages)
}

/// Every version of package `id` on the feeds, prereleases included: the latest stable one first, then
/// the rest newest first.
pub fn versions(dir: &Path, id: &str) -> anyhow::Result<Vec<String>> {
    let (ok, stdout, stderr) = dotnet(dir, &["package", "search", id, "--exact-match", "--prerelease", "--format", "json"])?;
    if !ok && stdout.trim().is_empty() {
        bail!("{}", first_line(&stderr));
    }
    Ok(order_versions(parse_versions(&stdout)?))
}

/// The versions in `dotnet package search --exact-match --format json` output (several feeds merged).
pub fn parse_versions(json: &str) -> anyhow::Result<Vec<String>> {
    let root: serde_json::Value = serde_json::from_str(json.trim()).context("unexpected output from dotnet package search")?;
    let mut out: Vec<String> = Vec::new();
    for source in root.get("searchResult").and_then(|v| v.as_array()).into_iter().flatten() {
        for package in source.get("packages").and_then(|v| v.as_array()).into_iter().flatten() {
            if let Some(v) = package.get("version").and_then(|v| v.as_str())
                && !out.iter().any(|o| o.eq_ignore_ascii_case(v))
            {
                out.push(v.to_owned());
            }
        }
    }
    Ok(out)
}

/// Whether a NuGet version is a prerelease (`1.0.0-beta.2`).
pub fn is_prerelease(version: &str) -> bool {
    version.split('+').next().unwrap_or(version).contains('-')
}

/// Latest stable first (what Enter takes), then everything else newest first.
pub fn order_versions(mut versions: Vec<String>) -> Vec<String> {
    versions.sort_by(|a, b| compare_versions(b, a));
    if let Some(stable) = versions.iter().position(|v| !is_prerelease(v)) {
        let v = versions.remove(stable);
        versions.insert(0, v);
    }
    versions
}

/// Orders NuGet versions: numeric parts (missing ones are 0), then a release above its prereleases, then
/// the prerelease labels part by part (numbers numerically, below words). Build metadata (`+…`) is ignored.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let split = |v: &str| {
        let v = v.split('+').next().unwrap_or(v);
        let (release, pre) = v.split_once('-').map_or((v, None), |(r, p)| (r, Some(p)));
        let numbers: Vec<u64> = release.split('.').map(|n| n.parse().unwrap_or(0)).collect();
        (numbers, pre.map(str::to_owned))
    };
    let ((na, pa), (nb, pb)) = (split(a), split(b));
    for i in 0..na.len().max(nb.len()) {
        match na.get(i).unwrap_or(&0).cmp(nb.get(i).unwrap_or(&0)) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(pa), Some(pb)) => {
            let (ia, ib): (Vec<&str>, Vec<&str>) = (pa.split(['.', '-']).collect(), pb.split(['.', '-']).collect());
            for (x, y) in ia.iter().zip(&ib) {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.to_lowercase().cmp(&y.to_lowercase()),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
            ia.len().cmp(&ib.len())
        }
    }
}

/// Lists the outdated top-level packages of a project or solution. Runs `dotnet restore` first when the
/// project has never been restored (listing needs the assets file).
pub fn outdated(dir: &Path, target: &Path) -> anyhow::Result<Outdated> {
    let target_arg = target.to_string_lossy().into_owned();
    let list = || dotnet(dir, &["list", &target_arg, "package", "--outdated", "--format", "json"]);
    let (mut ok, mut stdout, mut stderr) = list()?;
    // Listing fails on a project that was never restored (no assets file): restore once and retry.
    if !ok {
        let (restored, out, err) = dotnet(dir, &["restore", &target_arg])?;
        if !restored {
            bail!("dotnet restore failed: {}", first_line(if err.trim().is_empty() { &out } else { &err }));
        }
        (ok, stdout, stderr) = list()?;
    }
    match parse_outdated(&stdout) {
        Ok(result) => Ok(result),
        Err(e) if !ok => bail!("{} ({e})", first_line(if stderr.trim().is_empty() { &stdout } else { &stderr })),
        Err(e) => Err(e),
    }
}

pub fn parse_outdated(json: &str) -> anyhow::Result<Outdated> {
    #[derive(Deserialize)]
    struct Root {
        #[serde(default)]
        projects: Vec<Project>,
    }
    #[derive(Deserialize)]
    struct Project {
        path: PathBuf,
        #[serde(default)]
        frameworks: Vec<Framework>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Framework {
        #[serde(default)]
        top_level_packages: Vec<Package>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Package {
        id: String,
        #[serde(default)]
        latest_version: Option<String>,
    }
    // Text before the JSON (restore messages) is skipped.
    let start = json.find('{').context("no JSON in the dotnet list package output")?;
    let root: Root = serde_json::from_str(&json[start..]).context("unexpected output from dotnet list package")?;
    let mut result = Outdated::new();
    for project in root.projects {
        let entry = result.entry(project.path).or_default();
        for package in project.frameworks.into_iter().flat_map(|f| f.top_level_packages) {
            // "Not found at the sources" is reported as a latest version too; keep only real versions.
            if let Some(latest) = package.latest_version.filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit())) {
                entry.insert(package.id.to_lowercase(), latest);
            }
        }
    }
    Ok(result)
}

fn first_line(text: &str) -> &str {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("failed")
}

/// "3.5B", "12.1M", "830K", "412".
pub fn format_downloads(n: u64) -> String {
    match n {
        1_000_000_000.. => format!("{:.1}B", n as f64 / 1e9),
        1_000_000.. => format!("{:.1}M", n as f64 / 1e6),
        1_000.. => format!("{}K", n / 1000),
        _ => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_results() {
        let json = r#"{"version":2,"problems":[],"searchResult":[
            {"sourceName":"nuget.org","packages":[{"id":"Serilog","latestVersion":"4.4.0","totalDownloads":3568473511,"owners":"serilog"}]},
            {"sourceName":"local","packages":[{"id":"serilog","latestVersion":"1.0.0"},{"id":"Mine","latestVersion":"0.1.0","owners":["me","you"]}]}]}"#;
        let p = parse_search(json).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0], PackageInfo { id: "Serilog".into(), version: "4.4.0".into(), downloads: 3568473511, owners: "serilog".into() });
        assert_eq!(p[1].owners, "me, you");
        assert_eq!(format_downloads(p[0].downloads), "3.6B");
        let err = parse_search(r#"{"problems":[{"text":"Unable to load the service index"}],"searchResult":[]}"#).unwrap_err();
        assert!(err.to_string().contains("service index"));
    }

    #[test]
    fn parses_outdated_packages() {
        let json = r#"  Determining projects to restore...
{"version":1,"projects":[{"path":"/w/A/A.csproj"},{"path":"/w/B/B.csproj","frameworks":[{"framework":"net10.0","topLevelPackages":[
            {"id":"Newtonsoft.Json","requestedVersion":"12.0.1","resolvedVersion":"12.0.1","latestVersion":"13.0.4"},
            {"id":"Gone","requestedVersion":"1.0.0","resolvedVersion":"1.0.0","latestVersion":"Not found at the sources"}]}]}]}"#;
        let o = parse_outdated(json).unwrap();
        assert!(o[Path::new("/w/A/A.csproj")].is_empty());
        let b = &o[Path::new("/w/B/B.csproj")];
        assert_eq!(b.get("newtonsoft.json").map(String::as_str), Some("13.0.4"));
        assert!(!b.contains_key("gone"));
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn orders_versions_like_nuget() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("13.0.3", "13.0.10"), Less);
        assert_eq!(compare_versions("2.0.0", "2.0.0-beta02-0092"), Greater);
        assert_eq!(compare_versions("1.0.0-beta.2", "1.0.0-beta.10"), Less);
        assert_eq!(compare_versions("1.0.0-alpha", "1.0.0-beta"), Less);
        assert_eq!(compare_versions("1.0.0-rc.1", "1.0.0-rc.1.1"), Less);
        assert_eq!(compare_versions("2.0", "2.0.0.0"), Equal);
        assert_eq!(compare_versions("1.0.0+build.5", "1.0.0"), Equal);
    }

    #[test]
    fn latest_stable_comes_first() {
        let all = ["2.0.0-beta02-0092", "2.0.0", "3.0.0-rc.1", "2.14.1", "3.0.0-beta.54"].map(String::from).to_vec();
        assert_eq!(order_versions(all), ["2.14.1", "3.0.0-rc.1", "3.0.0-beta.54", "2.0.0", "2.0.0-beta02-0092"]);
        assert_eq!(order_versions(vec!["1.0.0-preview.1".into()]), ["1.0.0-preview.1"]);
    }

    #[test]
    fn parses_exact_match_output() {
        let json = r#"{"version":2,"problems":[],"searchResult":[{"sourceName":"nuget.org","packages":[{"id":"A","version":"1.0.0"},{"id":"A","version":"1.1.0-beta"}]},{"sourceName":"local","packages":[{"id":"A","version":"1.0.0"}]}]}"#;
        assert_eq!(parse_versions(json).unwrap(), ["1.0.0", "1.1.0-beta"]);
        assert!(is_prerelease("1.1.0-beta") && !is_prerelease("1.1.0+sha.abc-def"));
    }
}
