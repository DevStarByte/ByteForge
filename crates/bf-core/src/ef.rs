//! Entity Framework Core migrations: the `DbContext`s and migrations of a folder, found by scanning the
//! source (instant, no build), and the `dotnet ef` command lines that act on them.

use std::path::{Path, PathBuf};

/// A `DbContext` subclass and the project it is declared in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbContextInfo {
    pub project: PathBuf,
    /// The class name (what `--context` takes).
    pub name: String,
}

/// A migration, from its `[Migration("…")]` attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    /// `20260101093000_AddOrders`: what `database update` takes.
    pub id: String,
    /// `AddOrders`.
    pub name: String,
    /// The context of its `[DbContext(typeof(…))]` attribute.
    pub context: Option<String>,
    /// The migration's code file (not the `.Designer.cs`).
    pub file: PathBuf,
}

/// What to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// `migrations add <name>`.
    Add(String),
    /// `migrations remove`: the last migration, when it isn't applied.
    Remove,
    /// `database update [<migration>]`: to the latest, or to this migration id (`0` reverts all).
    Update(Option<String>),
    /// `migrations script --idempotent --output <file>`.
    Script(PathBuf),
    /// `database drop --force`.
    Drop,
    /// `dbcontext scaffold <connection> <provider> --output-dir <dir>`: entity classes and a DbContext
    /// from an existing database.
    Scaffold { connection: String, provider: String, output_dir: String },
}

/// The database providers offered for scaffolding: (name, NuGet package).
pub const PROVIDERS: &[(&str, &str)] = &[
    ("SQL Server", "Microsoft.EntityFrameworkCore.SqlServer"),
    ("PostgreSQL", "Npgsql.EntityFrameworkCore.PostgreSQL"),
    ("SQLite", "Microsoft.EntityFrameworkCore.Sqlite"),
    ("MySQL / MariaDB", "Pomelo.EntityFrameworkCore.MySql"),
    ("Oracle", "Oracle.EntityFrameworkCore"),
];

/// A migration as `dotnet ef migrations list` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedMigration {
    pub id: String,
    /// Applied to the database; `None` when the database couldn't be asked.
    pub applied: Option<bool>,
}

/// `dotnet ef migrations list --json --prefix-output` for a context.
pub fn list_args(project: &Path, startup: &Path, context: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["ef".into(), "migrations".into(), "list".into(), "--json".into(), "--prefix-output".into()];
    args.extend(["--project".into(), project.to_string_lossy().into_owned()]);
    if startup != project {
        args.extend(["--startup-project".into(), startup.to_string_lossy().into_owned()]);
    }
    if !context.is_empty() {
        args.extend(["--context".into(), context.to_owned()]);
    }
    args.push("--no-color".into());
    args
}

/// The migrations in `migrations list --json --prefix-output` output (the JSON is on the `data:` lines),
/// and why the database couldn't be asked, when it couldn't.
pub fn parse_list(output: &str) -> (Vec<ListedMigration>, Option<String>) {
    let json: String = output.lines().filter_map(|l| l.strip_prefix("data:")).collect::<Vec<_>>().join("\n");
    let listed = serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|m| Some(ListedMigration { id: m.get("id")?.as_str()?.to_owned(), applied: m.get("applied").and_then(|a| a.as_bool()) }))
        .collect();
    let warning = output
        .lines()
        .filter_map(|l| l.strip_prefix("warn:"))
        .map(str::trim)
        .find(|l| l.contains("accessing the database"))
        .map(|l| l.split(" Error: ").nth(1).unwrap_or(l).chars().take(200).collect());
    (listed, warning)
}

/// Asks EF which migrations are applied (it builds the project first, so this takes a few seconds).
pub fn list(root: &Path, project: &Path, startup: &Path, context: &str) -> anyhow::Result<(Vec<ListedMigration>, Option<String>)> {
    let mut command = std::process::Command::new("dotnet");
    command.args(list_args(project, startup, context)).current_dir(root).env("DOTNET_NOLOGO", "1").stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = command.output()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        let why = text.lines().filter_map(|l| l.strip_prefix("error:")).map(str::trim).next().unwrap_or("dotnet ef migrations list failed").to_owned();
        anyhow::bail!(why);
    }
    Ok(parse_list(&text))
}

