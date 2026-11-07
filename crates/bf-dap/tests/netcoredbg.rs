//! End-to-end tests against a real `netcoredbg`: a console app, and a Razor Pages app with a breakpoint
//! inside a `.cshtml` file. Skip (pass) with a message when `netcoredbg` or `dotnet` is not installed.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use bf_dap::{AdapterConfig, DapClient, DapEvent, EvaluateContext, SourceBreakpoint, netcoredbg_command};
use serde_json::json;

const TIMEOUT: Duration = Duration::from_secs(90);

const CONSOLE_CSPROJ: &str = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>TFM</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
  </PropertyGroup>
</Project>
"#;

const CONSOLE_PROGRAM: &str = r#"var total = 0;
for (var i = 0; i < 3; i++)
{
    total += i;
}
Console.WriteLine($"total={total}");
"#;

const WEB_CSPROJ: &str = r#"<Project Sdk="Microsoft.NET.Sdk.Web">
  <PropertyGroup>
    <TargetFramework>TFM</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
  </PropertyGroup>
</Project>
"#;

const WEB_PROGRAM: &str = r#"var builder = WebApplication.CreateBuilder(args);
builder.Services.AddRazorPages();
var app = builder.Build();
app.MapRazorPages();
app.Run();
"#;

const WEB_PAGE: &str = r#"@page
@{
    var answer = 41 + 1;
}
<p>The answer is @answer</p>
"#;

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

