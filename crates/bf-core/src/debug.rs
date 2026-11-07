//! .NET debugging helpers: what to launch (the built assembly, its launch profile), MSBuild
//! diagnostics from build output, and running .NET processes to attach to.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::Value;

/// A profile from `Properties/launchSettings.json` (only `"commandName": "Project"` profiles apply).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchProfile {
    pub name: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// `applicationUrl`, e.g. `https://localhost:7001;http://localhost:5001` (becomes `ASPNETCORE_URLS`).
    pub application_url: Option<String>,
    pub launch_browser: bool,
    /// `launchUrl`, relative to the first application URL (e.g. `swagger`).
    pub launch_url: Option<String>,
}

impl LaunchProfile {
    /// The page to open in a browser once the app listens, if the profile asks for one.
    pub fn browser_url(&self, listening_on: &str) -> Option<String> {
        if !self.launch_browser {
            return None;
        }
        let base = listening_on.trim_end_matches('/');
        Some(match self.launch_url.as_deref().filter(|u| !u.is_empty()) {
            Some(url) if url.contains("://") => url.to_owned(),
            Some(url) => format!("{base}/{}", url.trim_start_matches('/')),
            None => base.to_owned(),
        })
    }
}

/// Reads the launch profile of `project` (a .csproj): the one named `wanted`, else the first
/// `"commandName": "Project"` profile. `None` if the project has no launch settings.
pub fn launch_profile(project: &Path, wanted: Option<&str>) -> Option<LaunchProfile> {
    let file = project.parent()?.join("Properties").join("launchSettings.json");
    let text = std::fs::read_to_string(file).ok()?;
    parse_launch_settings(&text, wanted)
}

fn parse_launch_settings(text: &str, wanted: Option<&str>) -> Option<LaunchProfile> {
    let root: Value = json5::from_str(text).ok()?;
    let profiles = root.get("profiles")?.as_object()?;
    let is_project = |p: &Value| p.get("commandName").and_then(Value::as_str).is_none_or(|c| c == "Project");
    let (name, profile) = wanted
        .filter(|w| !w.is_empty())
        .and_then(|w| profiles.iter().find(|(name, _)| name.as_str() == w))
        .or_else(|| profiles.iter().find(|(_, p)| is_project(p)))?;
    let text = |key: &str| profile.get(key).and_then(Value::as_str).map(str::to_owned).filter(|s| !s.is_empty());
    let mut env: Vec<(String, String)> = profile
        .get("environmentVariables")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_owned))).collect())
        .unwrap_or_default();
    env.sort();
    Some(LaunchProfile {
        name: name.clone(),
        args: text("commandLineArgs").map(|a| split_args(&a)).unwrap_or_default(),
        env,
        application_url: text("applicationUrl"),
        launch_browser: profile.get("launchBrowser").and_then(Value::as_bool).unwrap_or(false),
        launch_url: text("launchUrl"),
    })
}

/// Splits a command line on whitespace, honoring double quotes.
pub fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut any) = (false, false);
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    args.push(std::mem::take(&mut current));
                    any = false;
                }
            }
            c => {
                current.push(c);
                any = true;
            }
        }
    }
    if any {
        args.push(current);
    }
    args
}

/// The assembly `dotnet build` produces for `project` (asks MSBuild, so custom output paths work).
/// Blocking: runs `dotnet msbuild` (about a second).
pub fn target_assembly(project: &Path) -> Result<PathBuf> {
    let props = msbuild_properties(project, &["TargetPath", "TargetFrameworks"], None)?;
    let mut path = props.first().cloned().unwrap_or_default();
    if path.is_empty() {
        // Multi-targeting projects have no TargetPath until a framework is chosen.
        let first = props.get(1).and_then(|f| f.split(';').map(str::trim).find(|f| !f.is_empty()).map(str::to_owned));
        let Some(framework) = first else { bail!("MSBuild reported no TargetPath for {}", project.display()) };
        path = msbuild_properties(project, &["TargetPath"], Some(&framework))?.remove(0);
    }
    if path.is_empty() {
        bail!("MSBuild reported no TargetPath for {}", project.display());
    }
    Ok(PathBuf::from(path))
}

