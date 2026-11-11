//! Lists the real `dotnet new` templates and creates a project and an item the way New Project / New Item
//! do. Skips (passes) with a message when `dotnet` is not installed.

use std::process::Command;

use bf_core::templates::{TemplateKind, list, namespace_for, solution_in};

fn have_dotnet() -> bool {
    Command::new("dotnet").arg("--version").output().is_ok_and(|o| o.status.success())
}

#[test]
fn lists_templates_and_creates_a_project_in_the_solution() {
    if !have_dotnet() {
        eprintln!("dotnet not installed; skipping");
        return;
    }
    let projects = list(TemplateKind::Project).unwrap();
    assert!(projects.iter().any(|t| t.short_name == "console"), "{projects:?}");
    assert!(projects.iter().any(|t| t.short_name == "classlib"), "{projects:?}");
    let items = list(TemplateKind::Item).unwrap();
    assert!(items.iter().any(|t| t.short_name == "gitignore" && t.languages.is_empty()), "{items:?}");

    let root = std::env::temp_dir().join(format!("bf-templates-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let dotnet = |args: &[&str]| {
        let out = Command::new("dotnet").args(args).current_dir(&root).env("DOTNET_NOLOGO", "1").output().unwrap();
        assert!(out.status.success(), "dotnet {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    dotnet(&["new", "sln", "-n", "Shop"]);
    let sln = solution_in(&root).expect("one solution");
    let folder = root.join("Shop.Api");
    dotnet(&["new", "classlib", "-n", "Shop.Api", "-o", &folder.to_string_lossy()]);
    let project = folder.join("Shop.Api.csproj");
    assert!(project.is_file());
    dotnet(&["sln", &sln.to_string_lossy(), "add", &project.to_string_lossy()]);
    assert_eq!(namespace_for(&folder.join("Models")).as_deref(), Some("Shop.Api.Models"));
    // An unnamed config item, as New Item runs it.
    dotnet(&["new", "gitignore", "-o", &folder.to_string_lossy()]);
    assert!(folder.join(".gitignore").is_file());
    std::fs::remove_dir_all(&root).unwrap();
}
