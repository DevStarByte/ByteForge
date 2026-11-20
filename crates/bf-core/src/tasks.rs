//! Tasks: commands a workspace runs often, each run in a terminal. ByteForge's own tasks live in
//! `.byteforge/tasks.json`; next to them come the ones the workspace already declares elsewhere:
//! package.json scripts (run with npm, pnpm, yarn or bun, whichever the lock file names), Makefile targets
//! and justfile recipes.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// ByteForge's per-workspace folder, and the tasks file in it.
pub const FOLDER: &str = ".byteforge";
pub const FILE: &str = "tasks.json";

/// How deep below the workspace root package.json files, Makefiles and justfiles are looked for.
const MAX_DEPTH: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub name: String,
    /// Run in the user's shell.
    pub command: String,
    /// Where the command runs (the workspace root when the task doesn't say).
    pub cwd: PathBuf,
    /// The header the task is listed under ("Tasks", "package.json", "web/Makefile", …).
    pub group: String,
    /// Save all files before running (default true).
    pub save_all: bool,
    pub source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `.byteforge/tasks.json`.
    Workspace,
    Npm,
    Make,
    Just,
}

impl Task {
    /// Identifies the task across refreshes (for "Rerun Last Task" and the view's clicks).
    pub fn key(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{}", self.group, self.name, self.cwd.display())
    }
}

/// The tasks of a workspace, and what was wrong with `.byteforge/tasks.json` if anything.
#[derive(Clone, Debug, Default)]
pub struct Tasks {
    pub tasks: Vec<Task>,
    pub error: Option<String>,
}

pub fn tasks_file(root: &Path) -> PathBuf {
    root.join(FOLDER).join(FILE)
}

/// Every task of the workspace at `root`: its own first, then package.json scripts, Makefile targets and
/// justfile recipes (the root's before those of subfolders). Blocking; reads a few files.
pub fn discover(root: &Path) -> Tasks {
    let mut out = Tasks::default();
    match std::fs::read_to_string(tasks_file(root)) {
        Ok(text) => match parse_tasks_file(&text, root) {
            Ok(tasks) => out.tasks.extend(tasks),
            Err(e) => out.error = Some(format!("{}/{}: {e}", FOLDER, FILE)),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => out.error = Some(format!("{}/{}: {e}", FOLDER, FILE)),
    }
    let mut dirs = Vec::new();
    collect_dirs(root, 0, &mut dirs);
    for dir in &dirs {
        let label = |file: &str| match dir.strip_prefix(root).ok().filter(|r| !r.as_os_str().is_empty()) {
            Some(rel) => format!("{}/{file}", rel.to_string_lossy().replace('\\', "/")),
            None => file.to_owned(),
        };
        if let Ok(text) = std::fs::read_to_string(dir.join("package.json")) {
            let manager = package_manager(dir);
            for script in package_scripts(&text) {
                out.tasks.push(Task {
                    command: format!("{manager} run {}", shell_word(&script)),
                    name: script,
                    cwd: dir.clone(),
                    group: label("package.json"),
                    save_all: true,
                    source: Source::Npm,
                });
            }
        }
        if let Some((file, text)) = ["Makefile", "makefile", "GNUmakefile"].iter().find_map(|f| Some((*f, std::fs::read_to_string(dir.join(f)).ok()?))) {
            for target in make_targets(&text) {
                out.tasks.push(Task {
                    command: format!("make {}", shell_word(&target)),
                    name: target,
                    cwd: dir.clone(),
                    group: label(file),
                    save_all: true,
                    source: Source::Make,
                });
            }
        }
        if let Some((file, text)) = ["justfile", "Justfile", ".justfile"].iter().find_map(|f| Some((*f, std::fs::read_to_string(dir.join(f)).ok()?))) {
            for recipe in just_recipes(&text) {
                out.tasks.push(Task {
                    command: format!("just {}", shell_word(&recipe)),
                    name: recipe,
                    cwd: dir.clone(),
                    group: label(file),
                    save_all: true,
                    source: Source::Just,
                });
            }
        }
    }
    out
}

/// The root, then its subfolders breadth-first, skipping dependency, build and hidden folders.
fn collect_dirs(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    out.push(root.to_path_buf());
    let mut level = vec![root.to_path_buf()];
    for _ in depth..MAX_DEPTH {
        let mut next = Vec::new();
        for dir in &level {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut subs: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    !name.starts_with('.') && !matches!(name.as_str(), "node_modules" | "bin" | "obj" | "target" | "dist" | "build" | "out" | "vendor" | "packages")
                })
                .map(|e| e.path())
                .collect();
            subs.sort();
            next.extend(subs);
        }
        out.extend(next.iter().cloned());
        level = next;
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileTask {
    name: String,
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    save_all: Option<bool>,
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    tasks: Vec<FileTask>,
}

/// `{ "tasks": [ { "name", "command", "cwd"?, "group"?, "saveAll"? } ] }`, comments and trailing commas
/// allowed.
pub fn parse_tasks_file(text: &str, root: &Path) -> Result<Vec<Task>, String> {
    let file: File = json5::from_str(text).map_err(|e| e.to_string())?;
    Ok(file
        .tasks
        .into_iter()
        .filter(|t| !t.name.trim().is_empty() && !t.command.trim().is_empty())
        .map(|t| Task {
            cwd: match t.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                Some(c) => root.join(c),
                None => root.to_path_buf(),
            },
            group: t.group.filter(|g| !g.trim().is_empty()).unwrap_or_else(|| "Tasks".to_owned()),
            save_all: t.save_all.unwrap_or(true),
            name: t.name,
            command: t.command,
            source: Source::Workspace,
        })
        .collect())
}

