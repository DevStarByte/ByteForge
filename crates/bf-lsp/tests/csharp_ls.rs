//! End-to-end test against a real `csharp-ls`. Skips (passes) with a message when
//! `csharp-ls` or `dotnet` is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use bf_lsp::{LspClient, LspEvent, Position, ServerConfig, Severity, csharp_ls_command};

const PROGRAM: &str = r#"using System;

namespace Demo;

public static class Program
{
    public static void Main()
    {
        int x = "s";
        var greeting = "hello";
        Console.WriteLine(greeting);
        Console.Wri
    }
}
"#;

const CSPROJ: &str = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>TFM</TargetFramework>
    <Nullable>enable</Nullable>
  </PropertyGroup>
</Project>
"#;

/// Picks the newest installed SDK major version as target framework.
fn target_framework() -> Option<String> {
    let output = Command::new("dotnet").arg("--list-sdks").output().ok()?;
    let major = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| l.split('.').next()?.trim().parse::<u32>().ok())
        .max()?;
    Some(format!("net{major}.0"))
}

/// Finds `(line, utf16_column)` of the first occurrence of `needle` plus `offset` chars.
fn find(text: &str, needle: &str, offset: u32) -> Position {
    let (line, content) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains(needle))
        .expect("needle");
    let col = content[..content.find(needle).unwrap()]
        .encode_utf16()
        .count() as u32;
    Position {
        line: line as u32,
        character: col + offset,
    }
}

struct TempProject(PathBuf);

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn recv<T>(rx: flume::Receiver<anyhow::Result<T>>, what: &str) -> T {
    rx.recv_timeout(Duration::from_secs(60))
        .unwrap_or_else(|e| panic!("{what}: no response ({e})"))
        .unwrap_or_else(|e| panic!("{what}: {e:#}"))
}

#[test]
fn csharp_ls_end_to_end() {
    let Some(command) = csharp_ls_command() else {
        println!("SKIPPED: csharp-ls not found on PATH or in ~/.dotnet/tools");
        return;
    };
    let Some(tfm) = target_framework() else {
        println!("SKIPPED: dotnet SDK not found");
        return;
    };
    println!("using {command} with {tfm}");

    let dir = std::env::temp_dir().join(format!("bf-lsp-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Use the long, canonical path: %TEMP% may be an 8.3 short path (C:\Users\ABCDEF~1\...)
    // while MSBuild reports long paths, and csharp-ls matches documents by exact path.
    let dir = strip_verbatim(std::fs::canonicalize(&dir).unwrap());
    let project = TempProject(dir.clone());
    std::fs::write(dir.join("Demo.csproj"), CSPROJ.replace("TFM", &tfm)).unwrap();
    let program = dir.join("Program.cs");
    std::fs::write(&program, PROGRAM).unwrap();

    let t0 = Instant::now();
    let restore = Command::new("dotnet")
        .arg("restore")
        .current_dir(&dir)
        .output()
        .expect("dotnet restore");
    assert!(
        restore.status.success(),
        "dotnet restore failed: {}",
        String::from_utf8_lossy(&restore.stdout)
    );
    println!("[{:>6.1?}] dotnet restore done", t0.elapsed());

    let t0 = Instant::now();
    let (client, events) = LspClient::start(ServerConfig {
        command,
        args: vec![],
        root: dir.clone(),
        language_id: "csharp".into(),
    })
    .expect("start csharp-ls");
    // Issued before Ready: must be queued and delivered after the handshake.
    client.did_open(&program, 1, PROGRAM);

    let deadline = t0 + Duration::from_secs(120);
    let mut ready = false;
    let mut found_error = false;
    while !found_error {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = events
            .recv_timeout(remaining)
            .unwrap_or_else(|_| panic!("timed out waiting for diagnostics"));
        match event {
            LspEvent::Ready => {
                ready = true;
                println!(
                    "[{:>6.1?}] Ready; semantic token legend: {:?}",
                    t0.elapsed(),
                    client.semantic_token_legend()
                );
            }
            LspEvent::Diagnostics {
                path,
                version,
                diagnostics,
            } => {
                println!(
                    "[{:>6.1?}] {} diagnostics for {} (version {version:?})",
                    t0.elapsed(),
                    diagnostics.len(),
                    path.display()
                );
                for d in &diagnostics {
                    println!(
                        "           {:?} {:?} {:?} {}",
                        d.severity, d.code, d.range.start, d.message
                    );
                }
                found_error = same_file(&path, &program)
                    && diagnostics.iter().any(|d| d.severity == Severity::Error);
            }
            LspEvent::Progress(p) => println!("[{:>6.1?}] progress: {p}", t0.elapsed()),
            LspEvent::Log(l) => println!(
                "[{:>6.1?}] log: {}",
                t0.elapsed(),
                l.chars().take(160).collect::<String>()
            ),
            LspEvent::Exited(reason) => panic!("server exited: {reason}"),
        }
    }
    assert!(ready);
    println!("[{:>6.1?}] got error diagnostics", t0.elapsed());

    let t = Instant::now();
    let completion = recv(
        client.completion(&program, find(PROGRAM, "Console.Wri", 8)),
        "completion",
    );
    println!(
        "[{:>6.1?}] completion: {} items (took {:?})",
        t0.elapsed(),
        completion.len(),
        t.elapsed()
    );
    let write_line = completion.iter().find(|c| c.label == "WriteLine");
    println!("           WriteLine item: {write_line:?}");
    assert!(write_line.is_some(), "no WriteLine in completion");

    let t = Instant::now();
    let locations = recv(
        client.definition(&program, find(PROGRAM, "(greeting)", 1)),
        "definition",
    );
    println!(
        "[{:>6.1?}] definition: {locations:?} (took {:?})",
        t0.elapsed(),
        t.elapsed()
    );
    assert!(!locations.is_empty(), "no definition location");
    assert!(same_file(&locations[0].path, &program));
    assert_eq!(
        locations[0].range.start.line,
        find(PROGRAM, "var greeting", 0).line
    );

    let t = Instant::now();
    let hover = recv(
        client.hover(&program, find(PROGRAM, "(greeting)", 1)),
        "hover",
    );
    println!(
        "[{:>6.1?}] hover: {hover:?} (took {:?})",
        t0.elapsed(),
        t.elapsed()
    );

    let t = Instant::now();
    let tokens = recv(client.semantic_tokens(&program), "semantic tokens");
    println!(
        "[{:>6.1?}] semantic tokens: {} (took {:?})",
        t0.elapsed(),
        tokens.len(),
        t.elapsed()
    );
    for token in tokens.iter().take(12) {
        let line = PROGRAM.lines().nth(token.line as usize).unwrap_or_default();
        let text: String = line
            .chars()
            .skip(token.start as usize)
            .take(token.length as usize)
            .collect();
        println!(
            "           {}:{} {:?} -> {}",
            token.line, token.start, text, token.kind
        );
    }

    let t = Instant::now();
    client.shutdown();
    assert!(
        client.wait_for_exit(Duration::from_secs(5)),
        "server did not exit"
    );
    println!(
        "[{:>6.1?}] shut down (took {:?})",
        t0.elapsed(),
        t.elapsed()
    );
    while let Ok(event) = events.try_recv() {
        if let LspEvent::Exited(reason) = event {
            println!("           exited: {reason}");
        }
    }
    drop(project);
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    norm(a) == norm(b)
}
