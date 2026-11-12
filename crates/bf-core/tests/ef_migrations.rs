//! Runs the EF Core migrations command lines against a real project: add a migration and script it.
//! Needs no database (SQL Server provider, nothing applied). Skips (passes) with a message when
//! `dotnet ef` is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use bf_core::ef::{self, Op};

/// The EF Core version the `dotnet-ef` tool has (the project's packages must match its major version).
fn ef_version() -> Option<String> {
    let output = Command::new("dotnet").args(["ef", "--version", "--no-color"]).output().ok()?;
    // dotnet-ef 10 prints the version to stdout, 11 to stderr.
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let version = text.lines().map(str::trim).rfind(|l| l.chars().next().is_some_and(|c| c.is_ascii_digit()))?.to_owned();
    output.status.success().then_some(version)
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dotnet(dir: &Path, args: &[String]) -> (bool, String) {
    let output = Command::new("dotnet").args(args).current_dir(dir).output().expect("dotnet");
    (output.status.success(), format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
}

#[test]
fn add_and_script_a_migration() {
    let Some(version) = ef_version() else {
        println!("SKIPPED: dotnet ef not installed (dotnet tool install --global dotnet-ef)");
        return;
    };
    let major = version.split('.').next().unwrap_or("10");
    let dir = std::env::temp_dir().join(format!("bf-core-it-ef-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let guard = TempDir(dir.clone());
    let project = dir.join("Shop.csproj");
    std::fs::write(
        &project,
        format!(
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net{major}.0</TargetFramework><Nullable>enable</Nullable></PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Microsoft.EntityFrameworkCore.SqlServer" Version="{version}" />
    <PackageReference Include="Microsoft.EntityFrameworkCore.Design" Version="{version}" />
  </ItemGroup>
</Project>"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("ShopContext.cs"),
        r#"using Microsoft.EntityFrameworkCore;
public class Order { public int Id { get; set; } public string Customer { get; set; } = ""; }
public class ShopContext : DbContext
{
    public DbSet<Order> Orders => Set<Order>();
    protected override void OnConfiguring(DbContextOptionsBuilder o) => o.UseSqlServer("Server=localhost;Database=Shop;Trusted_Connection=True");
}
public static class Program { public static void Main() { } }
"#,
    )
    .unwrap();

    let contexts = ef::find_contexts(&dir);
    assert_eq!(contexts.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["ShopContext"]);
    let context = &contexts[0].name;

    // As ByteForge does: restore first, `dotnet ef` doesn't.
    assert!(ef::needs_restore(&project));
    let (ok, out) = dotnet(&dir, &["restore".into(), project.to_string_lossy().into_owned()]);
    assert!(ok, "restore failed:\n{out}");
    assert!(!ef::needs_restore(&project));

    let (ok, out) = dotnet(&dir, &ef::args(&Op::Add("Init".into()), &project, &project, context));
    assert!(ok, "migrations add failed:\n{out}");
    let found = ef::migrations(&project, Some(context));
    assert_eq!(found.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Init"], "{out}");
    assert!(found[0].file.exists() && !found[0].file.to_string_lossy().contains("Designer"));

    let script = dir.join("out.sql");
    let (ok, out) = dotnet(&dir, &ef::args(&Op::Script(script.clone()), &project, &project, context));
    assert!(ok, "migrations script failed:\n{out}");
    let sql = std::fs::read_to_string(&script).unwrap();
    assert!(sql.contains("CREATE TABLE [Orders]"), "{sql}");

    // Which migrations are applied: no server here, so EF lists them without that and says why.
    let (listed, warning) = ef::list(&dir, &project, &project, context).expect("migrations list");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert!(listed[0].id.ends_with("_Init"));
    assert_eq!(listed[0].applied, None);
    assert!(warning.is_some(), "the reason the database couldn't be asked");

    // `migrations remove` asks the database whether the migration is applied, so it needs a server.
    drop(guard);
}

