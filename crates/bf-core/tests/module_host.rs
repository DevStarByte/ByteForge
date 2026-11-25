//! The .NET module host with a real module (modulehost/tests/EchoModule): load, messages both ways, unload
//! (after which the module's folder can be deleted, also on Windows), load again, and a module that fails.
//! Builds both with the .NET SDK; skips (passes) with a message when there is no .NET 10 SDK or newer.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use bf_core::plugins::{ModuleManifest, PluginHandle, PluginMessage};

fn has_dotnet_10() -> bool {
    Command::new("dotnet")
        .arg("--list-sdks")
        .output()
        .ok()
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.split('.').next()?.trim().parse::<u32>().ok()).any(|m| m >= 10))
}

fn build(project: &Path) {
    let out = Command::new("dotnet").args(["build", "-c", "Release", "--disable-build-servers", "-nologo"]).arg(project).output().unwrap();
    assert!(out.status.success(), "dotnet build {}:\n{}{}", project.display(), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn next(rx: &flume::Receiver<PluginMessage>, what: &str, mut matches: impl FnMut(&PluginMessage) -> bool) -> PluginMessage {
    loop {
        let m = rx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| panic!("no {what} from the module"));
        if matches(&m) {
            return m;
        }
    }
}

#[test]
fn dotnet_modules_load_talk_unload_and_fail_alone() {
    if !has_dotnet_10() {
        eprintln!("skipped: no .NET 10 SDK");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modulehost");
    build(&root.join("ByteForge.ModuleHost.csproj"));
    build(&root.join("tests/EchoModule/EchoModule.csproj"));
    bf_core::modulehost::set_host_dll(root.join("bin/ByteForge.ModuleHost.dll"));

    // An installed module: module.json and its build in bin/.
    let dir: PathBuf = std::env::temp_dir().join(format!("bf-module-host-{}", std::process::id())).join("echo");
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&root.join("tests/EchoModule/bin"), &dir.join("bin"));
    let text = r#"{ id: "test.echo", name: "Echo", runtime: "dotnet", assembly: "bin/EchoModule.dll" }"#;
    std::fs::write(dir.join("module.json"), text).unwrap();
    let mut manifest: ModuleManifest = json5::from_str(text).unwrap();
    manifest.dir = dir.clone();
    assert!(manifest.is_dotnet() && manifest.is_runnable());

    let (tx, rx) = flume::unbounded();
    let handle = PluginHandle::start(manifest.clone(), tx.clone()).unwrap();
    next(&rx, "registerCommand", |m| matches!(m, PluginMessage::RegisterCommand { module, id, .. } if module == "test.echo" && id == "echo.hello"));
    handle.activate(None, &serde_json::json!({}));
    next(&rx, "activate's log", |m| matches!(m, PluginMessage::Log { text, .. } if text == "activated in echo"));
    handle.execute("echo.hello");
    next(&rx, "the command's log", |m| matches!(m, PluginMessage::Log { text, .. } if text == "ran echo.hello"));

    // Unloaded: the host holds none of its files, so an uninstall can delete them right away.
    handle.stop();
    std::fs::remove_dir_all(dir.join("bin")).expect("the module's files aren't held open");
    copy_dir(&root.join("tests/EchoModule/bin"), &dir.join("bin"));

    // Loaded again (an update), in the same host; then it fails: only it ends, and says why.
    let handle = PluginHandle::start(manifest, tx).unwrap();
    next(&rx, "registerCommand again", |m| matches!(m, PluginMessage::RegisterCommand { .. }));
    handle.execute("echo.crash");
    let exited = next(&rx, "exited", |m| matches!(m, PluginMessage::Exited { .. }));
    let PluginMessage::Exited { module, reason } = exited else { unreachable!() };
    assert_eq!(module, "test.echo");
    assert!(reason.contains("crashed on purpose"), "{reason}");

    // A missing assembly is reported, not a hang.
    let (tx2, rx2) = flume::unbounded();
    let mut broken: ModuleManifest = json5::from_str(r#"{ id: "test.broken", name: "Broken", runtime: "dotnet", assembly: "bin/Nope.dll" }"#).unwrap();
    broken.dir = dir.clone();
    let _broken = PluginHandle::start(broken, tx2).unwrap();
    let exited = next(&rx2, "exited for a missing assembly", |m| matches!(m, PluginMessage::Exited { .. }));
    assert!(matches!(exited, PluginMessage::Exited { reason, .. } if reason.contains("missing")));

    bf_core::modulehost::shutdown();
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}
