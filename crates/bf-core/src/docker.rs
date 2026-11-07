//! Docker integration through the `docker` CLI (so contexts, credentials and Docker Desktop or Podman's
//! `docker` shim all apply): listing and managing containers and images, finding Dockerfiles and Compose
//! files in the workspace, and generating a Dockerfile, `.dockerignore` and Compose file for .NET projects
//! (like Visual Studio's "Add Docker Support").

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`, `paused`, `created`, `restarting`, `removing` or `dead`.
    pub state: String,
    /// Human-readable status, e.g. "Up 3 minutes" or "Exited (0) 2 hours ago".
    pub status: String,
    pub ports: String,
    /// The Compose project and service the container belongs to, if any.
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
}

impl Container {
    pub fn is_running(&self) -> bool {
        matches!(self.state.as_str(), "running" | "restarting")
    }

    pub fn is_paused(&self) -> bool {
        self.state == "paused"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub id: String,
    pub repository: String,
    pub tag: String,
    pub size: String,
    pub created: String,
}

impl Image {
    /// `repository:tag`, or the id for dangling images (`<none>`).
    pub fn reference(&self) -> String {
        if self.repository == "<none>" {
            self.id.clone()
        } else if self.tag == "<none>" || self.tag.is_empty() {
            self.repository.clone()
        } else {
            format!("{}:{}", self.repository, self.tag)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub containers: Vec<Container>,
    pub images: Vec<Image>,
}

fn docker() -> Command {
    let mut command = Command::new("docker");
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// Runs `docker <args>` and returns stdout, or stderr as the error.
pub fn run(args: &[&str]) -> anyhow::Result<String> {
    let output = docker().args(args).output().map_err(|e| anyhow::anyhow!("could not run docker (is it installed?): {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = if stderr.trim().is_empty() { stdout.trim().to_owned() } else { stderr.trim().to_owned() };
        anyhow::bail!("{message}")
    }
}

/// Whether the `docker` CLI is installed (the daemon may still be down).
pub fn cli_available() -> bool {
    docker().arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Containers (all, not only running ones) and images. Fails when the daemon can't be reached.
pub fn snapshot() -> anyhow::Result<Snapshot> {
    let containers = parse_containers(&run(&["ps", "--all", "--no-trunc", "--format", "{{json .}}"])?);
    let images = parse_images(&run(&["images", "--format", "{{json .}}"])?);
    Ok(Snapshot { containers, images })
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawContainer {
    #[serde(rename = "ID")]
    id: String,
    names: String,
    image: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    ports: String,
    #[serde(default)]
    labels: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawImage {
    #[serde(rename = "ID")]
    id: String,
    repository: String,
    tag: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    created_since: String,
}

/// Parses `docker ps --format '{{json .}}'` (one object per line), sorted by Compose project, then name.
pub fn parse_containers(text: &str) -> Vec<Container> {
    let mut containers: Vec<Container> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<RawContainer>(line.trim()).ok())
        .map(|c| {
            let label = |key: &str| {
                c.labels.split(',').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('=')).map(str::to_owned).filter(|v| !v.is_empty())
            };
            let state = if c.state.is_empty() { state_from_status(&c.status).to_owned() } else { c.state.to_ascii_lowercase() };
            Container {
                id: c.id.chars().take(12).collect(),
                name: c.names.split(',').next().unwrap_or_default().to_owned(),
                image: c.image,
                state,
                status: c.status,
                ports: c.ports,
                compose_project: label("com.docker.compose.project"),
                compose_service: label("com.docker.compose.service"),
            }
        })
        .collect();
    containers.sort_by(|a, b| (a.compose_project.is_none(), &a.compose_project, &a.name).cmp(&(b.compose_project.is_none(), &b.compose_project, &b.name)));
    containers
}

/// Older engines don't report `State`; derive it from the status text.
fn state_from_status(status: &str) -> &'static str {
    let s = status.to_ascii_lowercase();
    if s.contains("(paused)") {
        "paused"
    } else if s.starts_with("up") {
        "running"
    } else if s.starts_with("restarting") {
        "restarting"
    } else if s.starts_with("created") {
        "created"
    } else {
        "exited"
    }
}

/// Parses `docker images --format '{{json .}}'`.
pub fn parse_images(text: &str) -> Vec<Image> {
    let mut images: Vec<Image> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<RawImage>(line.trim()).ok())
        .map(|i| Image {
            id: i.id.trim_start_matches("sha256:").chars().take(12).collect(),
            repository: i.repository,
            tag: i.tag,
            size: i.size,
            created: i.created_since,
        })
        .collect();
    images.sort_by(|a, b| (a.repository == "<none>", &a.repository, &a.tag).cmp(&(b.repository == "<none>", &b.repository, &b.tag)));
    images
}

pub fn start(id: &str) -> anyhow::Result<String> {
    run(&["start", id])
}

pub fn stop(id: &str) -> anyhow::Result<String> {
    run(&["stop", id])
}

pub fn restart(id: &str) -> anyhow::Result<String> {
    run(&["restart", id])
}

pub fn unpause(id: &str) -> anyhow::Result<String> {
    run(&["unpause", id])
}

/// Removes a container, stopping it first if it runs.
pub fn remove_container(id: &str) -> anyhow::Result<String> {
    run(&["rm", "--force", id])
}

pub fn remove_image(reference: &str) -> anyhow::Result<String> {
    run(&["rmi", reference])
}

/// Removes stopped containers, dangling images and unused networks (`docker system prune`, no volumes).
pub fn prune() -> anyhow::Result<String> {
    run(&["system", "prune", "--force"])
}

// ───────────── workspace files ─────────────

/// `compose.yaml`, `compose.yml`, `docker-compose.yml`, `docker-compose.yaml` and their variants
/// (`docker-compose.override.yml`, `compose.prod.yaml`, …).
pub fn is_compose_file_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let Some(stem) = name.strip_suffix(".yml").or_else(|| name.strip_suffix(".yaml")) else { return false };
    ["compose", "docker-compose"].iter().any(|base| stem == *base || stem.strip_prefix(base).is_some_and(|rest| rest.starts_with('.') && rest.len() > 1))
}

/// Splits workspace files into Dockerfiles and Compose files.
pub fn docker_files(files: &[PathBuf], is_dockerfile: impl Fn(&str) -> bool) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dockerfiles = files.iter().filter(|p| is_dockerfile(&name(p))).cloned().collect();
    let compose = files.iter().filter(|p| is_compose_file_name(&name(p))).cloned().collect();
    (dockerfiles, compose)
}

/// The build context for a Dockerfile: its folder, unless it sits next to a .NET project in a workspace
/// with a solution at `root` and no `.dockerignore` of its own. Visual Studio's Dockerfiles build from the
/// solution folder (they copy `Project/Project.csproj`).
pub fn build_context(dockerfile: &Path, root: &Path) -> PathBuf {
    let Some(dir) = dockerfile.parent() else { return root.to_path_buf() };
    if dir == root || dir.join(".dockerignore").exists() {
        return dir.to_path_buf();
    }
    let has = |d: &Path, exts: &[&str]| {
        std::fs::read_dir(d).is_ok_and(|entries| {
            entries.flatten().any(|e| e.path().extension().and_then(|x| x.to_str()).is_some_and(|x| exts.contains(&x)))
        })
    };
    let solution_root = has(root, &["sln", "slnx"]) || root.join(".dockerignore").exists();
    if dir.starts_with(root) && has(dir, &["csproj", "fsproj", "vbproj"]) && solution_root {
        return root.to_path_buf();
    }
    dir.to_path_buf()
}

/// An image name from a folder or project name: lowercase, with only the characters Docker allows.
pub fn image_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        let separator_ok = !out.is_empty() && !out.ends_with(['.', '_', '-']);
        if c.is_ascii_alphanumeric() || (matches!(c, '.' | '_' | '-') && separator_ok) {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with(['.', '_', '-']) {
            out.push('-');
        }
    }
    let out = out.trim_end_matches(['.', '_', '-']).to_owned();
    if out.is_empty() { "app".into() } else { out }
}

/// The image name for a Dockerfile: its folder's name (`Api/Dockerfile` → `api`), or the variant
/// (`Dockerfile.worker` → `<folder>-worker`).
pub fn image_name_for(dockerfile: &Path) -> String {
    let folder = dockerfile.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "app".into());
    let file = dockerfile.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let variant = file
        .strip_prefix("dockerfile.")
        .or_else(|| file.strip_prefix("containerfile."))
        .or_else(|| file.strip_suffix(".dockerfile"))
        .or_else(|| file.strip_suffix(".containerfile"));
    match variant {
        Some(v) => image_name(&format!("{folder}-{v}")),
        None => image_name(&folder),
    }
}

/// Quotes an argument for POSIX shells and fish (`'…'`, with `'` written as `'\''`).
pub fn shell_quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c)) {
        return arg.to_owned();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// A command line from arguments, quoted for the shell.
pub fn command_line(args: &[&str]) -> String {
    args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
}

// ───────────── .NET Docker support ─────────────

/// What a Dockerfile for a .NET project needs to know about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DotnetProject {
    pub path: PathBuf,
    /// The assembly the container runs (`<AssemblyName>`, else the project file name).
    pub assembly: String,
    /// The .NET version of the base images, e.g. "9.0".
    pub version: String,
    /// ASP.NET Core (`Microsoft.NET.Sdk.Web`, Razor, Blazor): the `aspnet` base image and exposed ports.
    pub web: bool,
}

/// .NET version used when a project's target framework can't be read.
pub const DEFAULT_DOTNET_VERSION: &str = "10.0";

impl DotnetProject {
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "App".into());
        let assembly = xml_value(&text, "AssemblyName").filter(|a| !a.contains('$')).unwrap_or(stem);
        let frameworks = xml_value(&text, "TargetFramework").or_else(|| xml_value(&text, "TargetFrameworks")).unwrap_or_default();
        // The highest modern `netX.Y` (OS-specific ones like net9.0-windows can't run in Linux containers).
        let version = frameworks
            .split(';')
            .filter_map(|f| {
                let v = f.trim().strip_prefix("net")?;
                let (major, minor) = v.split_once('.')?;
                let major: u32 = major.parse().ok()?;
                let minor: u32 = minor.parse().ok()?;
                (major >= 5).then_some((major, minor))
            })
            .max()
            .map_or_else(|| DEFAULT_DOTNET_VERSION.to_owned(), |(a, b)| format!("{a}.{b}"));
        let lower = text.to_ascii_lowercase();
        let web = lower.contains("microsoft.net.sdk.web") || lower.contains("microsoft.net.sdk.razor") || lower.contains("microsoft.net.sdk.blazorwebassembly");
        Ok(Self { path: path.to_path_buf(), assembly, version, web })
    }

    /// The projects this one references, transitively (`<ProjectReference Include="…">`), excluding itself.
    pub fn references(&self) -> Vec<PathBuf> {
        let mut seen: Vec<PathBuf> = vec![normalize(&self.path)];
        let mut queue = vec![self.path.clone()];
        let mut out = Vec::new();
        while let Some(project) = queue.pop() {
            let Ok(text) = std::fs::read_to_string(&project) else { continue };
            let dir = project.parent().unwrap_or(Path::new("."));
            for include in attribute_values(&text, "ProjectReference", "Include") {
                let path = normalize(&dir.join(include.replace('\\', "/")));
                if !seen.contains(&path) {
                    seen.push(path.clone());
                    out.push(path.clone());
                    queue.push(path);
                }
            }
        }
        out.sort();
        out
    }
}

/// Resolves `.` and `..` without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The text of the first `<name>…</name>` element.
fn xml_value(text: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&format!("</{name}>"))?;
    Some(text[start..end].trim().to_owned()).filter(|v| !v.is_empty())
}

/// Values of `attribute` on every `<element …>` tag.
fn attribute_values(text: &str, element: &str, attribute: &str) -> Vec<String> {
    let mut out = Vec::new();
    let open = format!("<{element}");
    let mut rest = text;
    while let Some(at) = rest.find(&open) {
        rest = &rest[at + open.len()..];
        let tag = &rest[..rest.find('>').unwrap_or(rest.len())];
        let key = format!("{attribute}=");
        if let Some(k) = tag.find(&key) {
            let value = &tag[k + key.len()..];
            if let Some(quote) = value.chars().next().filter(|c| *c == '"' || *c == '\'')
                && let Some(end) = value[1..].find(quote)
            {
                out.push(value[1..1 + end].to_owned());
            }
        }
    }
    out
}

fn relative(path: &Path, base: &Path) -> String {
    path.strip_prefix(base).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// A multi-stage Dockerfile for `project`, built with `context` as the build context (Visual Studio's
/// layout: restore with only the project files first so that layer is cached, then build and publish).
pub fn dotnet_dockerfile(project: &DotnetProject, context: &Path) -> String {
    let v = &project.version;
    let project_rel = relative(&project.path, context);
    let project_dir = Path::new(&project_rel).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let project_file = Path::new(&project_rel).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let dir_slash = |dir: &str| if dir.is_empty() { "./".to_owned() } else { format!("{dir}/") };
    let mut s = String::new();
    s.push_str("# Build:  docker build -f ");
    s.push_str(&dir_slash(&project_dir));
    s.push_str("Dockerfile -t ");
    s.push_str(&image_name(&project.assembly));
    s.push_str(" .\n# (run from the build context: the folder that contains ");
    s.push_str(if project_dir.is_empty() { "this file" } else { "the solution" });
    s.push_str(")\n\n");
    if project.web {
        s.push_str(&format!("FROM mcr.microsoft.com/dotnet/aspnet:{v} AS base\nUSER $APP_UID\nWORKDIR /app\nEXPOSE 8080\nEXPOSE 8081\n\n"));
    } else {
        s.push_str(&format!("FROM mcr.microsoft.com/dotnet/runtime:{v} AS base\nUSER $APP_UID\nWORKDIR /app\n\n"));
    }
    s.push_str(&format!("FROM mcr.microsoft.com/dotnet/sdk:{v} AS build\nARG BUILD_CONFIGURATION=Release\nWORKDIR /src\n"));
    // Restore inputs first, so the restore layer is reused until a project or package version changes.
    for shared in ["Directory.Build.props", "Directory.Build.targets", "Directory.Packages.props", "NuGet.config", "nuget.config", "global.json"] {
        if context.join(shared).is_file() {
            s.push_str(&format!("COPY [\"{shared}\", \".\"]\n"));
        }
    }
    let mut copies = vec![project.path.clone()];
    copies.extend(project.references().into_iter().filter(|r| r.starts_with(context)));
    for path in copies {
        let rel = relative(&path, context);
        let dir = Path::new(&rel).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        s.push_str(&format!("COPY [\"{rel}\", \"{}\"]\n", dir_slash(&dir)));
    }
    s.push_str(&format!("RUN dotnet restore \"./{project_rel}\"\nCOPY . .\nWORKDIR \"/src/{project_dir}\"\n"));
    s.push_str(&format!("RUN dotnet build \"./{project_file}\" -c $BUILD_CONFIGURATION -o /app/build\n\n"));
    s.push_str("FROM build AS publish\nARG BUILD_CONFIGURATION=Release\n");
    s.push_str(&format!("RUN dotnet publish \"./{project_file}\" -c $BUILD_CONFIGURATION -o /app/publish /p:UseAppHost=false\n\n"));
    s.push_str(&format!("FROM base AS final\nWORKDIR /app\nCOPY --from=publish /app/publish .\nENTRYPOINT [\"dotnet\", \"{}.dll\"]\n", project.assembly));
    s.replace("WORKDIR \"/src/\"", "WORKDIR \"/src\"")
}

/// A `.dockerignore` for .NET solutions (keeps build output, IDE files and secrets out of the context).
pub const DOTNET_DOCKERIGNORE: &str = "**/.classpath
**/.dockerignore
**/.env
**/.git
**/.gitignore
**/.project
**/.settings
**/.toolstarget
**/.vs
**/.vscode
**/.idea
**/*.*proj.user
**/*.dbmdl
**/*.jfm
**/azds.yaml
**/bin
**/charts
**/docker-compose*
**/compose.y*ml
**/Dockerfile*
**/node_modules
**/npm-debug.log
**/obj
**/secrets.dev.yaml
**/values.dev.yaml
LICENSE
README.md
!**/.gitignore
!.git/HEAD
!.git/config
!.git/packed-refs
!.git/refs/heads/**
";

/// A service of a generated Compose file.
pub struct ComposeService {
    pub name: String,
    pub dockerfile: PathBuf,
    pub context: PathBuf,
    pub web: bool,
}

/// A `compose.yaml` (in `dir`) that builds and runs `services`. Web services get host ports 8080, 8090, …
pub fn compose_file(services: &[ComposeService], dir: &Path) -> String {
    let mut s = String::from("# Start: docker compose up --build    Stop: docker compose down\nservices:\n");
    let mut port = 8080;
    for service in services {
        let mut context = relative(&service.context, dir);
        if context.is_empty() {
            context = ".".into();
        } else if !context.starts_with('.') && !context.starts_with('/') {
            context = format!("./{context}");
        }
        let dockerfile = relative(&service.dockerfile, &service.context);
        s.push_str(&format!("  {}:\n    image: {}\n    build:\n      context: {context}\n      dockerfile: {dockerfile}\n", service.name, service.name));
        if service.web {
            s.push_str(&format!("    ports:\n      - \"{port}:8080\"\n    environment:\n      - ASPNETCORE_ENVIRONMENT=Development\n"));
            port += 10;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_containers_and_images() {
        let ps = r#"{"Command":"\"dotnet Api.dll\"","CreatedAt":"2026-11-07 10:00:00 +0100 CET","ID":"0123456789abcdef0123","Image":"api","Labels":"com.docker.compose.project=shop,com.docker.compose.service=api,x=1","Names":"shop-api-1","Ports":"0.0.0.0:8080->8080/tcp","State":"running","Status":"Up 5 minutes"}
{"ID":"fedcba987654","Image":"postgres:17","Labels":"","Names":"db","Ports":"","Status":"Exited (0) 2 hours ago"}
not json"#;
        let containers = parse_containers(ps);
        assert_eq!(containers.len(), 2);
        let api = &containers[0];
        assert_eq!(api.id, "0123456789ab");
        assert_eq!(api.name, "shop-api-1");
        assert!(api.is_running());
        assert_eq!(api.compose_project.as_deref(), Some("shop"));
        assert_eq!(api.compose_service.as_deref(), Some("api"));
        let db = &containers[1];
        assert_eq!(db.state, "exited");
        assert_eq!(db.compose_project, None);

        let images = parse_images(
            r#"{"Containers":"N/A","CreatedAt":"","CreatedSince":"2 days ago","ID":"sha256:abcdef0123456789","Repository":"<none>","Tag":"<none>","Size":"220MB"}
{"CreatedSince":"3 weeks ago","ID":"1234567890ab","Repository":"mcr.microsoft.com/dotnet/aspnet","Tag":"9.0","Size":"225MB"}"#,
        );
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].reference(), "mcr.microsoft.com/dotnet/aspnet:9.0");
        assert_eq!(images[1].reference(), "abcdef012345");
    }

    #[test]
    fn names() {
        assert!(is_compose_file_name("compose.yaml"));
        assert!(is_compose_file_name("docker-compose.override.yml"));
        assert!(!is_compose_file_name("compose.json"));
        assert!(!is_compose_file_name("my-compose.yml"));
        assert_eq!(image_name("My.Shop Api"), "my.shop-api");
        assert_eq!(image_name("__"), "app");
        assert_eq!(image_name_for(Path::new("/w/Shop.Api/Dockerfile")), "shop.api");
        assert_eq!(image_name_for(Path::new("/w/Shop/Dockerfile.worker")), "shop-worker");
        assert_eq!(shell_quote("a b'c"), r"'a b'\''c'");
        assert_eq!(command_line(&["docker", "build", "-t", "api", "/x y"]), "docker build -t api '/x y'");
    }

    #[test]
    fn generates_dotnet_docker_files() {
        let dir = std::env::temp_dir().join(format!("bf-docker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |rel: &str, text: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("Shop.slnx", "<Solution />");
        write("Directory.Packages.props", "<Project />");
        write("Shop.Api/Shop.Api.csproj", r#"<Project Sdk="Microsoft.NET.Sdk.Web"><PropertyGroup><TargetFrameworks>net8.0;net9.0;net9.0-windows</TargetFrameworks></PropertyGroup><ItemGroup><ProjectReference Include="..\Shop.Core\Shop.Core.csproj" /></ItemGroup></Project>"#);
        write("Shop.Core/Shop.Core.csproj", r#"<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><ProjectReference Include="../Shop.Data/Shop.Data.csproj" /></ItemGroup></Project>"#);
        write("Shop.Data/Shop.Data.csproj", r#"<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><ProjectReference Include="..\Shop.Core\Shop.Core.csproj" /></ItemGroup></Project>"#);
        write("Tool/Tool.csproj", r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><AssemblyName>shoptool</AssemblyName></PropertyGroup></Project>"#);

        let api = DotnetProject::read(&dir.join("Shop.Api/Shop.Api.csproj")).unwrap();
        assert_eq!((api.assembly.as_str(), api.version.as_str(), api.web), ("Shop.Api", "9.0", true));
        assert_eq!(api.references(), vec![dir.join("Shop.Core/Shop.Core.csproj"), dir.join("Shop.Data/Shop.Data.csproj")]);
        let text = dotnet_dockerfile(&api, &dir);
        assert!(text.contains("FROM mcr.microsoft.com/dotnet/aspnet:9.0 AS base"), "{text}");
        assert!(text.contains("COPY [\"Directory.Packages.props\", \".\"]"), "{text}");
        assert!(text.contains("COPY [\"Shop.Api/Shop.Api.csproj\", \"Shop.Api/\"]"), "{text}");
        assert!(text.contains("COPY [\"Shop.Data/Shop.Data.csproj\", \"Shop.Data/\"]"), "{text}");
        assert!(text.contains("WORKDIR \"/src/Shop.Api\""), "{text}");
        assert!(text.contains("ENTRYPOINT [\"dotnet\", \"Shop.Api.dll\"]"), "{text}");

        let tool = DotnetProject::read(&dir.join("Tool/Tool.csproj")).unwrap();
        assert_eq!((tool.assembly.as_str(), tool.version.as_str(), tool.web), ("shoptool", DEFAULT_DOTNET_VERSION, false));
        let text = dotnet_dockerfile(&tool, &dir.join("Tool"));
        assert!(text.contains("FROM mcr.microsoft.com/dotnet/runtime:"), "{text}");
        assert!(!text.contains("EXPOSE"), "{text}");
        assert!(text.contains("WORKDIR \"/src\"\n"), "{text}");
        assert!(text.contains("RUN dotnet restore \"./Tool.csproj\""), "{text}");

        // A Dockerfile next to a project in a solution builds from the solution folder.
        write("Shop.Api/Dockerfile", &dotnet_dockerfile(&api, &dir));
        write("Tool/Dockerfile", "FROM scratch\n");
        write("Tool/.dockerignore", "bin\n");
        write("deploy/compose.yaml", "services: {}\n");
        assert_eq!(build_context(&dir.join("Shop.Api/Dockerfile"), &dir), dir);
        assert_eq!(build_context(&dir.join("Tool/Dockerfile"), &dir), dir.join("Tool"));
        let files = [dir.join("Shop.Api/Dockerfile"), dir.join("Tool/Dockerfile"), dir.join("Tool/.dockerignore"), dir.join("deploy/compose.yaml")];
        let (dockerfiles, compose) = docker_files(&files, |n| n == "Dockerfile");
        assert_eq!(dockerfiles, vec![dir.join("Shop.Api/Dockerfile"), dir.join("Tool/Dockerfile")]);
        assert_eq!(compose, vec![dir.join("deploy/compose.yaml")]);

        let services = [
            ComposeService { name: "shop.api".into(), dockerfile: dir.join("Shop.Api/Dockerfile"), context: dir.clone(), web: true },
            ComposeService { name: "tool".into(), dockerfile: dir.join("Tool/Dockerfile"), context: dir.join("Tool"), web: false },
        ];
        let compose = compose_file(&services, &dir);
        assert!(compose.contains("  shop.api:\n    image: shop.api\n    build:\n      context: .\n      dockerfile: Shop.Api/Dockerfile\n    ports:\n      - \"8080:8080\""), "{compose}");
        assert!(compose.contains("  tool:\n    image: tool\n    build:\n      context: ./Tool\n      dockerfile: Dockerfile\n"), "{compose}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
