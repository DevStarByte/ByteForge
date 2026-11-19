//! Debugging a Blazor WebAssembly app through ByteForge's adapter (`byteforge --blazor-wasm-adapter`),
//! driven over DAP like the IDE drives it: a breakpoint in a Razor component's `@code` is hit, the stack,
//! locals and an expression answer, and the session ends. Creates the app from the `blazorwasm`
//! template (needs the .NET SDK and NuGet) and runs a headless Edge, Chrome or Chromium; skips without
//! a browser. Runs only on request (`--ignored`): about a minute.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use bf_dap::{AdapterConfig, DapClient, DapEvent, EvaluateContext, SourceBreakpoint};
use serde_json::json;

const CODE: &str = r#"
@code {
    private int length;

    protected override void OnInitialized()
    {
        var greeting = "hello";
        length = greeting.Length;
    }
}
"#;

fn recv<T>(rx: flume::Receiver<anyhow::Result<T>>, what: &str) -> T {
    rx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|e| panic!("{what}: no response ({e})")).unwrap_or_else(|e| panic!("{what}: {e:#}"))
}

fn long_path(path: &Path) -> PathBuf {
    PathBuf::from(std::fs::canonicalize(path).unwrap().to_string_lossy().trim_start_matches(r"\\?\"))
}

fn dotnet(dir: &Path, args: &[&str]) {
    let out = Command::new("dotnet").args(args).current_dir(dir).env("DOTNET_NOLOGO", "1").output().expect("dotnet");
    assert!(out.status.success(), "dotnet {args:?}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

#[test]
#[ignore]
fn breakpoint_in_a_blazor_webassembly_component() {
    let Some(browser) = bf_dap::wasm::launch::find_browser(std::env::var("BF_BROWSER").ok().as_deref()) else {
        println!("SKIPPED: no Edge, Chrome or Chromium (set BF_BROWSER)");
        return;
    };
    let dir = std::env::temp_dir().join(format!("bf-wasm-debug-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = long_path(&dir);
    dotnet(&dir, &["new", "blazorwasm", "-o", "WasmDemo", "--no-https"]);
    let app = dir.join("WasmDemo");
    let home = app.join("Pages").join("Home.razor");
    let mut text = std::fs::read_to_string(&home).unwrap();
    text.push_str(CODE);
    std::fs::write(&home, &text).unwrap();
    let line = text.lines().position(|l| l.contains("length = greeting.Length")).unwrap() as u32 + 1;
    dotnet(&app, &["build", "-v", "q"]);

    let t0 = Instant::now();
    let config = AdapterConfig { command: env!("CARGO_BIN_EXE_byteforge").into(), args: vec!["--blazor-wasm-adapter".into()], cwd: app.clone() };
    let (client, events) = DapClient::start(config).expect("start the adapter");
    recv(client.initialize(), "initialize");
    let project = app.join("WasmDemo.csproj");
    let launch = client.launch(json!({ "project": project, "headless": true, "browser": browser }));
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut output = String::new();
    let next = |what: &str, output: &mut String| loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(DapEvent::Output { text, .. }) => output.push_str(&text),
            Ok(DapEvent::AdapterExited(reason)) => panic!("{what}: the adapter ended ({reason})\n{output}"),
            Ok(DapEvent::Terminated) => panic!("{what}: the session ended\n{output}"),
            Ok(event) => return event,
            Err(_) => panic!("{what}: nothing within the time\n{output}"),
        }
    };
    // initialized → breakpoints → configurationDone (the page opens) → the breakpoint is hit. A failed
    // launch answers with its reason instead of `initialized`.
    loop {
        if let Ok(result) = launch.try_recv()
            && let Err(e) = result
        {
            panic!("launch: {e:#}\n{output}");
        }
        match events.recv_timeout(Duration::from_millis(250)) {
            Ok(DapEvent::Initialized) => break,
            Ok(DapEvent::Output { text, .. }) => output.push_str(&text),
            Ok(DapEvent::AdapterExited(reason)) => panic!("initialized: the adapter ended ({reason})\n{output}"),
            Ok(_) => {}
            Err(_) if Instant::now() > deadline => panic!("initialized: nothing within the time\n{output}"),
            Err(_) => {}
        }
    }
    recv(launch, "launch");
    let set = recv(client.set_breakpoints(&home, &[SourceBreakpoint { line, condition: None }]), "setBreakpoints");
    println!("[{:>5.1?}] breakpoint: {set:?}", t0.elapsed());
    recv(client.configuration_done(), "configurationDone");
    loop {
        match next("the breakpoint", &mut output) {
            DapEvent::Stopped { reason, .. } => {
                println!("[{:>5.1?}] stopped: {reason}", t0.elapsed());
                break;
            }
            other => println!("[{:>5.1?}] {other:?}", t0.elapsed()),
        }
    }
    let frames = recv(client.stack_trace(1, 20), "stackTrace");
    let top = frames.first().expect("a frame");
    println!("           top frame: {} {:?}:{}", top.name, top.path, top.line);
    assert_eq!(top.line, line, "{frames:?}");
    assert!(top.path.as_deref().is_some_and(|p| p.ends_with("Home.razor")), "{frames:?}");
    let scopes = recv(client.scopes(top.id), "scopes");
    let locals = recv(client.variables(scopes[0].variables_reference), "variables");
    println!("           locals: {:?}", locals.iter().map(|v| (v.name.clone(), v.value.clone())).collect::<Vec<_>>());
    assert!(locals.iter().any(|v| v.name == "greeting" && v.value.contains("hello")), "{locals:?}");
    let length = recv(client.evaluate("greeting.Length", Some(top.id), EvaluateContext::Watch), "evaluate");
    assert_eq!(length.result, "5");
    recv(client.continue_(1), "continue");
    let _ = client.disconnect(true).recv_timeout(Duration::from_secs(20));
    let _ = std::fs::remove_dir_all(&dir);
}