struct TempProject(PathBuf);

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_project(name: &str, files: &[(&str, String)]) -> TempProject {
    let dir = std::env::temp_dir().join(format!("bf-dap-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    TempProject(std::fs::canonicalize(&dir).unwrap())
}

/// Builds the project and returns the assembly to launch.
fn build(project: &Path) -> PathBuf {
    let output = Command::new("dotnet").arg("build").arg(project).arg("-nologo").output().expect("dotnet build");
    assert!(output.status.success(), "dotnet build failed: {}", String::from_utf8_lossy(&output.stdout));
    let program = bf_core::debug::target_assembly(project).expect("target assembly");
    assert!(program.is_file(), "{} does not exist", program.display());
    program
}

fn recv<T>(rx: flume::Receiver<anyhow::Result<T>>, what: &str) -> T {
    rx.recv_timeout(TIMEOUT).unwrap_or_else(|e| panic!("{what}: no response ({e})")).unwrap_or_else(|e| panic!("{what}: {e:#}"))
}

/// Waits for an event matching `pick`, collecting output text on the way.
fn wait_for<T>(events: &flume::Receiver<DapEvent>, output: &mut String, what: &str, mut pick: impl FnMut(&DapEvent, &str) -> Option<T>) -> T {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let event = events.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap_or_else(|_| panic!("timed out waiting for {what}; output:\n{output}"));
        if let DapEvent::Output { text, .. } = &event {
            output.push_str(text);
        }
        if let DapEvent::AdapterExited(reason) = &event {
            panic!("adapter exited while waiting for {what}: {reason}; output:\n{output}");
        }
        if let Some(result) = pick(&event, output) {
            return result;
        }
    }
}

/// Starts netcoredbg, launches `program` with `breakpoints` set in `source`, and waits for the launch.
fn launch(adapter: &str, program: &Path, cwd: &Path, env: serde_json::Value, source: &Path, lines: &[u32]) -> (DapClient, flume::Receiver<DapEvent>, String) {
    let (client, events) = DapClient::start(AdapterConfig { command: adapter.into(), args: vec!["--interpreter=vscode".into()], cwd: cwd.into() }).expect("start netcoredbg");
    let capabilities = recv(client.initialize(), "initialize");
    assert!(capabilities.get("supportsConfigurationDoneRequest").is_some(), "capabilities: {capabilities}");
    let launched = client.launch(json!({ "program": program, "args": [], "cwd": cwd, "env": env, "stopAtEntry": false, "justMyCode": true }));
    let mut output = String::new();
    wait_for(&events, &mut output, "initialized", |e, _| matches!(e, DapEvent::Initialized).then_some(()));
    let breakpoints: Vec<SourceBreakpoint> = lines.iter().map(|&line| SourceBreakpoint { line, condition: None }).collect();
    let bound = recv(client.set_breakpoints(source, &breakpoints), "setBreakpoints");
    assert_eq!(bound.len(), lines.len());
    recv(client.set_exception_breakpoints(&["user-unhandled"]), "setExceptionBreakpoints");
    recv(client.configuration_done(), "configurationDone");
    recv(launched, "launch");
    (client, events, output)
}

#[test]
fn console_app_breakpoints_variables_and_exit() {
    let Some(adapter) = netcoredbg_command(None) else {
        println!("SKIPPED: netcoredbg not found");
        return;
    };
    let Some(tfm) = target_framework() else {
        println!("SKIPPED: dotnet SDK not found");
        return;
    };
    let project = temp_project("console", &[("Demo.csproj", CONSOLE_CSPROJ.replace("TFM", &tfm)), ("Program.cs", CONSOLE_PROGRAM.into())]);
    let program = build(&project.0.join("Demo.csproj"));
    let source = project.0.join("Program.cs");
    let (client, events, mut output) = launch(&adapter, &program, &project.0, json!({}), &source, &[4]);

    let thread = wait_for(&events, &mut output, "breakpoint hit", |e, _| match e {
        DapEvent::Stopped { reason, thread_id, .. } if reason == "breakpoint" => *thread_id,
        _ => None,
    });
    let frames = recv(client.stack_trace(thread, 20), "stackTrace");
    let top = &frames[0];
    assert_eq!(top.line, 4, "stopped at {top:?}");
    assert_eq!(top.path.as_deref().and_then(Path::file_name).and_then(|n| n.to_str()), Some("Program.cs"));
    let scopes = recv(client.scopes(top.id), "scopes");
    let locals = recv(client.variables(scopes[0].variables_reference), "variables");
    let total = locals.iter().find(|v| v.name == "total").unwrap_or_else(|| panic!("no `total` in {locals:?}"));
    assert_eq!(total.value, "0");
    let result = recv(client.evaluate("total + 100", Some(top.id), EvaluateContext::Repl), "evaluate");
    assert_eq!(result.result, "100");

    // Step over the loop body, then run to the end without breakpoints.
    recv(client.next(thread), "next");
    wait_for(&events, &mut output, "step", |e, _| matches!(e, DapEvent::Stopped { reason, .. } if reason == "step").then_some(()));
    recv(client.set_breakpoints(&source, &[]), "clear breakpoints");
    recv(client.continue_(thread), "continue");
    let code = wait_for(&events, &mut output, "exit", |e, _| match e {
        DapEvent::Exited(code) => Some(*code),
        _ => None,
    });
    assert_eq!(code, 0);
    wait_for(&events, &mut output, "terminated", |e, _| matches!(e, DapEvent::Terminated).then_some(()));
    assert!(output.contains("total=3"), "output:\n{output}");
    let _ = client.disconnect(true).recv_timeout(Duration::from_secs(5));
}

#[test]
fn razor_page_breakpoint() {
    let Some(adapter) = netcoredbg_command(None) else {
        println!("SKIPPED: netcoredbg not found");
        return;
    };
    let Some(tfm) = web_framework() else {
        println!("SKIPPED: no ASP.NET Core runtime installed");
        return;
    };
    let project = temp_project(
        "web",
        &[("Web.csproj", WEB_CSPROJ.replace("TFM", &tfm)), ("Program.cs", WEB_PROGRAM.into()), ("Pages/Index.cshtml", WEB_PAGE.into())],
    );
    let program = build(&project.0.join("Web.csproj"));
    let page = project.0.join("Pages").join("Index.cshtml");
    let env = json!({ "ASPNETCORE_URLS": "http://127.0.0.1:0", "ASPNETCORE_ENVIRONMENT": "Development" });
    let (client, events, mut output) = launch(&adapter, &program, &project.0, env, &page, &[3]);

    let url = wait_for(&events, &mut output, "Kestrel URL", |_, output| output.lines().find_map(bf_core::debug::listening_url).map(str::to_owned));
    let address = url.trim_start_matches("http://").trim_end_matches('/').to_owned();
    // The request blocks while the page is paused, so send it from another thread.
    let request = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(&address).expect("connect");
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        write!(stream, "GET / HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        response
    });

    let thread = wait_for(&events, &mut output, "breakpoint in Index.cshtml", |e, _| match e {
        DapEvent::Stopped { reason, thread_id, .. } if reason == "breakpoint" => *thread_id,
        _ => None,
    });
    let frames = recv(client.stack_trace(thread, 20), "stackTrace");
    let top = &frames[0];
    assert_eq!(top.path.as_deref().and_then(Path::file_name).and_then(|n| n.to_str()), Some("Index.cshtml"), "stopped at {top:?}");
    assert_eq!(top.line, 3, "stopped at {top:?}");

    recv(client.set_breakpoints(&page, &[]), "clear breakpoints");
    recv(client.continue_(thread), "continue");
    let response = request.join().unwrap();
    assert!(response.contains("The answer is 42"), "response:\n{response}");

    // Stopping (Shift+F5) while the app runs must end it, so the port is free for the next start.
    let address = url.trim_start_matches("http://").trim_end_matches('/').to_owned();
    recv(client.disconnect(true), "disconnect");
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(&address).is_ok() {
        assert!(Instant::now() < deadline, "the web app still listens on {address} after disconnect");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn killing_the_adapter_ends_the_launched_program() {
    let Some(adapter) = netcoredbg_command(None) else {
        println!("SKIPPED: netcoredbg not found");
        return;
    };
    let Some(tfm) = target_framework() else {
        println!("SKIPPED: dotnet SDK not found");
        return;
    };
    let program = "Console.WriteLine($\"pid={Environment.ProcessId}\");\nThread.Sleep(120_000);\n";
    let project = temp_project("kill", &[("Demo.csproj", CONSOLE_CSPROJ.replace("TFM", &tfm)), ("Program.cs", program.into())]);
    let assembly = build(&project.0.join("Demo.csproj"));
    let (client, events, mut output) = launch(&adapter, &assembly, &project.0, json!({}), &project.0.join("Program.cs"), &[]);
    let pid: u32 = wait_for(&events, &mut output, "pid", |_, output| {
        output.lines().find_map(|l| l.trim().strip_prefix("pid=")).and_then(|p| p.trim().parse().ok())
    });
    assert!(Path::new(&format!("/proc/{pid}")).exists());
    // What happens when the IDE quits or the session is dropped without a clean disconnect.
    client.kill();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Path::new(&format!("/proc/{pid}")).exists() {
        assert!(Instant::now() < deadline, "the debugged program {pid} survived the adapter");
        std::thread::sleep(Duration::from_millis(50));
    }
}
