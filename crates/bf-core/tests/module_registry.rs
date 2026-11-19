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

/// `modules/` ships with every download, so it holds the preinstalled modules and nothing else; every
/// other module lives in the registry (`modules/` on the packages branch).
#[test]
fn only_preinstalled_modules_are_bundled() {
    let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
    let mut ids: Vec<String> = discover(&bundled).into_iter().map(|m| m.id).collect();
    ids.sort();
    let mut expected: Vec<String> = bf_core::registry::PREINSTALLED.iter().map(|s| (*s).to_owned()).collect();
    expected.sort();
    assert_eq!(ids, expected, "modules/ should hold only the preinstalled modules");
}

#[test]
fn preinstalls_bundled_modules_once() {
    let root = std::env::temp_dir().join(format!("bf-preinstall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
    let modules = root.join("modules");

    let done = preinstall(&bundled, &modules, &[]);
    assert_eq!(done, ["byteforge.resharper-format"], "only the ReSharper formatter is preinstalled");
    assert!(modules.join("resharper-format/resharper_format.py").exists());

    // Uninstalled later: not copied again.
    std::fs::remove_dir_all(modules.join("resharper-format")).unwrap();
    assert!(preinstall(&bundled, &modules, &done).is_empty());
    assert!(!modules.join("resharper-format").exists());

    // Someone who had CSharpier preinstalled by an older version gets the ReSharper formatter added once.
    let older = root.join("older");
    assert_eq!(preinstall(&bundled, &older, &["byteforge.csharpier".to_owned()]), ["byteforge.resharper-format"]);
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

/// Every module in the repository's `modules/` folder and in the module test data parses (a broken
/// `module.json` is skipped silently at runtime), and the settings they declare are well formed.
#[test]
fn bundled_modules_parse_with_their_settings() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut modules = Vec::new();
    for dir in [root.join("modules"), root.join("tests/fixtures/modules")] {
        let folders = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().join("module.json").is_file()).count();
        let found = discover(&dir);
        assert_eq!(found.len(), folders, "a module.json in {} didn't parse", dir.display());
        modules.extend(found);
    }
    let sql = modules.iter().find(|m| m.id == "byteforge.sql").expect("the SQL module");
    assert!(sql.settings.iter().any(|s| s.key == "sql.format.keywordCase" && s.choices.contains(&"upper".to_owned())));
    for m in &modules {
        for s in &m.settings {
            assert!(s.key.contains('.') && !s.title.is_empty(), "{}: setting {:?}", m.id, s.key);
            assert!(s.choices.is_empty() || s.default.as_str().is_some_and(|d| s.choices.iter().any(|c| c == d)), "{}: {} default isn't a choice", m.id, s.key);
        }
    }
}

/// Updating a module while a program runs in its folder (the module itself, or a helper it started): on
/// Windows the folder can't be moved then. The update fails and the installed version stays whole (it used
/// to be half deleted); once the program is gone, the update goes through.
#[test]
fn an_update_while_the_module_runs_never_breaks_it() {
    if std::process::Command::new("curl").arg("--version").output().is_err() {
        println!("SKIPPED: curl not found");
        return;
    }
    let root = std::env::temp_dir().join(format!("bf-registry-busy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (registry, modules) = (root.join("registry"), root.join("modules"));
    let publish = |version: &str| {
        write(&registry.join("index.json"), &format!(r#"{{"modules":[{{"id":"me.busy","name":"Busy","version":"{version}","dir":"busy","files":["module.json","run.py"]}}]}}"#));
        write(&registry.join("busy/module.json"), &format!(r#"{{ "id": "me.busy", "name": "Busy", "version": "{version}", "command": "python3", "args": ["run.py"] }}"#));
        write(&registry.join("busy/run.py"), &format!("print('{version}')\n"));
    };
    let base = format!("file://{}", registry.display());
    publish("1.0.0");
    let dir = install(&base, &fetch_index(&base).unwrap()[0], &modules).unwrap();

    // A process working in the module's folder, like the running module.
    let mut busy = if cfg!(windows) {
        std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).current_dir(&dir).spawn().unwrap()
    } else {
        std::process::Command::new("sleep").arg("30").current_dir(&dir).spawn().unwrap()
    };
    publish("1.1.0");
    let package = fetch_index(&base).unwrap().remove(0);
    let result = install(&base, &package, &modules);
    if cfg!(windows) {
        let error = format!("{:#}", result.expect_err("the folder is in use"));
        assert!(error.contains("in use"), "{error}");
        assert_eq!(std::fs::read_to_string(dir.join("run.py")).unwrap(), "print('1.0.0')\n", "the installed version is whole");
        assert_eq!(discover(&modules)[0].version.as_deref(), Some("1.0.0"));
    } else {
        result.unwrap(); // nothing stops a rename elsewhere
    }
    let _ = busy.kill();
    let _ = busy.wait();

    install(&base, &package, &modules).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("run.py")).unwrap(), "print('1.1.0')\n");
    let names: Vec<String> = std::fs::read_dir(&modules).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["busy"], "no staging or old folder left behind");
    let _ = std::fs::remove_dir_all(&root);
}