/// What "Tasks: Configure Tasks" writes into a new `.byteforge/tasks.json`.
pub const EXAMPLE: &str = r#"// ByteForge tasks: run them from the Tasks view or "Tasks: Run Task…" (each in a terminal).
// name: shown in the list; command: run in your shell; cwd: relative to the workspace (optional);
// group: a header in the Tasks view (optional); saveAll: save open files first (default true).
{
  "tasks": [
    {
      "name": "Build",
      "command": "dotnet build",
      "group": "Build"
    },
    {
      "name": "Test",
      "command": "dotnet test",
      "group": "Build"
    },
    {
      "name": "Watch",
      "command": "dotnet watch run",
      "saveAll": false
    }
  ]
}
"#;

/// The script names of a package.json, in file order.
pub fn package_scripts(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    match value.get("scripts").and_then(|s| s.as_object()) {
        // serde_json keeps the file's order (preserve_order) only when enabled, so read the order from the text.
        Some(scripts) => {
            let mut names: Vec<String> = scripts.keys().cloned().collect();
            names.sort_by_key(|n| text.find(&format!("\"{n}\"")).unwrap_or(usize::MAX));
            names
        }
        None => Vec::new(),
    }
}

/// npm, pnpm, yarn or bun, from the lock file next to package.json (npm when there is none).
pub fn package_manager(dir: &Path) -> &'static str {
    if dir.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if dir.join("yarn.lock").exists() {
        "yarn"
    } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        "bun"
    } else {
        "npm"
    }
}

/// A Makefile's explicit targets, in file order: not special targets (`.PHONY`), pattern rules (`%.o`),
/// variable assignments or targets with variables in their names.
pub fn make_targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with(['\t', ' ', '#']) {
            continue;
        }
        let Some((targets, rest)) = line.split_once(':') else { continue };
        // `x := 1`, `x ::= 1`
        if rest.starts_with('=') || rest.starts_with(":=") || targets.contains('=') {
            continue;
        }
        for target in targets.split_whitespace() {
            let ok = !target.starts_with('.') && !target.contains(['%', '$', '(', ')']) && !out.iter().any(|t| t == target);
            if ok {
                out.push(target.to_owned());
            }
        }
    }
    out
}

/// A justfile's public recipes, in file order (not `_private` ones or those marked `[private]`).
pub fn just_recipes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut private = false;
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#']) || line.trim().is_empty() {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            private |= trimmed.contains("private");
            continue;
        }
        let first = trimmed.split_whitespace().next().unwrap_or_default();
        if matches!(first, "set" | "alias" | "export" | "import" | "mod") || trimmed.contains(":=") {
            private = false;
            continue;
        }
        let Some((head, _)) = trimmed.split_once(':') else {
            private = false;
            continue;
        };
        let name = head.split_whitespace().next().unwrap_or_default().trim_start_matches('@');
        let valid = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-');
        if valid && !name.starts_with('_') && !private {
            out.push(name.to_owned());
        }
        private = false;
    }
    out
}

