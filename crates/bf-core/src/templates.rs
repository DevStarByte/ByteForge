//! `dotnet new`: the installed project and item templates, plus ByteForge's own C# item templates
//! (class, interface, record, enum, struct), which the SDK doesn't ship.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateKind {
    Project,
    Item,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    /// The first short name (`dotnet new <short>`).
    pub short_name: String,
    /// Languages as listed, e.g. "[C#],F#" (empty for language-neutral templates like `gitignore`).
    pub languages: String,
    pub tags: String,
}

/// The installed templates of `kind` that support C# or are language-neutral (`dotnet new list`).
pub fn list(kind: TemplateKind) -> anyhow::Result<Vec<Template>> {
    let kind_arg = match kind {
        TemplateKind::Project => "project",
        TemplateKind::Item => "item",
    };
    let mut command = Command::new("dotnet");
    command
        .args(["new", "list", "--type", kind_arg])
        .env("DOTNET_NOLOGO", "1")
        .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = command.output().context("could not start dotnet (is the .NET SDK installed?)")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let templates = parse_list(&text);
    if templates.is_empty() && !out.status.success() {
        bail!("dotnet new list failed: {}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or_default());
    }
    Ok(templates.into_iter().filter(|t| t.languages.is_empty() || t.languages.contains("C#")).collect())
}

/// Parses the table `dotnet new list` prints. Columns are found from the dashed line under the header,
/// so localized headers don't matter.
pub fn parse_list(text: &str) -> Vec<Template> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(dash_ix) = lines.iter().position(|l| l.trim_start().starts_with("---") && l.contains("  ")) else { return Vec::new() };
    // Column start and end offsets (in chars) from the runs of dashes.
    let dash_line: Vec<char> = lines[dash_ix].chars().collect();
    let mut columns: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < dash_line.len() {
        if dash_line[i] == '-' {
            let start = i;
            while i < dash_line.len() && dash_line[i] == '-' {
                i += 1;
            }
            columns.push((start, i));
        } else {
            i += 1;
        }
    }
    if columns.len() < 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in &lines[dash_ix + 1..] {
        if line.trim().is_empty() {
            break;
        }
        let chars: Vec<char> = line.chars().collect();
        let cell = |c: usize| -> String {
            let (start, _) = columns[c];
            // A cell runs to the next column's start (the last one to the end of the line).
            let end = columns.get(c + 1).map_or(chars.len(), |n| n.0).min(chars.len());
            if start >= end { String::new() } else { chars[start..end].iter().collect::<String>().trim().to_owned() }
        };
        let name = cell(0);
        let short_name = cell(1).split(',').next().unwrap_or_default().trim().to_owned();
        if name.is_empty() || short_name.is_empty() {
            continue;
        }
        let languages = if columns.len() > 2 { cell(2) } else { String::new() };
        let tags = if columns.len() > 3 { cell(3) } else { String::new() };
        out.push(Template { name, short_name, languages, tags });
    }
    out
}

/// A C# file ByteForge creates itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CSharpItem {
    Class,
    Interface,
    Record,
    Enum,
    Struct,
}

impl CSharpItem {
    pub const ALL: [CSharpItem; 5] = [CSharpItem::Class, CSharpItem::Interface, CSharpItem::Record, CSharpItem::Enum, CSharpItem::Struct];

    pub fn label(self) -> &'static str {
        match self {
            CSharpItem::Class => "Class",
            CSharpItem::Interface => "Interface",
            CSharpItem::Record => "Record",
            CSharpItem::Enum => "Enum",
            CSharpItem::Struct => "Struct",
        }
    }

    pub fn keyword(self) -> &'static str {
        match self {
            CSharpItem::Class => "class",
            CSharpItem::Interface => "interface",
            CSharpItem::Record => "record",
            CSharpItem::Enum => "enum",
            CSharpItem::Struct => "struct",
        }
    }

    pub fn from_keyword(keyword: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.keyword() == keyword)
    }

    /// The file's text: a file-scoped namespace (when there is one) and an empty type.
    pub fn render(self, namespace: Option<&str>, name: &str) -> String {
        let body = match self {
            CSharpItem::Record => format!("public record {name};\n"),
            other => format!("public {} {name}\n{{\n}}\n", other.keyword()),
        };
        match namespace.filter(|n| !n.is_empty()) {
            Some(ns) => format!("namespace {ns};\n\n{body}"),
            None => body,
        }
    }
}

