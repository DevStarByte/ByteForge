//! Runs the preinstalled ReSharper formatter module through the module host, like ByteForge does. Skips
//! (passes) without `python3`. Without the ReSharper command line tools (`jb`) it checks the "not installed"
//! answer instead.

use std::path::Path;
use std::time::Duration;

use bf_core::plugins::{PluginHandle, PluginMessage, discover};

#[test]
fn resharper_module_formats_over_the_module_protocol() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        println!("SKIPPED: python3 not found");
        return;
    }
    let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
    let manifest = discover(&bundled).into_iter().find(|m| m.id == "byteforge.resharper-format").expect("bundled ReSharper module");
    assert!(manifest.version.is_some());
    let (tx, rx) = flume::unbounded();
    let module = PluginHandle::start(manifest, tx).unwrap();

    let registered = rx
        .recv_timeout(Duration::from_secs(10))
        .ok()
        .into_iter()
        .chain(rx.try_iter())
        .find_map(|m| match m {
            PluginMessage::RegisterFormatter { id, extensions, .. } => Some((id, extensions)),
            _ => None,
        });
    let (id, extensions) = registered.expect("no registerFormatter");
    assert_eq!(id, "resharper");
    assert_eq!(extensions, ["cs"]);

    let dir = std::env::temp_dir().join(format!("bf-resharper-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    module.format(1, &dir.join("A.cs"), "class A{void M(){int x=1;}}");
    // cleanupcode starts a whole ReSharper engine: the first run can take a while.
    let result = loop {
        match rx.recv_timeout(Duration::from_secs(300)).expect("no formatResult") {
            PluginMessage::FormatResult { request: 1, result, .. } => break result,
            _ => continue,
        }
    };
    module.stop();
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Ok(text) => {
            assert!(text.contains("int x = 1;"), "not reformatted: {text:?}");
            assert!(text.lines().count() >= 5, "still on one line: {text:?}");
        }
        Err(e) => {
            assert!(e.contains("aren't installed"), "unexpected error: {e}");
            println!("The ReSharper tools are not installed; checked the install hint only");
        }
    }
}