/// `word` as one shell word (quoted when it needs to be).
fn shell_word(word: &str) -> String {
    if word.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/' | '+' | '=' | '@')) {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tasks_file_with_comments_and_defaults() {
        let root = Path::new("/w");
        let tasks = parse_tasks_file(EXAMPLE, root).unwrap();
        assert_eq!(tasks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Build", "Test", "Watch"]);
        assert_eq!(tasks[0].group, "Build");
        assert_eq!(tasks[2].group, "Tasks", "the default group");
        assert!(tasks[0].save_all && !tasks[2].save_all);
        assert_eq!(tasks[0].cwd, root);
        let tasks = parse_tasks_file(r#"{ tasks: [ { name: "Docs", command: "make html", cwd: "docs", }, { name: "", command: "x" } ] }"#, root).unwrap();
        assert_eq!(tasks.len(), 1, "a task without a name is skipped");
        assert_eq!(tasks[0].cwd, root.join("docs"));
        assert!(parse_tasks_file("{ \"tasks\": [ { \"name\": 1 } ] }", root).is_err());
    }

    #[test]
    fn package_scripts_in_file_order() {
        let json = r#"{ "name": "web", "scripts": { "dev": "vite", "build": "vite build", "test:unit": "vitest" } }"#;
        assert_eq!(package_scripts(json), ["dev", "build", "test:unit"]);
        assert!(package_scripts("{}").is_empty() && package_scripts("not json").is_empty());
    }

    #[test]
    fn make_targets_skip_special_pattern_and_variable_lines() {
        let make = ".PHONY: build test\nCC := gcc\nOUT = bin\nbuild: deps\n\tgo build\ntest:\n\tgo test\n%.o: %.c\n\tcc\n$(OUT)/app: build\n# c: x\nclean distclean:\n\trm -rf out\n";
        assert_eq!(make_targets(make), ["build", "test", "clean", "distclean"]);
    }

    #[test]
    fn just_recipes_skip_private_settings_and_assignments() {
        let just = "set shell := [\"bash\", \"-c\"]\nversion := \"1\"\n# Build it\nbuild target='debug':\n    cargo build\n@test: build\n    cargo test\n_helper:\n    echo\n[private]\nsecret:\n    echo\nalias b := build\nrelease: (build \"release\")\n";
        assert_eq!(just_recipes(just), ["build", "test", "release"]);
    }

    #[test]
    fn discovers_tasks_across_the_workspace() {
        let dir = std::env::temp_dir().join(format!("bf-tasks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |rel: &str, text: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(".byteforge/tasks.json", "// mine\n{ \"tasks\": [ { \"name\": \"Hello\", \"command\": \"echo hi\" } ] }");
        write("web/package.json", r#"{ "scripts": { "dev": "vite" } }"#);
        write("web/pnpm-lock.yaml", "");
        write("web/node_modules/x/package.json", r#"{ "scripts": { "hidden": "x" } }"#);
        write("Makefile", "all:\n\techo\n");
        write("tools/justfile", "fmt:\n    cargo fmt\n");
        let found = discover(&dir);
        assert!(found.error.is_none());
        let summary: Vec<(String, String, String)> = found.tasks.iter().map(|t| (t.group.clone(), t.name.clone(), t.command.clone())).collect();
        assert_eq!(
            summary,
            [
                ("Tasks".into(), "Hello".into(), "echo hi".into()),
                ("Makefile".into(), "all".into(), "make all".into()),
                ("tools/justfile".into(), "fmt".into(), "just fmt".into()),
                ("web/package.json".into(), "dev".into(), "pnpm run dev".into()),
            ]
        );
        assert_eq!(found.tasks[3].cwd, dir.join("web"));
        // A broken tasks file says why and the other tasks are still there.
        write(".byteforge/tasks.json", "{ tasks: [ ");
        let found = discover(&dir);
        assert!(found.error.as_deref().is_some_and(|e| e.starts_with(".byteforge/tasks.json")));
        assert_eq!(found.tasks.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
