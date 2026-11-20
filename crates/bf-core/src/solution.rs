//! The solution model for the Solution view: the projects of a `.sln`/`.slnx` (with their solution
//! folders), and for each project its target frameworks, package references and project references.
//! Without a solution file, every `.csproj` in the folder is listed.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// The type GUID `.sln` files give solution folders.
const SLN_FOLDER: &str = "2150E333-8FDC-42A3-9474-1A3956D46DE8";

#[derive(Clone, Debug, Default)]
pub struct Solution {
    /// The `.sln`/`.slnx` file, or `None` when the projects were found by scanning the folder.
    pub path: Option<PathBuf>,
    pub name: String,
    pub root: Folder,
}

#[derive(Clone, Debug, Default)]
pub struct Folder {
    pub name: String,
    pub folders: Vec<Folder>,
    pub projects: Vec<Project>,
}

#[derive(Clone, Debug, Default)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    /// False when the solution lists a project file that isn't there.
    pub exists: bool,
    pub sdk: String,
    pub frameworks: Vec<String>,
    pub output_type: Option<String>,
    pub is_app: bool,
    pub is_test: bool,
    pub packages: Vec<PackageRef>,
    pub references: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageRef {
    pub name: String,
    /// From the project, or from `Directory.Packages.props` (central package management).
    pub version: Option<String>,
}

impl Folder {
    pub fn project_count(&self) -> usize {
        self.projects.len() + self.folders.iter().map(Folder::project_count).sum::<usize>()
    }

    /// Every project in this folder and below.
    pub fn all_projects(&self) -> Vec<&Project> {
        let mut out: Vec<&Project> = self.projects.iter().collect();
        for f in &self.folders {
            out.extend(f.all_projects());
        }
        out
    }
}

impl Project {
    /// "net9.0 · Web", "net8.0;net9.0 · Library", …
    pub fn summary(&self) -> String {
        let kind = if !self.exists {
            "missing"
        } else if self.is_test {
            "Tests"
        } else if self.sdk.eq_ignore_ascii_case("Microsoft.NET.Sdk.Web") {
            "Web"
        } else if self.sdk.eq_ignore_ascii_case("Microsoft.NET.Sdk.Worker") {
            "Worker"
        } else if self.sdk.to_lowercase().starts_with("aspire.apphost.sdk") {
            "Aspire"
        } else if self.is_app {
            "App"
        } else {
            "Library"
        };
        if self.frameworks.is_empty() { kind.to_owned() } else { format!("{} · {kind}", self.frameworks.join(";")) }
    }
}

/// Loads the solution in `root`: `preferred` (a configured `dotnet.project` that is a solution), else the
/// first `.slnx`/`.sln` in the folder, else every project found below it.
pub fn load(root: &Path, preferred: Option<&str>) -> Solution {
    let configured = preferred.filter(|p| p.ends_with(".sln") || p.ends_with(".slnx")).map(|p| root.join(p)).filter(|p| p.is_file());
    let found = configured.or_else(|| {
        let mut files: Vec<PathBuf> = std::fs::read_dir(root)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("sln" | "slnx")))
            .collect();
        // .slnx is the newer format; when both exist it's the one kept up to date.
        files.sort_by_key(|p| (p.extension().is_none_or(|e| e != "slnx"), p.clone()));
        files.into_iter().next()
    });
    let central = central_versions(root);
    if let Some(path) = found {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let dir = path.parent().unwrap_or(root);
        let mut folder = if path.extension().is_some_and(|e| e == "slnx") { parse_slnx(&text, dir) } else { parse_sln(&text, dir) };
        fill(&mut folder, &central);
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        return Solution { path: Some(path), name, root: folder };
    }
    let mut folder = Folder { projects: crate::dotnet::projects(root).into_iter().map(|p| project_stub(&p)).collect(), ..Default::default() };
    fill(&mut folder, &central);
    let name = root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Solution { path: None, name, root: folder }
}

fn fill(folder: &mut Folder, central: &HashMap<String, String>) {
    for project in &mut folder.projects {
        read_project(project, central);
    }
    for sub in &mut folder.folders {
        fill(sub, central);
    }
}

