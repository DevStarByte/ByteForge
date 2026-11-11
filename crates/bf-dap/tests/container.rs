//! Debugs an app inside a Docker container, the way ByteForge does: netcoredbg copied into the container
//! and started with `docker exec -i`, a breakpoint sent as a short workspace-relative path, and the
//! stopped frame's container path mapped back to the workspace file.
//!
//! Needs a running container whose app loops through a line (skips otherwise): set BF_TEST_CONTAINER to
//! its name, BF_TEST_CONTAINER_SOURCE to that source file in the workspace and BF_TEST_CONTAINER_LINE to
//! the (1-based) line.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bf_core::container;
use bf_dap::{AdapterConfig, DapClient, DapEvent, SourceBreakpoint};
use serde_json::json;

#[test]
fn breakpoint_in_a_container() {
    let (Ok(name), Ok(source), Ok(line)) = (std::env::var("BF_TEST_CONTAINER"), std::env::var("BF_TEST_CONTAINER_SOURCE"), std::env::var("BF_TEST_CONTAINER_LINE")) else {
        println!("SKIPPED: set BF_TEST_CONTAINER, BF_TEST_CONTAINER_SOURCE and BF_TEST_CONTAINER_LINE");
        return;
    };
    let source = PathBuf::from(source);
    let line: u32 = line.parse().expect("line number");
    let root = source.parent().and_then(Path::parent).expect("workspace root").to_path_buf();
    let files = vec![source.clone(), root.join("Other").join("Program.cs")];

    let pid = container::app_candidates(&container::processes(&name)).first().expect("an app process").pid;
    if !container::debugger_ready(&name) {
        let arch = container::architecture(&name).expect("architecture");
        let dir = container::host_debugger_dir("/opt/netcoredbg/netcoredbg", &arch)
            .unwrap_or_else(|| container::download_debugger(&arch, &std::env::temp_dir().join("bf-netcoredbg-cache")).expect("download"));
        container::install_debugger(&name, &dir).expect("install netcoredbg");
    }

    let (client, events) = DapClient::start(AdapterConfig { command: "docker".into(), args: container::adapter_args(&name), cwd: root.clone() }).expect("start adapter");
    client.initialize().recv_timeout(Duration::from_secs(30)).expect("initialize").expect("initialize");
    let attach = client.attach(json!({ "name": ".NET Attach", "type": "coreclr", "request": "attach", "processId": pid, "justMyCode": true }));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match events.recv_deadline(deadline).expect("initialized event") {
            DapEvent::Initialized => break,
            _ => continue,
        }
    }
    let sent = container::breakpoint_path(&source, &root, &files);
    assert!(!sent.starts_with('/'), "{sent}");
    client.set_breakpoints(Path::new(&sent), &[SourceBreakpoint { line, condition: None }]).recv_timeout(Duration::from_secs(30)).expect("setBreakpoints").expect("setBreakpoints");
    client.configuration_done().recv_timeout(Duration::from_secs(30)).expect("configurationDone").expect("configurationDone");
    let _ = attach.recv_timeout(Duration::from_secs(30));

    let deadline = Instant::now() + Duration::from_secs(30);
    let thread = loop {
        match events.recv_deadline(deadline).expect("stopped at the breakpoint") {
            DapEvent::Stopped { reason, thread_id, .. } if reason == "breakpoint" => break thread_id.expect("thread"),
            _ => continue,
        }
    };
    let frames = client.stack_trace(thread, 5).recv_timeout(Duration::from_secs(30)).expect("stackTrace").expect("stackTrace");
    let top = frames.first().expect("a frame");
    let container_path = top.path.as_ref().expect("frame source").to_string_lossy().into_owned();
    println!("sent {sent}; stopped at {container_path}:{}", top.line);
    assert_eq!(top.line, line);
    assert_eq!(container::host_path(&container_path, &files), Some(source.clone()));
    let _ = client.disconnect(false).recv_timeout(Duration::from_secs(10));
}
