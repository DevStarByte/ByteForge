//! Razor through Microsoft's Roslyn language server (the `roslyn-language-server` package from nuget.org,
//! which carries the Razor extension next to the server): a Blazor component gets diagnostics,
//! completion and hover. Needs the server: set BF_ROSLYN_DLL to its
//! `Microsoft.CodeAnalysis.LanguageServer.dll` (skips otherwise), and the .NET 10 SDK.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use bf_lsp::{LspClient, LspEvent, Position, roslyn_config};

const COMPONENT: &str = r#"@page "/counter"

<h1>Count: @currentCount</h1>
<button class="btn" @onclick="IncrementCount">Click me</button>

@code {
    private int currentCount = 0;
    private string label = "clicks";

    private void IncrementCount()
    {
        currentCount++;
        int wrong = "text";
        var n = label.Length;
    }
}
"#;

const CSPROJ: &str = r#"<Project Sdk="Microsoft.NET.Sdk.Web">
  <PropertyGroup><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings><Nullable>enable</Nullable></PropertyGroup>
</Project>
"#;

const IMPORTS: &str = "@using Microsoft.AspNetCore.Components.Web\n";

const PROGRAM: &str = "var app = WebApplication.CreateBuilder(args).Build();\napp.Run();\n";

fn find(needle: &str, offset: u32) -> Position {
    let (line, text) = COMPONENT.lines().enumerate().find(|(_, l)| l.contains(needle)).expect("needle");
    Position { line: line as u32, character: text.find(needle).unwrap() as u32 + offset }
}

fn recv<T>(rx: flume::Receiver<anyhow::Result<T>>, what: &str) -> T {
    rx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|e| panic!("{what}: no response ({e})")).unwrap_or_else(|e| panic!("{what}: {e:#}"))
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Prints what the client refuses, so a Razor request it doesn't answer shows up in the test output.
struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        let text = record.args().to_string();
        if text.contains("unsupported server request") {
            println!("           client: {text}");
        }
    }
    fn flush(&self) {}
}

fn bf_lsp_test_long_path(path: &Path) -> PathBuf {
    let long = std::fs::canonicalize(path).unwrap();
    // canonicalize gives `\\?\C:\…` on Windows; the editor uses plain paths.
    PathBuf::from(long.to_string_lossy().trim_start_matches(r"\\?\"))
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn razor_component_end_to_end() {
    let Ok(dll) = std::env::var("BF_ROSLYN_DLL") else {
        println!("SKIPPED: set BF_ROSLYN_DLL to Microsoft.CodeAnalysis.LanguageServer.dll");
        return;
    };
    let _ = log::set_logger(&Logger).map(|()| log::set_max_level(log::LevelFilter::Debug));
    let dir = std::env::temp_dir().join(format!("bf-roslyn-razor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    // The long form of the path (Windows' TEMP can be an 8.3 short path), as the editor has it.
    let dir = bf_lsp_test_long_path(&dir);
    let guard = TempDir(dir.clone());
    let project = dir.join("Web.csproj");
    let component = dir.join("Components").join("Counter.razor");
    write(&project, CSPROJ);
    write(&dir.join("Program.cs"), PROGRAM);
    write(&dir.join("Components").join("_Imports.razor"), IMPORTS);
    write(&component, COMPONENT);
    assert!(Command::new("dotnet").args(["restore", "-v", "q"]).current_dir(&dir).status().unwrap().success());

    let t0 = Instant::now();
    let config = roslyn_config(PathBuf::from(&dll).as_path(), dir.clone(), &dir.join("logs"), None, std::slice::from_ref(&project));
    let (client, events) = LspClient::start(config).expect("start the Roslyn server");
    client.did_open(&component, 1, COMPONENT);

    // The C# error inside @code comes back as a diagnostic of the .razor file.
    let deadline = t0 + Duration::from_secs(120);
    let mut diagnostics = Vec::new();
    while let Ok(event) = events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        match event {
            LspEvent::Diagnostics { diagnostics: d, .. } => {
                diagnostics = d;
                if diagnostics.iter().any(|d| d.code.as_deref() == Some("CS0029")) {
                    break;
                }
            }
            LspEvent::Exited(reason) => panic!("server exited: {reason}"),
            _ => {}
        }
    }
    println!("[{:>5.1?}] diagnostics: {:?}", t0.elapsed(), diagnostics.iter().map(|d| d.code.clone()).collect::<Vec<_>>());

    let error = diagnostics.iter().find(|d| d.code.as_deref() == Some("CS0029")).expect("CS0029 in the .razor file");
    assert_eq!(error.range.start.line, find("int wrong", 0).line, "on the line in the .razor file");

    let completion = recv(client.completion(&component, find("label.Length", 6)), "completion");
    println!("           completion after `label.`: {} items", completion.len());
    assert!(completion.iter().any(|c| c.label == "Length"));
    let hover = recv(client.hover(&component, find("currentCount++", 2)), "hover").unwrap_or_default();
    assert!(hover.contains("currentCount"), "{hover}");
    let definition = recv(client.definition(&component, find("IncrementCount\"", 2)), "definition");
    assert_eq!(definition.first().map(|l| l.range.start.line), Some(find("void IncrementCount", 0).line));
    // In the markup the server asks the editor for HTML results; without an HTML server it still answers.
    recv(client.completion(&component, find("class=\"btn\"", 1)), "completion in HTML");

    client.shutdown();
    let _ = client.wait_for_exit(Duration::from_secs(10));
    drop(guard);
}