fn msbuild_properties(project: &Path, names: &[&str], framework: Option<&str>) -> Result<Vec<String>> {
    let mut command = Command::new("dotnet");
    command.arg("msbuild").arg(project).arg("-nologo");
    for name in names {
        command.arg(format!("-getProperty:{name}"));
    }
    if let Some(framework) = framework {
        command.arg(format!("-p:TargetFramework={framework}"));
    }
    command.current_dir(project.parent().unwrap_or(Path::new("."))).env("DOTNET_NOLOGO", "1").env("DOTNET_CLI_TELEMETRY_OPTOUT", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().context("could not run dotnet msbuild")?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() {
        bail!("dotnet msbuild failed: {}", stdout.lines().last().unwrap_or_default());
    }
    if names.len() == 1 {
        return Ok(vec![stdout]);
    }
    // Several properties come back as {"Properties": {"Name": "value", …}}.
    let json: Value = serde_json::from_str(&stdout).map_err(|e| anyhow!("unexpected msbuild output ({e}): {stdout}"))?;
    Ok(names.iter().map(|n| json["Properties"][*n].as_str().unwrap_or_default().to_owned()).collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildSeverity {
    Error,
    Warning,
}

/// A compiler or MSBuild diagnostic from build output; `line` and `column` are 1-based (0 if unknown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildDiagnostic {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub severity: BuildSeverity,
    pub code: Option<String>,
    pub message: String,
}

/// Parses one line of `dotnet build` output, e.g.
/// `/src/Pages/Index.cshtml(12,5): error CS0103: The name 'x' does not exist [/src/App.csproj]`.
/// Relative paths are resolved against `cwd`.
pub fn parse_build_line(line: &str, cwd: &Path) -> Option<BuildDiagnostic> {
    let line = line.trim();
    let (at, severity, rest) = [(": error ", BuildSeverity::Error), (": warning ", BuildSeverity::Warning)]
        .into_iter()
        .find_map(|(marker, severity)| line.find(marker).map(|i| (i, severity, &line[i + marker.len()..])))?;
    let location = line[..at].trim_end();
    let (file, line_no, column) = match location.strip_suffix(')').and_then(|l| l.rsplit_once('(')) {
        Some((file, pos)) => {
            let mut nums = pos.split(',').map(|n| n.trim().parse::<u32>());
            let line_no = nums.next()?.ok()?;
            let column = nums.next().and_then(Result::ok).unwrap_or(1);
            (file, line_no, column)
        }
        None => (location, 0, 0),
    };
    if file.is_empty() || file.starts_with("MSBUILD") || file.contains(": ") {
        return None;
    }
    let (code, message) = match rest.split_once(": ") {
        Some((code, message)) if !code.contains(' ') => (Some(code.to_owned()), message),
        _ => (None, rest),
    };
    // Drop the trailing " [/path/Project.csproj]".
    let message = match message.rfind(" [") {
        Some(i) if message.ends_with(']') => &message[..i],
        _ => message,
    };
    let path = PathBuf::from(file);
    Some(BuildDiagnostic {
        path: if path.is_absolute() { path } else { cwd.join(path) },
        line: line_no,
        column,
        severity,
        code,
        message: message.trim().to_owned(),
    })
}

/// The URL in an ASP.NET Core "Now listening on: http://localhost:5000" log line.
pub fn listening_url(line: &str) -> Option<&str> {
    let rest = &line[line.find("Now listening on:")? + "Now listening on:".len()..];
    Some(rest.trim()).filter(|u| u.starts_with("http"))
}

/// Opens a URL in the default browser.
pub fn open_browser(url: &str) {
    let result = if cfg!(windows) {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = result {
        log::warn!("could not open {url}: {e}");
    }
}

/// A running process that hosts the .NET runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeProcess {
    pub pid: u32,
    pub name: String,
    pub command_line: String,
}

/// Running .NET processes of the current user (they load the CoreCLR runtime).
pub fn dotnet_processes() -> Vec<RuntimeProcess> {
    let mut out = Vec::new();
    #[cfg(target_os = "linux")]
    {
        let own = std::process::id();
        let Ok(entries) = std::fs::read_dir("/proc") else { return out };
        for entry in entries.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            if pid == own {
                continue;
            }
            let dir = entry.path();
            // Other users' maps aren't readable, which conveniently skips them.
            let Ok(maps) = std::fs::read_to_string(dir.join("maps")) else { continue };
            if !maps.contains("libcoreclr.so") {
                continue;
            }
            let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
            let command_line = cmdline.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect::<Vec<_>>().join(" ");
            let name = std::fs::read_to_string(dir.join("comm")).map(|c| c.trim().to_owned()).unwrap_or_default();
            out.push(RuntimeProcess { pid, name, command_line });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = Command::new("tasklist").args(["/m", "coreclr.dll", "/fo", "csv", "/nh"]).creation_flags(0x0800_0000).output();
        if let Ok(output) = output {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let cols: Vec<&str> = line.split("\",\"").map(|c| c.trim_matches('"')).collect();
                if let (Some(name), Some(Ok(pid))) = (cols.first(), cols.get(1).map(|p| p.parse::<u32>())) {
                    out.push(RuntimeProcess { pid, name: (*name).to_owned(), command_line: (*name).to_owned() });
                }
            }
        }
    }
    out.sort_by_key(|p| p.pid);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_build_lines() {
        let cwd = Path::new("/w");
        let d = parse_build_line("/src/Pages/Index.cshtml(12,5): error CS0103: The name 'x' does not exist in the current context [/src/App.csproj]", cwd).unwrap();
        assert_eq!(d.path, PathBuf::from("/src/Pages/Index.cshtml"));
        assert_eq!((d.line, d.column, d.severity), (12, 5, BuildSeverity::Error));
        assert_eq!(d.code.as_deref(), Some("CS0103"));
        assert_eq!(d.message, "The name 'x' does not exist in the current context");

        let d = parse_build_line("  Program.cs(3,1,3,9): warning CS8321: The local function 'f' is declared but never used [/w/A.csproj]", cwd).unwrap();
        assert_eq!(d.path, PathBuf::from("/w/Program.cs"));
        assert_eq!((d.line, d.column, d.severity), (3, 1, BuildSeverity::Warning));

        let d = parse_build_line("/w/A.csproj : error NU1101: Unable to find package Foo.", cwd).unwrap();
        assert_eq!((d.line, d.code.as_deref()), (0, Some("NU1101")));

        assert!(parse_build_line("Build succeeded.", cwd).is_none());
        assert!(parse_build_line("    0 Error(s)", cwd).is_none());
    }

    #[test]
    fn reads_launch_settings() {
        let text = r#"{
          // comments are allowed
          "profiles": {
            "IIS Express": { "commandName": "IISExpress" },
            "https": {
              "commandName": "Project",
              "launchBrowser": true,
              "launchUrl": "swagger",
              "applicationUrl": "https://localhost:7001;http://localhost:5001",
              "commandLineArgs": "--seed \"two words\"",
              "environmentVariables": { "ASPNETCORE_ENVIRONMENT": "Development" }
            },
          }
        }"#;
        let p = parse_launch_settings(text, None).unwrap();
        assert_eq!(p.name, "https");
        assert_eq!(p.args, vec!["--seed", "two words"]);
        assert_eq!(p.env, vec![("ASPNETCORE_ENVIRONMENT".to_owned(), "Development".to_owned())]);
        assert_eq!(p.application_url.as_deref(), Some("https://localhost:7001;http://localhost:5001"));
        assert_eq!(p.browser_url("https://localhost:7001/").as_deref(), Some("https://localhost:7001/swagger"));
        assert_eq!(parse_launch_settings(text, Some("IIS Express")).unwrap().name, "IIS Express");
    }

    #[test]
    fn finds_listening_url() {
        assert_eq!(listening_url("info: Microsoft.Hosting.Lifetime[14]\n      Now listening on: http://localhost:5000"), Some("http://localhost:5000"));
        assert_eq!(listening_url("      Now listening on: https://localhost:7001"), Some("https://localhost:7001"));
        assert_eq!(listening_url("Application started."), None);
    }
}