/// The `DbContext`s of the (non-test) projects below `root`.
pub fn find_contexts(root: &Path) -> Vec<DbContextInfo> {
    let projects = crate::dotnet::projects(root);
    let mut out = Vec::new();
    for project in &projects {
        if std::fs::read_to_string(project).is_ok_and(|t| crate::testing::is_test_project_text(&t)) {
            continue;
        }
        for file in project_sources(project, &projects) {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            for name in scan_contexts(&text) {
                if !out.iter().any(|c: &DbContextInfo| c.project == *project && c.name == name) {
                    out.push(DbContextInfo { project: project.clone(), name });
                }
            }
        }
    }
    out
}

/// The migrations of a project (of `context` when given), oldest first.
pub fn migrations(project: &Path, context: Option<&str>) -> Vec<Migration> {
    let projects = crate::dotnet::projects(project.parent().unwrap_or(project));
    let mut out: Vec<Migration> = Vec::new();
    for file in project_sources(project, &projects) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let Some((id, ctx)) = scan_migration(&text) else { continue };
        if context.is_some_and(|c| ctx.as_deref().is_some_and(|m| m != c)) || out.iter().any(|m| m.id == id) {
            continue;
        }
        // The attributes live in Foo.Designer.cs; the code to read is Foo.cs.
        let name = file.file_name().map(|n| n.to_string_lossy().replace(".Designer.cs", ".cs")).unwrap_or_default();
        let code = file.with_file_name(name);
        let file = if code.exists() { code } else { file };
        let short = id.split_once('_').map_or(id.as_str(), |(_, n)| n).to_owned();
        out.push(Migration { id, name: short, context: ctx, file });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// The `.cs` files of a project, without those of projects nested in its folder.
fn project_sources(project: &Path, projects: &[PathBuf]) -> Vec<PathBuf> {
    let Some(dir) = project.parent() else { return Vec::new() };
    let others: Vec<&Path> = projects.iter().filter(|p| p.as_path() != project).filter_map(|p| p.parent()).filter(|d| *d != dir && d.starts_with(dir)).collect();
    let mut files = Vec::new();
    crate::testing::collect_sources(dir, &others, 0, &mut files);
    files.sort();
    files
}

/// Names of the non-abstract classes in a C# file that derive from a `…DbContext` (`DbContext`,
/// `IdentityDbContext<User>`, …).
pub fn scan_contexts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let code = line.split("//").next().unwrap_or_default();
        let Some(at) = find_word(code, "class") else { continue };
        if find_word(&code[..at], "abstract").is_some() {
            continue;
        }
        let rest = code[at + "class".len()..].trim_start();
        let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        let Some((_, bases)) = rest.split_once(':') else { continue };
        let first = bases.split(['{', ',', '(']).next().unwrap_or_default().trim();
        let base = first.split('<').next().unwrap_or_default().trim();
        let base = base.rsplit('.').next().unwrap_or_default();
        if !name.is_empty() && base.ends_with("DbContext") {
            out.push(name);
        }
    }
    out
}

/// The position of `word` in `text` as a whole word.
fn find_word(text: &str, word: &str) -> Option<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).map(|(i, _)| i).find(|&i| {
        let before = text[..i].chars().next_back().is_none_or(|c| !is_word(c));
        let after = text[i + word.len()..].chars().next().is_none_or(|c| !is_word(c));
        before && after
    })
}

/// The migration id and context of a migration file: `[Migration("…")]` and `[DbContext(typeof(…))]`.
pub fn scan_migration(text: &str) -> Option<(String, Option<String>)> {
    let id = attribute_arg(text, "[Migration(")?.trim_matches('"').to_owned();
    let context = attribute_arg(text, "[DbContext(").map(|a| {
        let a = a.trim();
        let inner = a.strip_prefix("typeof(").and_then(|s| s.strip_suffix(')')).unwrap_or(a);
        inner.rsplit('.').next().unwrap_or(inner).to_owned()
    });
    Some((id, context))
}

