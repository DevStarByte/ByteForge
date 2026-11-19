//! Builds real projects with the .NET SDK: finds the built assembly, and reads a Razor compile error
//! from `dotnet build` output. Skips (passes) with a message when `dotnet` is not installed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

use bf_core::debug::{BuildSeverity, parse_build_line, target_assembly};

fn target_framework() -> Option<String> {
    let output = Command::new("dotnet").arg("--list-sdks").output().ok()?;
    let major = String::from_utf8_lossy(&output.stdout).lines().filter_map(|l| l.split('.').next()?.trim().parse::<u32>().ok()).max()?;
    Some(format!("net{major}.0"))
}

/// The newest framework with an ASP.NET Core runtime installed (web projects need it to build and run).
fn web_framework() -> Option<String> {
    let output = Command::new("dotnet").arg("--list-runtimes").output().ok()?;
    let major = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("Microsoft.AspNetCore.App ")?.split('.').next()?.parse::<u32>().ok())
        .max()?;
    Some(format!("net{major}.0"))
}

/// One `dotnet build` at a time, without build servers: on a fresh CI runner (macOS especially) two
/// parallel first builds race over NuGet restore and the shared MSBuild/compiler server processes and
/// fail now and then.
static DOTNET: Mutex<()> = Mutex::new(());

fn dotnet_build(project: &Path) -> Output {
    let _one_at_a_time = DOTNET.lock().unwrap_or_else(|e| e.into_inner());
    Command::new("dotnet").arg("build").arg(project).arg("--disable-build-servers").output().unwrap()
}

fn shown(output: &Output) -> String {
    format!("exit {:?}
--- stdout ---
{}
--- stderr ---
{}", output.status.code(), String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

struct TempProject(PathBuf);

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_project(name: &str, files: &[(&str, String)]) -> TempProject {
    let dir = std::env::temp_dir().join(format!("bf-core-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    // The long path (TEMP can be an 8.3 short name), without Windows' `\\?\` prefix, which MSBuild can't use.
    let dir = std::fs::canonicalize(&dir).unwrap();
    TempProject(PathBuf::from(dir.to_string_lossy().trim_start_matches(r"\\?\")))
}

#[test]
fn target_assembly_of_built_project() {
    let Some(tfm) = target_framework() else {
        println!("SKIPPED: dotnet SDK not found");
        return;
    };
    let csproj = format!(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFrameworks>{tfm}</TargetFrameworks></PropertyGroup></Project>"
    );
    let project = temp_project("target", &[("Multi.csproj", csproj), ("Program.cs", "System.Console.WriteLine(1);".into())]);
    let csproj = project.0.join("Multi.csproj");
    let output = dotnet_build(&csproj);
    assert!(output.status.success(), "{}", shown(&output));
    // TargetFrameworks (plural) has no TargetPath until one framework is picked.
    let program = {
        let _one_at_a_time = DOTNET.lock().unwrap_or_else(|e| e.into_inner());
        target_assembly(&csproj).unwrap()
    };
    assert!(program.ends_with(Path::new(&tfm).join("Multi.dll")), "{}", program.display());
    assert!(program.is_file());
}

#[test]
fn razor_errors_from_build_output() {
    let Some(tfm) = web_framework() else {
        println!("SKIPPED: no ASP.NET Core runtime installed");
        return;
    };
    let csproj = format!("<Project Sdk=\"Microsoft.NET.Sdk.Web\"><PropertyGroup><TargetFramework>{tfm}</TargetFramework></PropertyGroup></Project>");
    let program = "var app = Microsoft.AspNetCore.Builder.WebApplication.CreateBuilder(args).Build(); app.Run();";
    let page = "@page\n<h1>Hello</h1>\n<p>@notDefined</p>\n";
    let project = temp_project("razor", &[("Web.csproj", csproj), ("Program.cs", program.into()), ("Pages/Index.cshtml", page.into())]);
    let output = dotnet_build(&project.0.join("Web.csproj"));
    assert!(!output.status.success(), "the build should fail:
{}", shown(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let found: Vec<_> = stdout.lines().filter_map(|l| parse_build_line(l, &project.0)).collect();
    let error = found
        .iter()
        .find(|d| d.path.ends_with(Path::new("Pages").join("Index.cshtml")))
        .unwrap_or_else(|| panic!("no Index.cshtml diagnostic in:\n{stdout}"));
    assert_eq!(error.severity, BuildSeverity::Error);
    assert_eq!(error.code.as_deref(), Some("CS0103"));
    assert_eq!((error.line, error.column), (3, 5), "{error:?}");
    assert!(error.message.contains("notDefined"), "{error:?}");
}
