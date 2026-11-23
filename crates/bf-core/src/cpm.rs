//! Central Package Management: package versions in `Directory.Packages.props` (`<PackageVersion>`),
//! projects' `<PackageReference>`s without a version of their own. What the solution has (which
//! package at which version, used by which projects; references that still carry a `Version` or
//! `VersionOverride`), moving a version into the props file, and turning it on for a solution. Edits
//! change only the elements and attributes they're about.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const PROPS: &str = "Directory.Packages.props";

fn parse(text: &str) -> anyhow::Result<roxmltree::Document<'_>> {
    Ok(roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })?)
}

/// A `<PackageReference Include=…>` of a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub id: String,
    /// Its own `Version` (attribute or element).
    pub version: Option<String>,
    pub version_override: Option<String>,
}

pub fn references(project_text: &str) -> Vec<Reference> {
    let Ok(doc) = parse(project_text) else { return Vec::new() };
    doc.descendants()
        .filter(|n| n.has_tag_name("PackageReference"))
        .filter_map(|n| {
            Some(Reference {
                id: n.attribute("Include")?.to_owned(),
                version: n.attribute("Version").map(str::to_owned).or_else(|| n.children().find(|c| c.has_tag_name("Version")).and_then(|c| c.text()).map(|t| t.trim().to_owned())),
                version_override: n.attribute("VersionOverride").map(str::to_owned),
            })
        })
        .collect()
}

/// A `<PackageVersion>` of the props file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Central {
    pub id: String,
    pub version: String,
    /// 0-based line in the props file.
    pub line: usize,
}

pub fn central(props_text: &str) -> Vec<Central> {
    let Ok(doc) = parse(props_text) else { return Vec::new() };
    doc.descendants()
        .filter(|n| n.has_tag_name("PackageVersion"))
        .filter_map(|n| Some(Central { id: n.attribute("Include")?.to_owned(), version: n.attribute("Version")?.to_owned(), line: props_text[..n.range().start].matches('\n').count() }))
        .collect()
}

/// Whether the props file turns central management on.
pub fn enabled(props_text: &str) -> bool {
    parse(props_text).is_ok_and(|doc| doc.descendants().any(|n| n.has_tag_name("ManagePackageVersionsCentrally") && n.text().is_some_and(|t| t.trim().eq_ignore_ascii_case("true"))))
}

/// The props file that applies to `dir`: the nearest one in it or a folder above (as MSBuild finds
/// it), else where a new one goes (in `dir`).
pub fn props_for(dir: &Path) -> PathBuf {
    dir.ancestors().map(|d| d.join(PROPS)).find(|p| p.is_file()).unwrap_or_else(|| dir.join(PROPS))
}

/// The solution's packages, seen from central management.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overview {
    pub props: PathBuf,
    pub exists: bool,
    pub enabled: bool,
    /// Each central package and the projects that reference it.
    pub central: Vec<(Central, Vec<PathBuf>)>,
    /// References with a version of their own: (project, reference).
    pub own: Vec<(PathBuf, Reference)>,
}

/// What `projects` (path, text) and the props file at `props` (its text, if it exists) add up to.
pub fn overview(props: &Path, props_text: Option<&str>, projects: &[(PathBuf, String)]) -> Overview {
    let central_list = props_text.map(central).unwrap_or_default();
    let refs: Vec<(PathBuf, Vec<Reference>)> = projects.iter().map(|(p, t)| (p.clone(), references(t))).collect();
    let central = central_list
        .into_iter()
        .map(|c| {
            let users = refs.iter().filter(|(_, r)| r.iter().any(|r| r.id.eq_ignore_ascii_case(&c.id))).map(|(p, _)| p.clone()).collect();
            (c, users)
        })
        .collect();
    let own = refs.iter().flat_map(|(p, r)| r.iter().filter(|r| r.version.is_some() || r.version_override.is_some()).map(|r| (p.clone(), r.clone()))).collect();
    Overview { props: props.to_path_buf(), exists: props_text.is_some(), enabled: props_text.is_some_and(enabled), central, own }
}