/// The text between `start` and the `)]` that closes the attribute.
fn attribute_arg<'a>(text: &'a str, start: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let end = text[from..].find(")]")?;
    Some(&text[from..from + end])
}

/// The `dotnet` arguments for `op` on `project`, started through `startup` (the app whose configuration
/// holds the connection string), for `context`.
pub fn args(op: &Op, project: &Path, startup: &Path, context: &str) -> Vec<String> {
    let mut args: Vec<String> = match op {
        Op::Add(name) => vec!["ef".into(), "migrations".into(), "add".into(), name.clone()],
        Op::Remove => vec!["ef".into(), "migrations".into(), "remove".into()],
        Op::Update(None) => vec!["ef".into(), "database".into(), "update".into()],
        Op::Update(Some(target)) => vec!["ef".into(), "database".into(), "update".into(), target.clone()],
        Op::Script(out) => vec!["ef".into(), "migrations".into(), "script".into(), "--idempotent".into(), "--output".into(), out.to_string_lossy().into_owned()],
        Op::Drop => vec!["ef".into(), "database".into(), "drop".into(), "--force".into()],
        Op::Scaffold { connection, provider, output_dir } => {
            vec!["ef".into(), "dbcontext".into(), "scaffold".into(), connection.clone(), provider.clone(), "--output-dir".into(), output_dir.clone()]
        }
    };
    args.extend(["--project".into(), project.to_string_lossy().into_owned()]);
    if startup != project {
        args.extend(["--startup-project".into(), startup.to_string_lossy().into_owned()]);
    }
    if !context.is_empty() {
        args.extend(["--context".into(), context.to_owned()]);
    }
    args.push("--no-color".into());
    args
}

/// Whether `dotnet ef` would fail on the project for lack of a restore (`obj/project.assets.json`): it
/// doesn't restore by itself and then only says "Unable to retrieve project metadata".
pub fn needs_restore(project: &Path) -> bool {
    project.parent().is_some_and(|dir| !dir.join("obj").join("project.assets.json").exists())
}

/// Whether an output line says the `dotnet-ef` tool isn't installed.
pub fn is_tool_missing(line: &str) -> bool {
    line.contains("dotnet-ef does not exist") || (line.contains("Could not execute because the specified command or file was not found") && !line.contains("dotnet-"))
}