fn project_stub(path: &Path) -> Project {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Project { name, path: path.to_path_buf(), ..Default::default() }
}

/// Parses a classic `.sln`: `Project("{type}") = "Name", "path", "{guid}"` lines, with solution folders
/// nested through the `NestedProjects` section.
pub fn parse_sln(text: &str, dir: &Path) -> Folder {
    struct Item {
        guid: String,
        name: String,
        project: Option<PathBuf>,
    }
    let mut items: Vec<Item> = Vec::new();
    let mut parents: HashMap<String, String> = HashMap::new();
    let mut in_nested = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Project(") {
            let quoted: Vec<&str> = rest.split('"').skip(1).step_by(2).collect();
            // [type guid, name, path, project guid]
            if quoted.len() < 4 {
                continue;
            }
            let kind = quoted[0].trim_matches(['{', '}']).to_uppercase();
            let guid = quoted[3].trim_matches(['{', '}']).to_uppercase();
            let project = (kind != SLN_FOLDER && quoted[2].ends_with("proj")).then(|| resolve(dir, quoted[2]));
            if kind != SLN_FOLDER && project.is_none() {
                continue; // website folders, shared projects without a project file, …
            }
            items.push(Item { guid, name: quoted[1].to_owned(), project });
        } else if line.starts_with("GlobalSection(NestedProjects)") {
            in_nested = true;
        } else if line.starts_with("EndGlobalSection") {
            in_nested = false;
        } else if in_nested && let Some((child, parent)) = line.split_once('=') {
            parents.insert(child.trim().trim_matches(['{', '}']).to_uppercase(), parent.trim().trim_matches(['{', '}']).to_uppercase());
        }
    }

    fn build(parent: Option<&str>, items: &[Item], parents: &HashMap<String, String>, depth: usize) -> Folder {
        let mut folder = Folder::default();
        if depth > 32 {
            return folder; // a cycle in a hand-edited file
        }
        // A parent that isn't in the file puts the item at the top.
        let parent_of = |i: &Item| parents.get(&i.guid).filter(|p| items.iter().any(|o| &o.guid == *p)).map(String::as_str);
        for item in items.iter().filter(|i| parent_of(i) == parent) {
            match &item.project {
                Some(path) => folder.projects.push(Project { name: item.name.clone(), ..project_stub(path) }),
                None => {
                    let mut sub = build(Some(&item.guid), items, parents, depth + 1);
                    sub.name = item.name.clone();
                    folder.folders.push(sub);
                }
            }
        }
        sort(&mut folder);
        folder
    }
    build(None, &items, &parents, 0)
}

/// Parses an XML `.slnx`: `<Folder Name="/src/tests/">` elements (the name is the full path) holding
/// `<Project Path="…"/>` elements.
pub fn parse_slnx(text: &str, dir: &Path) -> Folder {
    let mut root = Folder::default();
    let Ok(doc) = roxmltree::Document::parse(text) else { return root };
    for node in doc.descendants().filter(|n| n.has_tag_name("Project")) {
        let Some(rel) = node.attribute("Path") else { continue };
        let folder_path = node.ancestors().find(|n| n.has_tag_name("Folder")).and_then(|f| f.attribute("Name")).unwrap_or("/");
        let mut folder = &mut root;
        for part in folder_path.split('/').filter(|p| !p.is_empty()) {
            let ix = match folder.folders.iter().position(|f| f.name == part) {
                Some(ix) => ix,
                None => {
                    folder.folders.push(Folder { name: part.to_owned(), ..Default::default() });
                    folder.folders.len() - 1
                }
            };
            folder = &mut folder.folders[ix];
        }
        folder.projects.push(project_stub(&resolve(dir, rel)));
    }
    // Empty folders are listed too.
    for node in doc.descendants().filter(|n| n.has_tag_name("Folder")) {
        let mut folder = &mut root;
        for part in node.attribute("Name").unwrap_or_default().split('/').filter(|p| !p.is_empty()) {
            let ix = match folder.folders.iter().position(|f| f.name == part) {
                Some(ix) => ix,
                None => {
                    folder.folders.push(Folder { name: part.to_owned(), ..Default::default() });
                    folder.folders.len() - 1
                }
            };
            folder = &mut folder.folders[ix];
        }
    }
    sort_deep(&mut root);
    root
}

