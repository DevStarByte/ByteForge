//! Installing, updating and uninstalling module packages from a registry (a local `file://` one here,
//! through curl like the real one), and what ByteForge installs from it at start.

use std::path::Path;

use bf_core::plugins::discover;
use bf_core::registry::{PREINSTALLED, fetch_index, install, is_newer, to_install_at_start, uninstall};

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
        format!(r#"{{"modules":[{{"id":"me.todos","name":"TODOs","version":"{version}","dir":"todos","files":["module.json","bin/Todos.dll"],"requires":["dotnet"]}}]}}"#)
    };
    write(&registry.join("index.json"), &index("1.0.0"));
    write(&registry.join("todos/module.json"), r#"{ "id": "me.todos", "name": "TODOs", "version": "1.0.0", "runtime": "dotnet", "assembly": "bin/Todos.dll" }"#);
    write(&registry.join("todos/bin/Todos.dll"), "v1");
    let base = format!("file://{}", registry.display());

    let packages = fetch_index(&base).unwrap();
    assert_eq!(packages[0].requires, ["dotnet"]);
    let dir = install(&base, &packages[0], &modules).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("bin/Todos.dll")).unwrap(), "v1");
    let installed = discover(&modules);
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].version.as_deref(), Some("1.0.0"));

    // An update replaces the folder, except for the themes the module made.
    write(&dir.join("themes/todos.json"), r#"{ "name": "Todos" }"#);
    write(&registry.join("index.json"), &index("1.1.0"));
    write(&registry.join("todos/module.json"), r#"{ "id": "me.todos", "name": "TODOs", "version": "1.1.0", "runtime": "dotnet", "assembly": "bin/Todos.dll" }"#);
    write(&registry.join("todos/bin/Todos.dll"), "v2");
    let packages = fetch_index(&base).unwrap();
    assert!(is_newer(&packages[0].version, installed[0].version.as_deref().unwrap()));
    install(&base, &packages[0], &modules).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("bin/Todos.dll")).unwrap(), "v2");
    assert_eq!(discover(&modules)[0].version.as_deref(), Some("1.1.0"));
    assert!(dir.join("themes/todos.json").is_file(), "the module's own theme stays with the update");

    // A missing file fails without touching the installed version.
    std::fs::remove_file(registry.join("todos/bin/Todos.dll")).unwrap();
    assert!(install(&base, &packages[0], &modules).is_err());
    assert_eq!(std::fs::read_to_string(dir.join("bin/Todos.dll")).unwrap(), "v2");
    assert_eq!(std::fs::read_dir(&modules).unwrap().count(), 1, "staging folder left behind");

    uninstall(&modules, &dir).unwrap();
    assert!(discover(&modules).is_empty());
    assert!(uninstall(&modules, &root).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

/// At start ByteForge installs the ReSharper Formatter once (an uninstalled one stays uninstalled) and replaces
/// modules of the old kind (Python) with their .NET versions.
#[test]
fn what_is_installed_at_start() {
    let module = |json: &str| -> bf_core::plugins::ModuleManifest { json5::from_str(json).unwrap() };
    assert_eq!(PREINSTALLED, ["byteforge.resharper-format"]);
    assert_eq!(to_install_at_start(&[], &[]), ["byteforge.resharper-format"], "first start");
    assert!(to_install_at_start(&[], &["byteforge.resharper-format".to_owned()]).is_empty(), "installed once, then uninstalled");
    let python = module(r#"{ id: "byteforge.csharpier", name: "CSharpier", command: "python3", args: ["csharpier.py"] }"#);
    let dotnet = module(r#"{ id: "byteforge.docker", name: "Docker", runtime: "dotnet", assembly: "bin/Docker.dll" }"#);
    let icons = module(r#"{ id: "byteforge.azure", name: "Azure" }"#);
    assert!(python.is_legacy() && !dotnet.is_legacy() && !icons.is_legacy());
    let done = ["byteforge.resharper-format".to_owned()];
    assert_eq!(to_install_at_start(&[python, dotnet, icons], &done), ["byteforge.csharpier"], "only the Python module is replaced");
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

/// Every module in the module test data parses (a broken `module.json` is skipped silently at runtime), and the
/// settings they declare are well formed.
#[test]
fn bundled_modules_parse_with_their_settings() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join("tests/fixtures/modules");
    let folders = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().join("module.json").is_file()).count();
    let modules = discover(&dir);
    assert_eq!(modules.len(), folders, "a module.json in {} didn't parse", dir.display());
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
        write(&registry.join("index.json"), &format!(r#"{{"modules":[{{"id":"me.busy","name":"Busy","version":"{version}","dir":"busy","files":["module.json","bin/Busy.dll"]}}]}}"#));
        write(&registry.join("busy/module.json"), &format!(r#"{{ "id": "me.busy", "name": "Busy", "version": "{version}", "runtime": "dotnet", "assembly": "bin/Busy.dll" }}"#));
        write(&registry.join("busy/bin/Busy.dll"), version);
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
        assert_eq!(std::fs::read_to_string(dir.join("bin/Busy.dll")).unwrap(), "1.0.0", "the installed version is whole");
        assert_eq!(discover(&modules)[0].version.as_deref(), Some("1.0.0"));
    } else {
        result.unwrap(); // nothing stops a rename elsewhere
    }
    let _ = busy.kill();
    let _ = busy.wait();

    install(&base, &package, &modules).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("bin/Busy.dll")).unwrap(), "1.1.0");
    let names: Vec<String> = std::fs::read_dir(&modules).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["busy"], "no staging or old folder left behind");
    let _ = std::fs::remove_dir_all(&root);
}
