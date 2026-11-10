//! End-to-end test against a real `csharp-ls`. Skips (passes) with a message when
//! `csharp-ls` or `dotnet` is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use bf_lsp::{LspClient, LspEvent, Position, ServerConfig, csharp_ls_command};

const PROGRAM: &str = r#"using System;

namespace Demo;

public static class Program
{
    public static void Main()
    {
        int x = "s";
        var greeting = "hello";
        var list = new List<int>();
        Console.WriteLine(greeting);
        Console.Wri
        var sum = Add(1, 2);
    }

    static int Add(int left, int right) => left + right;
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
    let mut last_diagnostics = Vec::new();
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
                    && diagnostics.iter().any(|d| d.code.as_deref() == Some("CS0246"));
                if same_file(&path, &program) {
                    last_diagnostics = diagnostics;
                }
            }
            LspEvent::Progress(p) => println!("[{:>6.1?}] progress: {p}", t0.elapsed()),
            LspEvent::Log(l) => println!(
                "[{:>6.1?}] log: {}",
                t0.elapsed(),
                l.chars().take(160).collect::<String>()
            ),
            LspEvent::Exited(reason) => panic!("server exited: {reason}"),
            LspEvent::ApplyEdit(_) => {}
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
    let references = recv(client.references(&program, find(PROGRAM, "var greeting", 4)), "references");
    println!("[{:>6.1?}] references: {references:?} (took {:?})", t0.elapsed(), t.elapsed());
    assert_eq!(references.len(), 2, "declaration + one use expected: {references:?}");

    let t = Instant::now();
    let rename = recv(client.rename(&program, find(PROGRAM, "(greeting)", 1), "message"), "rename");
    println!("[{:>6.1?}] rename: {rename:?} (took {:?})", t0.elapsed(), t.elapsed());
    let renamed: Vec<_> = rename.changes.iter().flat_map(|(_, edits)| edits.iter()).collect();
    assert_eq!(renamed.len(), 2, "{rename:?}");
    assert!(renamed.iter().all(|e| e.new_text == "message"));

    let t = Instant::now();
    let help = recv(client.signature_help(&program, find(PROGRAM, "WriteLine(greeting)", 10)), "signature help");
    println!("[{:>6.1?}] signature help: {help:?} (took {:?})", t0.elapsed(), t.elapsed());
    let help = help.expect("no signature help inside Console.WriteLine(…)");
    assert!(help.signatures.iter().any(|s| s.label.contains("WriteLine")));

    let t = Instant::now();
    let highlights = recv(client.document_highlight(&program, find(PROGRAM, "(greeting)", 1)), "document highlight");
    println!("[{:>6.1?}] document highlight: {highlights:?} (took {:?})", t0.elapsed(), t.elapsed());
    assert_eq!(highlights.len(), 2, "declaration + use of greeting: {highlights:?}");

    let t = Instant::now();
    let whole = bf_lsp::Range { start: Position { line: 0, character: 0 }, end: Position { line: PROGRAM.lines().count() as u32, character: 0 } };
    let hints = recv(client.inlay_hints(&program, whole), "inlay hints");
    println!("[{:>6.1?}] inlay hints: {hints:?} (took {:?})", t0.elapsed(), t.elapsed());
    assert!(hints.iter().any(|h| h.parameter && h.label.starts_with("left")), "no parameter-name hint for Add(1, 2): {hints:?}");

    let t = Instant::now();
    let symbols = recv(client.document_symbols(&program), "document symbols");
    println!("[{:>6.1?}] document symbols: {:?} (took {:?})", t0.elapsed(), symbols.iter().map(|s| (&s.name, s.kind, s.depth)).collect::<Vec<_>>(), t.elapsed());
    assert!(symbols.iter().any(|s| s.name.starts_with("Program") && s.kind == "class"), "{symbols:?}");
    assert!(symbols.iter().any(|s| s.name.starts_with("Main") && s.kind == "method"), "{symbols:?}");

    let t = Instant::now();
    let found = recv(client.workspace_symbols("Program"), "workspace symbols");
    println!("[{:>6.1?}] workspace symbols: {found:?} (took {:?})", t0.elapsed(), t.elapsed());
    assert!(found.iter().any(|s| s.name.starts_with("Program")), "{found:?}");

    let t = Instant::now();
    let types = recv(client.type_definition(&program, find(PROGRAM, "(greeting)", 1)), "type definition");
    println!("[{:>6.1?}] type definition of greeting: {types:?} (took {:?})", t0.elapsed(), t.elapsed());

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

    // Quick fix for the missing `using` (CS0246 on `List<int>`).
    let t = Instant::now();
    let missing: Vec<_> = last_diagnostics.iter().filter(|d| d.code.as_deref() == Some("CS0246")).cloned().collect();
    let actions = recv(client.code_actions(&program, missing[0].range, &missing), "code actions");
    println!(
        "[{:>6.1?}] code actions: {:?} (took {:?})",
        t0.elapsed(),
        actions.iter().map(|a| (&a.title, &a.kind, a.edit.is_some())).collect::<Vec<_>>(),
        t.elapsed()
    );
    let fix = actions
        .iter()
        .find(|a| a.title.contains("System.Collections.Generic"))
        .unwrap_or_else(|| panic!("no `using System.Collections.Generic` fix in {:?}", actions.iter().map(|a| &a.title).collect::<Vec<_>>()))
        .clone();
    let fix = if fix.edit.is_some() { fix } else { recv(client.resolve_code_action(&fix), "resolve code action") };
    let edit = fix.edit.expect("the fix has no edit");
    let inserted: String = edit.changes.iter().flat_map(|(_, edits)| edits.iter().map(|e| e.new_text.clone())).collect();
    println!("           edit: {edit:?}");
    assert!(inserted.contains("System.Collections.Generic") || inserted.contains("List"), "unexpected edit {edit:?}");

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