fn sort(folder: &mut Folder) {
    folder.folders.sort_by_key(|f| f.name.to_lowercase());
    folder.projects.sort_by_key(|p| p.name.to_lowercase());
}

fn sort_deep(folder: &mut Folder) {
    sort(folder);
    folder.folders.iter_mut().for_each(sort_deep);
}

/// Reads a project file into `project` (SDK, frameworks, output type, packages, project references).
fn read_project(project: &mut Project, central: &HashMap<String, String>) {
    let Ok(text) = std::fs::read_to_string(&project.path) else { return };
    project.exists = true;
    let dir = project.path.parent().unwrap_or(Path::new("")).to_path_buf();
    parse_project(project, &text, &dir, central);
}

pub fn parse_project(project: &mut Project, text: &str, dir: &Path, central: &HashMap<String, String>) {
    let Ok(doc) = roxmltree::Document::parse(text) else { return };
    let root = doc.root_element();
    project.sdk = root
        .attribute("Sdk")
        .or_else(|| root.children().find(|n| n.has_tag_name("Sdk")).and_then(|n| n.attribute("Name")))
        .unwrap_or_default()
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let text_of = |tag: &str| doc.descendants().find(|n| n.has_tag_name(tag)).and_then(|n| n.text()).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    project.frameworks = text_of("TargetFrameworks")
        .or_else(|| text_of("TargetFramework"))
        .map(|t| t.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default();
    project.output_type = text_of("OutputType");
    let sdk = project.sdk.to_lowercase();
    project.is_app = project.output_type.as_deref().is_some_and(|o| o.eq_ignore_ascii_case("exe") || o.eq_ignore_ascii_case("winexe"))
        || sdk == "microsoft.net.sdk.web"
        || sdk == "microsoft.net.sdk.worker";
    project.is_test = crate::testing::is_test_project_text(text);
    project.packages.clear();
    project.references.clear();
    for node in doc.descendants() {
        if node.has_tag_name("PackageReference") {
            let Some(name) = node.attribute("Include").or_else(|| node.attribute("Update")) else { continue };
            if node.attribute("Include").is_none() {
                continue; // `Update` changes a reference made elsewhere
            }
            let version = node
                .attribute("Version")
                .map(str::to_owned)
                .or_else(|| node.children().find(|n| n.has_tag_name("Version")).and_then(|n| n.text()).map(|t| t.trim().to_owned()))
                .or_else(|| node.attribute("VersionOverride").map(str::to_owned))
                .or_else(|| central.get(&name.to_lowercase()).cloned());
            project.packages.push(PackageRef { name: name.to_owned(), version });
        } else if node.has_tag_name("ProjectReference") && let Some(include) = node.attribute("Include") {
            project.references.push(resolve(dir, include));
        }
    }
    project.packages.sort_by_key(|p| p.name.to_lowercase());
}

/// Package versions from `Directory.Packages.props` in the folder (central package management).
fn central_versions(root: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(root.join("Directory.Packages.props")) else { return HashMap::new() };
    parse_central_versions(&text)
}

pub fn parse_central_versions(text: &str) -> HashMap<String, String> {
    let Ok(doc) = roxmltree::Document::parse(text) else { return HashMap::new() };
    doc.descendants()
        .filter(|n| n.has_tag_name("PackageVersion"))
        .filter_map(|n| Some((n.attribute("Include")?.to_lowercase(), n.attribute("Version")?.to_owned())))
        .collect()
}

/// Joins a solution- or project-relative path (Windows separators allowed) and removes `.`/`..`.
fn resolve(dir: &Path, rel: &str) -> PathBuf {
    let joined = dir.join(rel.replace('\\', "/"));
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLN: &str = r#"
Microsoft Visual Studio Solution File, Format Version 12.00
Project("{2150E333-8FDC-42A3-9474-1A3956D46DE8}") = "src", "src", "{11111111-1111-1111-1111-111111111111}"
EndProject
Project("{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}") = "App", "src\App\App.csproj", "{22222222-2222-2222-2222-222222222222}"
EndProject
Project("{9A19103F-16F7-4668-BE54-9A1E7A4F7556}") = "App.Tests", "tests\App.Tests\App.Tests.csproj", "{33333333-3333-3333-3333-333333333333}"
EndProject
Global
	GlobalSection(NestedProjects) = preSolution
		{22222222-2222-2222-2222-222222222222} = {11111111-1111-1111-1111-111111111111}
	EndGlobalSection
EndGlobal
"#;

    #[test]
    fn parses_sln_with_folders() {
        let f = parse_sln(SLN, Path::new("/w"));
        assert_eq!(f.projects.len(), 1);
        assert_eq!(f.projects[0].name, "App.Tests");
        assert_eq!(f.projects[0].path, PathBuf::from("/w/tests/App.Tests/App.Tests.csproj"));
        assert_eq!(f.folders.len(), 1);
        assert_eq!(f.folders[0].name, "src");
        assert_eq!(f.folders[0].projects[0].path, PathBuf::from("/w/src/App/App.csproj"));
        assert_eq!(f.project_count(), 2);
    }

    #[test]
    fn parses_slnx_with_nested_folders() {
        let xml = r#"<Solution>
  <Folder Name="/src/" />
  <Folder Name="/src/libs/"><Project Path="src/libs/Core/Core.csproj" /></Folder>
  <Folder Name="/docs/" />
  <Project Path="App/App.csproj" />
</Solution>"#;
        let f = parse_slnx(xml, Path::new("/w"));
        assert_eq!(f.projects[0].name, "App");
        assert_eq!(f.folders.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["docs", "src"]);
        let libs = &f.folders[1].folders[0];
        assert_eq!(libs.name, "libs");
        assert_eq!(libs.projects[0].path, PathBuf::from("/w/src/libs/Core/Core.csproj"));
    }

    #[test]
    fn reads_project_details() {
        let csproj = r#"<Project Sdk="Microsoft.NET.Sdk.Web">
  <PropertyGroup><TargetFramework>net9.0</TargetFramework></PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Serilog" Version="4.0.0" />
    <PackageReference Include="Dapper"><Version>2.1.35</Version></PackageReference>
    <PackageReference Include="Polly" />
    <PackageReference Update="Ignored" Version="1.0" />
    <ProjectReference Include="..\Core\Core.csproj" />
  </ItemGroup>
</Project>"#;
        let central = parse_central_versions(r#"<Project><ItemGroup><PackageVersion Include="Polly" Version="8.4.1" /></ItemGroup></Project>"#);
        let mut p = Project::default();
        parse_project(&mut p, csproj, Path::new("/w/src/Api"), &central);
        p.exists = true;
        assert_eq!(p.sdk, "Microsoft.NET.Sdk.Web");
        assert_eq!(p.frameworks, ["net9.0"]);
        assert!(p.is_app && !p.is_test);
        assert_eq!(p.summary(), "net9.0 · Web");
        let names: Vec<_> = p.packages.iter().map(|p| (p.name.as_str(), p.version.as_deref())).collect();
        assert_eq!(names, [("Dapper", Some("2.1.35")), ("Polly", Some("8.4.1")), ("Serilog", Some("4.0.0"))]);
        assert_eq!(p.references, [PathBuf::from("/w/src/Core/Core.csproj")]);
    }

    #[test]
    fn loads_folder_without_solution() {
        let dir = std::env::temp_dir().join(format!("bf-solution-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Lib")).unwrap();
        std::fs::write(dir.join("Lib/Lib.csproj"), r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFrameworks>net8.0;net9.0</TargetFrameworks></PropertyGroup></Project>"#).unwrap();
        let s = load(&dir, None);
        assert!(s.path.is_none());
        assert_eq!(s.root.projects.len(), 1);
        assert_eq!(s.root.projects[0].summary(), "net8.0;net9.0 · Library");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