fn eol(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

fn escape_attr(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

fn indent_at(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[start..at];
    &line[..line.len() - line.trim_start().len()]
}

/// Sets the central version of `id`, adding a `<PackageVersion>` after the last one (or in a new
/// `<ItemGroup>`) when the file has none for it.
pub fn set_central_version(props_text: &str, id: &str, version: &str) -> anyhow::Result<String> {
    let doc = parse(props_text)?;
    let nl = eol(props_text);
    let versions: Vec<roxmltree::Node> = doc.descendants().filter(|n| n.has_tag_name("PackageVersion")).collect();
    if let Some(node) = versions.iter().find(|n| n.attribute("Include").is_some_and(|i| i.eq_ignore_ascii_case(id))) {
        let attr = node.attributes().find(|a| a.name() == "Version");
        return Ok(match attr {
            Some(a) => {
                let r = a.range_value();
                format!("{}{}{}", &props_text[..r.start], escape_attr(version), &props_text[r.end..])
            }
            None => {
                // `<PackageVersion Include="x" />`: the attribute goes after Include.
                let include = node.attributes().find(|a| a.name() == "Include").map(|a| a.range().end).unwrap_or(node.range().start);
                format!("{} Version=\"{}\"{}", &props_text[..include], escape_attr(version), &props_text[include..])
            }
        });
    }
    let element = format!("<PackageVersion Include=\"{}\" Version=\"{}\" />", escape_attr(id), escape_attr(version));
    if let Some(last) = versions.last() {
        let indent = indent_at(props_text, last.range().start).to_owned();
        let at = last.range().end;
        return Ok(format!("{}{nl}{indent}{element}{}", &props_text[..at], &props_text[at..]));
    }
    // No package versions yet: a new item group at the end of the project.
    let project = doc.root_element();
    let r = project.range();
    let inner = &props_text[r.clone()];
    let group = format!("  <ItemGroup>{nl}    {element}{nl}  </ItemGroup>{nl}");
    if inner.ends_with("/>") {
        let open = inner.trim_end_matches("/>").trim_end();
        return Ok(format!("{}{open}>{nl}{group}</Project>{}", &props_text[..r.start], &props_text[r.end..]));
    }
    let close = r.start + inner.rfind("</").unwrap_or(inner.len());
    let before = props_text[..close].trim_end_matches([' ', '\t']);
    let before = if before.ends_with('\n') { before.to_owned() } else { format!("{before}{nl}") };
    Ok(format!("{before}{group}{}", &props_text[close..]))
}

/// Removes the version a project gives `id` itself (`Version` / `VersionOverride` attributes, a
/// `<Version>` element), so the central one applies.
pub fn remove_own_version(project_text: &str, id: &str) -> anyhow::Result<String> {
    let doc = parse(project_text)?;
    let mut cuts: Vec<std::ops::Range<usize>> = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("PackageReference") && n.attribute("Include").is_some_and(|i| i.eq_ignore_ascii_case(id))) {
        for a in node.attributes().filter(|a| a.name() == "Version" || a.name() == "VersionOverride") {
            let r = a.range();
            // With the whitespace before it.
            let start = project_text[..r.start].trim_end().len();
            cuts.push(start..r.end);
        }
        for child in node.children().filter(|c| c.has_tag_name("Version")) {
            let r = child.range();
            let start = project_text[..r.start].trim_end().len();
            cuts.push(start..r.end);
        }
    }
    let mut out = project_text.to_owned();
    cuts.sort_by_key(|r| std::cmp::Reverse(r.start));
    for r in cuts {
        out.replace_range(r, "");
    }
    // `<PackageReference Include="x"></PackageReference>` left by a removed element: close it short.
    Ok(collapse_empty(&out))
}

fn collapse_empty(text: &str) -> String {
    let Ok(doc) = parse(text) else { return text.to_owned() };
    let mut out = text.to_owned();
    let mut spans: Vec<std::ops::Range<usize>> = doc
        .descendants()
        .filter(|n| n.has_tag_name("PackageReference") && !n.children().any(|c| c.is_element() || c.text().is_some_and(|t| !t.trim().is_empty())) && text[n.range()].ends_with("</PackageReference>"))
        .map(|n| n.range())
        .collect();
    spans.sort_by_key(|r| std::cmp::Reverse(r.start));
    for r in spans {
        let element = &text[r.clone()];
        let open_end = element.find('>').unwrap_or(element.len());
        let short = format!("{} />", element[..open_end].trim_end());
        out.replace_range(r, &short);
    }
    out
}

/// A new props file with central management on and these versions.
pub fn new_props(packages: &[(String, String)], nl: &str) -> String {
    let mut out = format!("<Project>{nl}  <PropertyGroup>{nl}    <ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally>{nl}  </PropertyGroup>{nl}  <ItemGroup>{nl}");
    for (id, version) in packages {
        out.push_str(&format!("    <PackageVersion Include=\"{}\" Version=\"{}\" />{nl}", escape_attr(id), escape_attr(version)));
    }
    out.push_str(&format!("  </ItemGroup>{nl}</Project>{nl}"));
    out
}

/// Turns central management on in an existing props file.
pub fn turn_on(props_text: &str) -> anyhow::Result<String> {
    let doc = parse(props_text)?;
    if let Some(node) = doc.descendants().find(|n| n.has_tag_name("ManagePackageVersionsCentrally")) {
        let r = node.range();
        return Ok(format!("{}<ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally>{}", &props_text[..r.start], &props_text[r.end..]));
    }
    let nl = eol(props_text);
    let project = doc.root_element();
    let r = project.range();
    let inner = &props_text[r.clone()];
    let group = format!("{nl}  <PropertyGroup>{nl}    <ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally>{nl}  </PropertyGroup>");
    if inner.ends_with("/>") {
        let open = inner.trim_end_matches("/>").trim_end();
        return Ok(format!("{}{open}>{group}{nl}</Project>{}", &props_text[..r.start], &props_text[r.end..]));
    }
    let open_end = r.start + inner.find('>').map_or(0, |i| i + 1);
    Ok(format!("{}{group}{}", &props_text[..open_end], &props_text[open_end..]))
}

/// The files to write to turn central management on: the props file (new, or with management turned on
/// and every package's version) and each project without its own versions. Packages at different
/// versions in different projects get the highest; `notes` says which.
pub struct Plan {
    pub writes: Vec<(PathBuf, String)>,
    pub notes: Vec<String>,
}

pub fn enable(props: &Path, props_text: Option<&str>, projects: &[(PathBuf, String)]) -> anyhow::Result<Plan> {
    let mut versions: BTreeMap<String, (String, Vec<(String, PathBuf)>)> = BTreeMap::new();
    for (project, text) in projects {
        for r in references(text) {
            let Some(v) = r.version.clone() else { continue };
            let entry = versions.entry(r.id.to_lowercase()).or_insert_with(|| (r.id.clone(), Vec::new()));
            entry.1.push((v, project.clone()));
        }
    }
    let mut notes = Vec::new();
    let mut chosen: Vec<(String, String)> = Vec::new();
    for (id, uses) in versions.values() {
        let best = uses.iter().map(|(v, _)| v.clone()).max_by(|a, b| crate::nuget::compare_versions(a, b)).unwrap_or_default();
        if uses.iter().any(|(v, _)| *v != best) {
            let lower: Vec<String> = uses.iter().filter(|(v, _)| *v != best).map(|(v, p)| format!("{} had {v}", p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())).collect();
            notes.push(format!("{id}: {best} for every project ({})", lower.join(", ")));
        }
        chosen.push((id.clone(), best));
    }
    chosen.sort_by_key(|(id, _)| id.to_lowercase());
    let mut writes = Vec::new();
    let props_out = match props_text {
        None => new_props(&chosen, projects.first().map_or("\n", |(_, t)| eol(t))),
        Some(text) => {
            let mut text = if enabled(text) { text.to_owned() } else { turn_on(text)? };
            let existing = central(&text);
            for (id, version) in &chosen {
                // A version the file has already stays.
                if !existing.iter().any(|c| c.id.eq_ignore_ascii_case(id)) {
                    text = set_central_version(&text, id, version)?;
                }
            }
            text
        }
    };
    writes.push((props.to_path_buf(), props_out));
    for (project, text) in projects {
        let mut out = text.clone();
        for r in references(text).iter().filter(|r| r.version.is_some()) {
            out = remove_own_version(&out, &r.id)?;
        }
        if out != *text {
            writes.push((project.clone(), out));
        }
    }
    Ok(Plan { writes, notes })
}

/// "Move to Directory.Packages.props" for one reference: the props file gets the project's version
/// when it has none for the package (a version it has wins), and the project loses its own.
pub fn move_to_props(props_text: Option<&str>, project_text: &str, id: &str) -> anyhow::Result<(String, String)> {
    let own = references(project_text).into_iter().find(|r| r.id.eq_ignore_ascii_case(id)).and_then(|r| r.version_override.or(r.version));
    let props = match (props_text, own) {
        (Some(text), Some(v)) if !central(text).iter().any(|c| c.id.eq_ignore_ascii_case(id)) => set_central_version(text, id, &v)?,
        (Some(text), _) => text.to_owned(),
        (None, Some(v)) => new_props(&[(id.to_owned(), v)], eol(project_text)),
        (None, None) => anyhow::bail!("{id} has no version to move"),
    };
    Ok((props, remove_own_version(project_text, id)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const API: &str = r#"<Project Sdk="Microsoft.NET.Sdk.Web">
  <ItemGroup>
    <PackageReference Include="Serilog" Version="4.0.0" />
    <PackageReference Include="Dapper">
      <Version>2.1.35</Version>
    </PackageReference>
    <PackageReference Include="Polly" />
  </ItemGroup>
</Project>
"#;
    const WORKER: &str = r#"<Project Sdk="Microsoft.NET.Sdk.Worker">
  <ItemGroup>
    <PackageReference Include="Serilog" Version="3.1.1" PrivateAssets="all" />
  </ItemGroup>
</Project>
"#;
    const PROPS_TEXT: &str = r#"<Project>
  <PropertyGroup>
    <ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally>
  </PropertyGroup>
  <ItemGroup>
    <PackageVersion Include="Polly" Version="8.4.0" />
  </ItemGroup>
</Project>
"#;

    fn projects() -> Vec<(PathBuf, String)> {
        vec![(PathBuf::from("/s/Api/Api.csproj"), API.to_owned()), (PathBuf::from("/s/Worker/Worker.csproj"), WORKER.to_owned())]
    }

    #[test]
    fn reads_references_and_central_versions() {
        let r = references(API);
        assert_eq!(r.iter().map(|r| (r.id.as_str(), r.version.as_deref())).collect::<Vec<_>>(), [("Serilog", Some("4.0.0")), ("Dapper", Some("2.1.35")), ("Polly", None)]);
        assert_eq!(central(PROPS_TEXT), [Central { id: "Polly".into(), version: "8.4.0".into(), line: 5 }]);
        assert!(enabled(PROPS_TEXT));
        assert!(!enabled("<Project />"));
    }

    #[test]
    fn overview_lists_users_and_own_versions() {
        let o = overview(Path::new("/s/Directory.Packages.props"), Some(PROPS_TEXT), &projects());
        assert!(o.exists && o.enabled);
        assert_eq!(o.central.len(), 1);
        assert_eq!(o.central[0].1, [PathBuf::from("/s/Api/Api.csproj")]);
        assert_eq!(o.own.iter().map(|(p, r)| (p.file_stem().unwrap().to_str().unwrap(), r.id.as_str())).collect::<Vec<_>>(), [("Api", "Serilog"), ("Api", "Dapper"), ("Worker", "Serilog")]);
    }

    #[test]
    fn removes_a_projects_own_version() {
        let out = remove_own_version(API, "Serilog").unwrap();
        assert_eq!(out, API.replace(r#"<PackageReference Include="Serilog" Version="4.0.0" />"#, r#"<PackageReference Include="Serilog" />"#));
        let out = remove_own_version(API, "Dapper").unwrap();
        assert!(out.contains("    <PackageReference Include=\"Dapper\" />\n    <PackageReference Include=\"Polly\" />"), "{out}");
        let out = remove_own_version(WORKER, "Serilog").unwrap();
        assert!(out.contains(r#"<PackageReference Include="Serilog" PrivateAssets="all" />"#), "{out}");
        let o = "<Project><ItemGroup><PackageReference Include=\"A\" VersionOverride=\"1.0\" /></ItemGroup></Project>";
        assert_eq!(remove_own_version(o, "a").unwrap(), "<Project><ItemGroup><PackageReference Include=\"A\" /></ItemGroup></Project>");
    }

    #[test]
    fn sets_central_versions() {
        let out = set_central_version(PROPS_TEXT, "polly", "8.5.0").unwrap();
        assert_eq!(out, PROPS_TEXT.replace("8.4.0", "8.5.0"));
        let out = set_central_version(PROPS_TEXT, "Serilog", "4.0.0").unwrap();
        assert!(out.contains("    <PackageVersion Include=\"Polly\" Version=\"8.4.0\" />\n    <PackageVersion Include=\"Serilog\" Version=\"4.0.0\" />\n  </ItemGroup>"), "{out}");
        let out = set_central_version("<Project>\n</Project>\n", "A", "1.0").unwrap();
        assert_eq!(central(&out)[0].version, "1.0");
        let out = set_central_version("<Project />", "A", "1.0").unwrap();
        assert_eq!(central(&out)[0].id, "A");
    }

    #[test]
    fn moves_a_version_into_the_props_file() {
        let (props, api) = move_to_props(Some(PROPS_TEXT), API, "Serilog").unwrap();
        assert_eq!(central(&props).iter().map(|c| (c.id.as_str(), c.version.as_str())).collect::<Vec<_>>(), [("Polly", "8.4.0"), ("Serilog", "4.0.0")]);
        assert!(references(&api).iter().all(|r| r.id != "Serilog" || r.version.is_none()));
        // The props file's version wins over the project's.
        let with_polly = API.replace(r#"<PackageReference Include="Polly" />"#, r#"<PackageReference Include="Polly" Version="7.0.0" />"#);
        let (props, _) = move_to_props(Some(PROPS_TEXT), &with_polly, "Polly").unwrap();
        assert_eq!(props, PROPS_TEXT);
        let (props, _) = move_to_props(None, API, "Dapper").unwrap();
        assert!(enabled(&props) && central(&props)[0].version == "2.1.35");
    }

    #[test]
    fn enabling_writes_props_and_strips_projects() {
        let plan = enable(Path::new("/s/Directory.Packages.props"), None, &projects()).unwrap();
        let (path, props) = &plan.writes[0];
        assert_eq!(path, Path::new("/s/Directory.Packages.props"));
        assert!(enabled(props));
        assert_eq!(central(props).iter().map(|c| (c.id.as_str(), c.version.as_str())).collect::<Vec<_>>(), [("Dapper", "2.1.35"), ("Serilog", "4.0.0")]);
        assert_eq!(plan.notes, ["Serilog: 4.0.0 for every project (Worker had 3.1.1)"]);
        assert_eq!(plan.writes.len(), 3);
        for (_, text) in &plan.writes[1..] {
            assert!(references(text).iter().all(|r| r.version.is_none()), "{text}");
        }
        // An existing props file without the switch gets it, and keeps its versions.
        let off = "<Project>\n  <ItemGroup>\n    <PackageVersion Include=\"Serilog\" Version=\"4.1.0\" />\n  </ItemGroup>\n</Project>\n";
        let plan = enable(Path::new("/s/Directory.Packages.props"), Some(off), &projects()).unwrap();
        let props = &plan.writes[0].1;
        assert!(enabled(props), "{props}");
        assert_eq!(central(props).iter().map(|c| (c.id.as_str(), c.version.as_str())).collect::<Vec<_>>(), [("Serilog", "4.1.0"), ("Dapper", "2.1.35")]);
    }
}