/// A valid migration name (a C# class name) from what was typed: "add orders table" → "AddOrdersTable".
pub fn migration_name(typed: &str) -> Option<String> {
    let name: String = typed
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect();
    let name = if name.chars().next().is_some_and(|c| c.is_ascii_digit()) { format!("_{name}") } else { name };
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_db_contexts() {
        let text = "public class ShopContext : DbContext\n{\n}\npublic sealed class AuthDb(DbContextOptions<AuthDb> o) : IdentityDbContext<User>(o) { }\n\
                    public abstract class BaseContext : DbContext { }\nclass Other : Microsoft.EntityFrameworkCore.DbContext {}\nclass Repo : IRepository {}\n// class Old : DbContext\n";
        assert_eq!(scan_contexts(text), ["ShopContext", "AuthDb", "Other"]);
    }

    #[test]
    fn reads_migration_attributes() {
        let text = "[DbContext(typeof(Shop.Data.ShopContext))]\n[Migration(\"20260101093000_AddOrders\")]\npartial class AddOrders { }";
        assert_eq!(scan_migration(text), Some(("20260101093000_AddOrders".into(), Some("ShopContext".into()))));
        assert_eq!(scan_migration("class X {}"), None);
    }

    #[test]
    fn builds_command_lines() {
        let (p, s) = (Path::new("/w/Data/Data.csproj"), Path::new("/w/Web/Web.csproj"));
        assert_eq!(
            args(&Op::Add("Init".into()), p, s, "ShopContext"),
            ["ef", "migrations", "add", "Init", "--project", "/w/Data/Data.csproj", "--startup-project", "/w/Web/Web.csproj", "--context", "ShopContext", "--no-color"]
        );
        assert_eq!(args(&Op::Update(Some("0".into())), p, p, ""), ["ef", "database", "update", "0", "--project", "/w/Data/Data.csproj", "--no-color"]);
        assert_eq!(args(&Op::Drop, p, p, "")[..4], ["ef", "database", "drop", "--force"]);
    }

    #[test]
    fn reads_the_applied_state() {
        let out = "info:    Build started...\ninfo:    Build succeeded.\ndata:    [\ndata:      {\ndata:        \"id\": \"20260101_Init\",\ndata:        \"name\": \"Init\",\ndata:        \"applied\": true\ndata:      },\ndata:      {\ndata:        \"id\": \"20260201_AddOrders\",\ndata:        \"applied\": false\ndata:      }\ndata:    ]\n";
        let (listed, warning) = parse_list(out);
        assert_eq!(listed, [ListedMigration { id: "20260101_Init".into(), applied: Some(true) }, ListedMigration { id: "20260201_AddOrders".into(), applied: Some(false) }]);
        assert_eq!(warning, None);
        let offline = "warn:    An error occurred while accessing the database. Continuing without the information provided by the database. Error: A network-related error occurred\ndata:    [\ndata:      { \"id\": \"20260101_Init\", \"applied\": null }\ndata:    ]\n";
        let (listed, warning) = parse_list(offline);
        assert_eq!(listed[0].applied, None);
        assert_eq!(warning.as_deref(), Some("A network-related error occurred"));
        assert_eq!(parse_list("error: nope").0, []);
    }

    #[test]
    fn scaffold_command_line() {
        let p = Path::new("/w/Data/Data.csproj");
        let op = Op::Scaffold { connection: "Server=.;Database=Shop".into(), provider: "Microsoft.EntityFrameworkCore.SqlServer".into(), output_dir: "Models".into() };
        assert_eq!(
            args(&op, p, p, ""),
            ["ef", "dbcontext", "scaffold", "Server=.;Database=Shop", "Microsoft.EntityFrameworkCore.SqlServer", "--output-dir", "Models", "--project", "/w/Data/Data.csproj", "--no-color"]
        );
        assert_eq!(list_args(p, Path::new("/w/Web/Web.csproj"), "ShopContext")[..5], ["ef", "migrations", "list", "--json", "--prefix-output"]);
    }

    #[test]
    fn turns_typed_text_into_a_migration_name() {
        assert_eq!(migration_name("add orders table").as_deref(), Some("AddOrdersTable"));
        assert_eq!(migration_name("AddOrders").as_deref(), Some("AddOrders"));
        assert_eq!(migration_name("2nd try").as_deref(), Some("_2ndTry"));
        assert_eq!(migration_name(" - ").as_deref(), None);
    }

    #[test]
    fn spots_the_missing_tool() {
        assert!(is_tool_missing("Could not execute because the specified command or file was not found."));
        assert!(is_tool_missing("  * You intended to execute a .NET program, but dotnet-ef does not exist."));
        assert!(!is_tool_missing("Build succeeded."));
    }

    #[test]
    fn scans_a_project_folder() {
        let dir = std::env::temp_dir().join(format!("bf-ef-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Migrations")).unwrap();
        std::fs::write(dir.join("Shop.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>").unwrap();
        std::fs::write(dir.join("ShopContext.cs"), "class ShopContext : DbContext {}").unwrap();
        for (id, name) in [("20260102000000_Second", "Second"), ("20260101000000_Init", "Init")] {
            std::fs::write(dir.join(format!("Migrations/{id}.cs")), format!("partial class {name} : Migration {{}}")).unwrap();
            std::fs::write(dir.join(format!("Migrations/{id}.Designer.cs")), format!("[DbContext(typeof(ShopContext))]\n[Migration(\"{id}\")]\npartial class {name} {{}}")).unwrap();
        }
        let contexts = find_contexts(&dir);
        assert_eq!(contexts.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["ShopContext"]);
        let found = migrations(&dir.join("Shop.csproj"), Some("ShopContext"));
        assert_eq!(found.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Init", "Second"]);
        assert!(found[0].file.ends_with("Migrations/20260101000000_Init.cs"));
        assert!(migrations(&dir.join("Shop.csproj"), Some("OtherContext")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
