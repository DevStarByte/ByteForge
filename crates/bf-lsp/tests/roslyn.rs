//! End-to-end test against Microsoft's Roslyn language server, through ByteForge's own client: the
//! project opens with `project/open`, diagnostics are pulled, and the features the editor uses answer.
//! Needs the server: set BF_ROSLYN_DLL to its `Microsoft.CodeAnalysis.LanguageServer.dll` (skips
//! otherwise). The test project needs the .NET 10 SDK or newer.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use bf_lsp::{LspClient, LspEvent, Position, Range, roslyn_config};

const PROGRAM: &str = r#"namespace Demo;

public static class Program
{
    public static void Main()
    {
        int x = "s";
        var greeting = "hello";
        Console.WriteLine(greeting);
        var sum = Add(1, 2);
    }

    static int Add(int left, int right) => left + right;
}
"#;

const CSPROJ: &str = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings><Nullable>enable</Nullable></PropertyGroup>
</Project>
"#;

fn find(needle: &str, offset: u32) -> Position {
    let (line, text) = PROGRAM.lines().enumerate().find(|(_, l)| l.contains(needle)).expect("needle");
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

#[test]
fn roslyn_end_to_end() {
    let Ok(dll) = std::env::var("BF_ROSLYN_DLL") else {
        println!("SKIPPED: set BF_ROSLYN_DLL to Microsoft.CodeAnalysis.LanguageServer.dll");
        return;
    };
    let dir = std::env::temp_dir().join(format!("bf-roslyn-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    let guard = TempDir(dir.clone());
    let project = dir.join("Demo.csproj");
    let program = dir.join("Program.cs");
    std::fs::write(&project, CSPROJ).unwrap();
    std::fs::write(&program, PROGRAM).unwrap();
    assert!(Command::new("dotnet").args(["restore", "-v", "q"]).current_dir(&dir).status().unwrap().success());

    let t0 = Instant::now();
    let config = roslyn_config(PathBuf::from(&dll).as_path(), dir.clone(), &dir.join("logs"), None, std::slice::from_ref(&project));
    let (client, events) = LspClient::start(config).expect("start the Roslyn server");
    client.did_open(&program, 1, PROGRAM);

    // Diagnostics arrive by pulling (the server never publishes them).
    let deadline = t0 + Duration::from_secs(120);
    let diagnostics = loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("diagnostics in time") {
            LspEvent::Diagnostics { diagnostics, .. } if diagnostics.iter().any(|d| d.code.as_deref() == Some("CS0029")) => break diagnostics,
            LspEvent::Exited(reason) => panic!("server exited: {reason}"),
            _ => {}
        }
    };
    println!("[{:>5.1?}] diagnostics: {:?}", t0.elapsed(), diagnostics.iter().map(|d| (d.code.clone(), d.severity)).collect::<Vec<_>>());

    let completion = recv(client.completion(&program, find("WriteLine", 3)), "completion");
    assert!(completion.iter().any(|c| c.label == "WriteLine"), "{} items", completion.len());
    let hover = recv(client.hover(&program, find("greeting)", 2)), "hover").unwrap_or_default();
    assert!(hover.contains("greeting"), "{hover}");
    let definition = recv(client.definition(&program, find("Add(1", 1)), "definition");
    assert_eq!(definition.first().map(|l| l.range.start.line), Some(find("static int Add", 0).line));
    let references = recv(client.references(&program, find("greeting =", 1)), "references");
    assert_eq!(references.len(), 2, "{references:?}");
    let rename = recv(client.rename(&program, find("greeting =", 1), "hi"), "rename");
    assert_eq!(rename.changes.iter().map(|(_, e)| e.len()).sum::<usize>(), 2, "{rename:?}");
    let symbols = recv(client.document_symbols(&program), "document symbols");
    let main = symbols.iter().find(|s| s.name.starts_with("Main")).expect("Main");
    assert!(main.full_range.end.line > main.full_range.start.line, "breadcrumbs need the body range: {main:?}");
    let tokens = recv(client.semantic_tokens(&program), "semantic tokens");
    assert!(tokens.len() > 10, "{} tokens", tokens.len());
    let end = Position { line: PROGRAM.lines().count() as u32 - 1, character: 0 };
    let hints = recv(client.inlay_hints(&program, Range { start: Position { line: 0, character: 0 }, end }), "inlay hints");
    println!("           inlay hints: {:?}", hints.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    assert!(hints.iter().any(|h| h.label.contains("left")), "parameter names from the configuration: {hints:?}");

    // Refactorings come with their kinds; Extract method resolves to an edit.
    let start = find("Console.WriteLine", 0);
    let statement = Range { start, end: Position { line: start.line, character: start.character + 28 } };
    let actions = recv(client.code_actions(&program, statement, &[]), "code actions");
    println!("           on a statement: {:?}", actions.iter().map(|a| (a.title.clone(), a.kind.clone(), a.group())).collect::<Vec<_>>());
    let extract = actions.iter().find(|a| a.title == "Extract method").expect("Extract method");
    assert_eq!(extract.group(), "Extract");
    let extract = if extract.edit.is_some() { extract.clone() } else { recv(client.resolve_code_action(extract), "resolve") };
    let inserted: String = extract.edit.expect("an edit").changes.iter().flat_map(|(_, e)| e.iter().map(|e| e.new_text.clone())).collect();
    assert!(inserted.contains("NewMethod"), "{inserted}");
    let caret = find("int left", 0);
    let on_parameters = recv(client.code_actions(&program, Range { start: caret, end: caret }, &[]), "code actions on parameters");
    println!("           on parameters: {:?}", on_parameters.iter().map(|a| a.title.clone()).collect::<Vec<_>>());

    let edits = recv(client.formatting(&program, 4, true), "formatting");
    println!("           formatting edits: {}", edits.len());
    client.shutdown();
    let _ = client.wait_for_exit(Duration::from_secs(10));
    drop(guard);
}
