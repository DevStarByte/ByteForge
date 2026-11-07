//! Runs the built-in CSharpier module through the module host, like ByteForge does. Skips (passes)
//! without `python3`. Without CSharpier installed it checks the "not installed" answer instead.

use std::path::Path;
use std::time::Duration;

use bf_core::plugins::{PluginHandle, PluginMessage, discover};

#[test]
fn csharpier_module_formats_over_the_module_protocol() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        println!("SKIPPED: python3 not found");
        return;
    }
    let builtin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
    let manifest = discover(&builtin, Path::new("/nonexistent")).into_iter().find(|m| m.id == "byteforge.csharpier").expect("built-in CSharpier module");
    assert!(manifest.builtin);
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
    assert_eq!(id, "csharpier");
    assert!(extensions.contains(&"cs".to_owned()) && extensions.contains(&"csproj".to_owned()));

    let dir = std::env::temp_dir().join(format!("bf-csharpier-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    module.format(1, &dir.join("A.cs"), "class A{void M(){int x=1;}}");
    module.format(2, &dir.join("B.razor"), "<p>x</p>");
    let mut results = std::collections::HashMap::new();
    while results.len() < 2 {
        match rx.recv_timeout(Duration::from_secs(60)).expect("no formatResult") {
            PluginMessage::FormatResult { request, result, .. } => {
                results.insert(request, result);
            }
            _ => {}
        }
    }
    module.stop();
    let _ = std::fs::remove_dir_all(&dir);
    match &results[&1] {
        Ok(text) => assert_eq!(text, "class A\n{\n    void M()\n    {\n        int x = 1;\n    }\n}\n"),
        Err(e) => {
            assert!(e.contains("isn't installed"), "unexpected error: {e}");
            println!("CSharpier is not installed; checked the install hint only");
        }
    }
    assert!(results[&2].is_err(), "a .razor file must not come back formatted (CSharpier returns nothing for it)");
}
