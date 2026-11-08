//! Installing, updating and uninstalling module packages from a registry (a local `file://` one here,
//! through curl like the real one), and preinstalling the bundled modules once.

use std::path::Path;

use bf_core::plugins::discover;
use bf_core::registry::{fetch_index, install, is_newer, preinstall, uninstall};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn installs_updates_and_uninstalls_from_a_registry() {
    if std::process::Command::new("curl").arg("--version").output().is_err() {
        println!("SKIPPED: curl not found");
        return;
    }
    let root = std::env::temp_dir().join(format!("bf-registry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let registry = root.join("registry");
    let modules = root.join("modules");
    let index = |version: &str| {
        format!(r#"{{"modules":[{{"id":"me.todos","name":"TODOs","version":"{version}","dir":"todos","files":["module.json","lib/todos.py"],"requires":["python3"]}}]}}"#)
    };
    write(&registry.join("index.json"), &index("1.0.0"));
    write(&registry.join("todos/module.json"), r#"{ "id": "me.todos", "name": "TODOs", "version": "1.0.0", "command": "python3", "args": ["lib/todos.py"] }"#);
    write(&registry.join("todos/lib/todos.py"), "print('v1')\n");
    let base = format!("file://{}", registry.display());

    let packages = fetch_index(&base).unwrap();
    assert_eq!(packages[0].requires, ["python3"]);
    let dir = install(&base, &packages[0], &modules).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("lib/todos.py")).unwrap(), "print('v1')\n");
    let installed = discover(&modules);
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].version.as_deref(), Some("1.0.0"));

    // An update replaces the folder.
    write(&registry.join("index.json"), &index("1.1.0"));
    write(&registry.join("todos/module.json"), r#"{ "id": "me.todos", "name": "TODOs", "version": "1.1.0", "command": "python3", "args": ["lib/todos.py"] }"#);
    write(&registry.join("todos/lib/todos.py"), "print('v2')\n");
    let packages = fetch_index(&base).unwrap();
    assert!(is_newer(&packages[0].version, installed[0].version.as_deref().unwrap()));
    install(&base, &packages[0], &modules).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("lib/todos.py")).unwrap(), "print('v2')\n");
    assert_eq!(discover(&modules)[0].version.as_deref(), Some("1.1.0"));

    // A missing file fails without touching the installed version.
    std::fs::remove_file(registry.join("todos/lib/todos.py")).unwrap();
    assert!(install(&base, &packages[0], &modules).is_err());
    assert_eq!(std::fs::read_to_string(dir.join("lib/todos.py")).unwrap(), "print('v2')\n");
    assert_eq!(std::fs::read_dir(&modules).unwrap().count(), 1, "staging folder left behind");

    uninstall(&modules, &dir).unwrap();
    assert!(discover(&modules).is_empty());
    assert!(uninstall(&modules, &root).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn preinstalls_bundled_modules_once() {
    let root = std::env::temp_dir().join(format!("bf-preinstall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
    let modules = root.join("modules");

    let done = preinstall(&bundled, &modules, &[]);
    assert_eq!(done, ["byteforge.csharpier"]);
    assert!(modules.join("csharpier/csharpier.py").exists());

    // Uninstalled later: not copied again.
    std::fs::remove_dir_all(modules.join("csharpier")).unwrap();
    assert!(preinstall(&bundled, &modules, &done).is_empty());
    assert!(!modules.join("csharpier").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// Checks a registry checkout before publishing it:
/// `BF_REGISTRY=file:///path/to/packages/modules cargo test -p bf-core --test module_registry real_registry`
#[test]
fn real_registry_installs_every_package() {
    let Ok(base) = std::env::var("BF_REGISTRY") else {
        println!("SKIPPED: set BF_REGISTRY to check a registry");
        return;
    };
    let modules = std::env::temp_dir().join(format!("bf-real-registry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&modules);
    let packages = fetch_index(&base).unwrap();
    for p in &packages {
        install(&base, p, &modules).unwrap_or_else(|e| panic!("{}: {e:#}", p.id));
    }
    let installed = discover(&modules);
    let _ = std::fs::remove_dir_all(&modules);
    assert_eq!(installed.len(), packages.len());
    for p in &packages {
        let m = installed.iter().find(|m| m.id == p.id).expect("installed");
        assert_eq!(m.version.as_deref(), Some(p.version.as_str()), "{}: index and module.json versions differ", p.id);
    }
}
