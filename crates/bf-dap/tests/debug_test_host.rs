//! "Debug Test" end to end: `dotnet test` with VSTEST_HOST_DEBUG=1 waits for a debugger, netcoredbg
//! attaches to the printed process id (as the workbench does), a breakpoint in the test is hit, and the
//! run finishes. Skips (passes) when netcoredbg, dotnet or the xunit template is missing.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use bf_core::testing::{filter, host_debug_pid};
use bf_dap::{AdapterConfig, DapClient, DapEvent, SourceBreakpoint, netcoredbg_command};
use serde_json::json;

const TIMEOUT: Duration = Duration::from_secs(120);

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn debugging_a_test_stops_at_its_breakpoint() {
    let Some(adapter) = netcoredbg_command(None) else {
        println!("SKIPPED: netcoredbg not found");
        return;
    };
    let root = TempDir(std::env::temp_dir().join(format!("bf-debug-test-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&root.0);
    std::fs::create_dir_all(&root.0).unwrap();
    let created = Command::new("dotnet").args(["new", "xunit", "-o", "Dbg.Tests", "-n", "Dbg.Tests"]).current_dir(&root.0).output();
    let project_dir = root.0.join("Dbg.Tests");
    if created.is_err() || !project_dir.join("Dbg.Tests.csproj").exists() {
        println!("SKIPPED: dotnet or the xunit template is not available");
        return;
    }
    let source = project_dir.join("UnitTest1.cs");
    // Line 8 (1-based) is the breakpoint.
    std::fs::write(&source, "namespace Dbg.Tests;\n\npublic class T\n{\n    [Fact]\n    public void Adds()\n    {\n        var sum = 2 + 2;\n        Assert.Equal(4, sum);\n    }\n}\n").unwrap();
    let build = Command::new("dotnet").args(["build", "-v", "q"]).current_dir(&project_dir).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stdout));

    let mut host = Command::new("dotnet")
        .args(["test", "--no-build", "--filter", &filter("Dbg.Tests.T.Adds", false)])
        .current_dir(&project_dir)
        .env("VSTEST_HOST_DEBUG", "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = std::io::BufReader::new(host.stdout.take().unwrap()).lines().map_while(Result::ok);
    let pid = lines.by_ref().find_map(|l| host_debug_pid(&l)).expect("process id line");

    let (client, events) = DapClient::start(AdapterConfig { command: adapter, args: vec!["--interpreter=vscode".into()], cwd: project_dir.clone() }).unwrap();
    let recv = |rx: flume::Receiver<anyhow::Result<serde_json::Value>>, what: &str| rx.recv_timeout(TIMEOUT).unwrap_or_else(|e| panic!("{what}: {e}")).unwrap();
    recv(client.initialize(), "initialize");
    let attached = client.attach(json!({ "name": ".NET Attach", "type": "coreclr", "request": "attach", "processId": pid, "justMyCode": true }));
    let deadline = Instant::now() + TIMEOUT;
    let mut next = |what: &str| events.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    while !matches!(next("initialized"), DapEvent::Initialized) {}
    client.set_breakpoints(&source, &[SourceBreakpoint { line: 8, condition: None }]).recv_timeout(TIMEOUT).unwrap().unwrap();
    recv(client.configuration_done(), "configurationDone");
    recv(attached, "attach");

    let thread = loop {
        match next("breakpoint") {
            DapEvent::Stopped { reason, thread_id, .. } if reason == "breakpoint" => break thread_id.expect("thread"),
            DapEvent::AdapterExited(r) => panic!("adapter exited: {r}"),
            _ => {}
        }
    };
    let frames = client.stack_trace(thread, 1).recv_timeout(TIMEOUT).unwrap().unwrap();
    assert_eq!(frames[0].line, 8, "{frames:?}");
    recv(client.continue_(thread), "continue");

    let rest: Vec<String> = lines.collect();
    let _ = host.wait();
    client.kill();
    assert!(rest.iter().any(|l| l.contains("Passed!")), "{}", rest.join("\n"));
}