/// `MyType.cs` → `MyType`; turns anything that isn't valid in a C# identifier into `_`.
pub fn type_name(input: &str) -> String {
    let stem = input.trim().strip_suffix(".cs").unwrap_or(input.trim());
    identifier(stem)
}

fn identifier(text: &str) -> String {
    let mut out: String = text.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// The namespace for a new file in `dir`, the way Visual Studio picks it: the nearest project's
/// `<RootNamespace>` (else its file name) plus the folders from the project down to `dir`.
pub fn namespace_for(dir: &Path) -> Option<String> {
    let project = nearest_project(dir)?;
    let project_dir = project.parent()?;
    let text = std::fs::read_to_string(&project).unwrap_or_default();
    let root = tag_value(&text, "RootNamespace").unwrap_or_else(|| project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let mut parts: Vec<String> = root.split('.').map(identifier).filter(|p| !p.is_empty()).collect();
    if let Ok(rel) = dir.strip_prefix(project_dir) {
        parts.extend(rel.components().map(|c| identifier(&c.as_os_str().to_string_lossy())).filter(|p| !p.is_empty()));
    }
    (!parts.is_empty()).then(|| parts.join("."))
}

/// The closest `.csproj` in `dir` or one of its parents.
pub fn nearest_project(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find_map(|d| {
        let mut projects: Vec<PathBuf> = std::fs::read_dir(d)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("csproj")))
            .collect();
        projects.sort();
        projects.into_iter().next()
    })
}

/// The text of the first `<tag>…</tag>` in an MSBuild file.
pub fn tag_value(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    let value = xml[start..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The solution file in `root` to add new projects to, when there is exactly one.
pub fn solution_in(root: &Path) -> Option<PathBuf> {
    let solutions: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sln") || e.eq_ignore_ascii_case("slnx")))
        .collect();
    (solutions.len() == 1).then(|| solutions[0].clone())
}

/// Where `dotnet user-secrets` keeps a project's secrets.
pub fn user_secrets_file(id: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        let appdata = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(appdata).join("Microsoft").join("UserSecrets").join(id).join("secrets.json"))
    } else {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".microsoft").join("usersecrets").join(id).join("secrets.json"))
    }
}

/// The project's `<UserSecretsId>`, if it has one.
pub fn user_secrets_id(project: &Path) -> Option<String> {
    tag_value(&std::fs::read_to_string(project).ok()?, "UserSecretsId")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "These templates matched your input: --type='item'

Template Name                          Short Name                    Language    Tags
-------------------------------------  ----------------------------  ----------  --------------------------
API Controller                         apicontroller                 [C#]        Web/ASP.NET
dotnet gitignore file                  gitignore,.gitignore                      Config
MSTest Test Class                      mstest-class                  [C#],F#,VB  Test/MSTest
";

    #[test]
    fn parses_the_template_table() {
        let list = parse_list(LIST);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], Template { name: "API Controller".into(), short_name: "apicontroller".into(), languages: "[C#]".into(), tags: "Web/ASP.NET".into() });
        assert_eq!(list[1].short_name, "gitignore");
        assert_eq!(list[1].languages, "");
        assert_eq!(list[2].languages, "[C#],F#,VB");
    }

    #[test]
    fn renders_csharp_items() {
        assert_eq!(CSharpItem::Class.render(Some("App.Models"), "User"), "namespace App.Models;\n\npublic class User\n{\n}\n");
        assert_eq!(CSharpItem::Record.render(None, "Point"), "public record Point;\n");
        assert_eq!(type_name("My Service.cs"), "My_Service");
        assert_eq!(type_name("1st"), "_1st");
    }

    #[test]
    fn namespace_follows_project_and_folders() {
        let dir = std::env::temp_dir().join(format!("bf-ns-{}", std::process::id()));
        let models = dir.join("Data").join("Models");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(dir.join("Shop.Api.csproj"), "<Project><PropertyGroup></PropertyGroup></Project>").unwrap();
        assert_eq!(namespace_for(&models).as_deref(), Some("Shop.Api.Data.Models"));
        std::fs::write(dir.join("Shop.Api.csproj"), "<Project><PropertyGroup><RootNamespace>Shop</RootNamespace></PropertyGroup></Project>").unwrap();
        assert_eq!(namespace_for(&models).as_deref(), Some("Shop.Data.Models"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
