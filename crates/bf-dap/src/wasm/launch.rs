//! The processes of a Blazor WebAssembly session: the app (`dotnet run`, which serves it together with
//! the debug proxy) and a Chromium-based browser with remote debugging on.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

/// Edge, Chrome or Chromium: `configured` if given, else the usual install places and `PATH`.
pub fn find_browser(configured: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = configured.filter(|c| !c.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
                candidates.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
                candidates.push(base.join(r"Google\Chrome\Application\chrome.exe"));
                candidates.push(base.join(r"Chromium\Application\chrome.exe"));
            }
        }
    } else if cfg!(target_os = "macos") {
        for app in ["Google Chrome.app/Contents/MacOS/Google Chrome", "Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "Chromium.app/Contents/MacOS/Chromium"] {
            candidates.push(Path::new("/Applications").join(app));
        }
    }
    let names: &[&str] = if cfg!(windows) { &["msedge.exe", "chrome.exe"] } else { &["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "microsoft-edge-stable"] };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            candidates.extend(names.iter().map(|n| dir.join(n)));
        }
    }
    candidates.into_iter().find(|c| c.is_file())
}

/// The running browser: its process, its own profile folder and the DevTools address (`127.0.0.1:port`).
pub struct Browser {
    pub child: Child,
    pub profile: PathBuf,
    pub devtools: String,
}

impl Browser {
    /// Starts `exe` on a blank page with a fresh profile and remote debugging on a free port.
    pub fn start(exe: &Path, headless: bool) -> Result<Browser> {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        let profile = std::env::temp_dir().join(format!("byteforge-browser-{}-{stamp}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile)?;
        let mut command = Command::new(exe);
        command
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .args(["--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-features=Translate"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if headless {
            command.arg("--headless=new");
        }
        command.arg("about:blank");
        own_group(&mut command);
        let child = command.spawn().with_context(|| format!("could not start the browser {}", exe.display()))?;
        let mut browser = Browser { child, profile, devtools: String::new() };
        // The browser writes the port it picked into DevToolsActivePort.
        let deadline = Instant::now() + Duration::from_secs(30);
        let file = browser.profile.join("DevToolsActivePort");
        loop {
            if let Some(port) = std::fs::read_to_string(&file).ok().and_then(|t| t.lines().next().map(str::trim).map(str::to_owned)).filter(|p| !p.is_empty()) {
                browser.devtools = format!("127.0.0.1:{port}");
                return Ok(browser);
            }
            if let Ok(Some(status)) = browser.child.try_wait() {
                bail!("the browser exited at once ({status}); is another instance holding the profile?");
            }
            if Instant::now() > deadline {
                bail!("the browser didn't open its debugging port within 30 s");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        kill_tree(&mut self.child);
        // The profile is locked until the browser is gone.
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// `dotnet run --no-build` for `project`; stdout and stderr lines go to `output` with their stream name.
pub fn start_app(project: &Path, profile: Option<&str>, env: &[(String, String)], output: impl Fn(&'static str, String) + Send + Clone + 'static) -> Result<Child> {
    let mut command = Command::new("dotnet");
    command.args(["run", "--no-build", "--project"]).arg(project);
    if let Some(profile) = profile {
        command.args(["--launch-profile", profile]);
    }
    command
        .current_dir(project.parent().unwrap_or(Path::new(".")))
        .env("DOTNET_NOLOGO", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    own_group(&mut command);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().context("could not start dotnet")?;
    for (stream, name) in [(child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), "stdout"), (child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), "stderr")] {
        let Some(stream) = stream else { continue };
        let output = output.clone();
        std::thread::Builder::new().name(format!("bf-wasm-{name}")).spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                output(name, line);
            }
        })?;
    }
    Ok(child)
}

/// Microsoft's debug proxy host for WebAssembly apps (`BrowserDebugHost.dll`, the one Visual Studio and
/// VS Code start), from the WebAssembly SDK pack matching the app's .NET version: in the NuGet cache
/// (`microsoft.net.sdk.webassembly.pack/<version>/WasmAppHost`, restored with the app) or the SDK's
/// `packs/Microsoft.NET.Runtime.WebAssembly.Sdk/<version>/WasmAppHost`. Until .NET 10 the app's dev
/// server started it; .NET 11's gateway doesn't, so the debugger does in every case.
pub fn find_debug_host(project: &Path) -> Result<PathBuf> {
    let major = target_major(project);
    let mut folders: Vec<PathBuf> = Vec::new();
    let nuget = std::env::var_os("NUGET_PACKAGES").map(PathBuf::from).or_else(|| {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| PathBuf::from(h).join(".nuget").join("packages"))
    });
    if let Some(nuget) = nuget {
        folders.push(nuget.join("microsoft.net.sdk.webassembly.pack"));
    }
    for root in dotnet_roots() {
        folders.push(root.join("packs").join("Microsoft.NET.Runtime.WebAssembly.Sdk"));
    }
    let mut found: Vec<(Vec<u32>, PathBuf)> = folders
        .iter()
        .flat_map(|f| std::fs::read_dir(f).into_iter().flatten().flatten())
        .filter_map(|e| {
            let dll = e.path().join("WasmAppHost").join("BrowserDebugHost.dll");
            let version: Vec<u32> = e.file_name().to_string_lossy().split(['.', '-']).map_while(|p| p.parse().ok()).collect();
            (dll.is_file() && !version.is_empty()).then_some((version, dll))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let matching = major.and_then(|m| found.iter().find(|(v, _)| v[0] == m));
    matching.or(found.first()).map(|(_, dll)| dll.clone()).context(
        "the WebAssembly debug proxy (BrowserDebugHost) wasn't found; build the app once so its packages are restored",
    )
}

/// The major .NET version the project targets (`<TargetFramework>net11.0</TargetFramework>` → 11).
fn target_major(project: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(project).ok()?;
    let start = text.find("<TargetFramework")?;
    let rest = &text[start..];
    let value = rest[rest.find('>')? + 1..].trim_start();
    value.strip_prefix("net")?.split(['.', ';']).next()?.parse().ok()
}

/// Folders the .NET SDK is installed in (`DOTNET_ROOT`, the folder of `dotnet` on PATH, the usual places).
fn dotnet_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os("DOTNET_ROOT").map(PathBuf::from).into_iter().collect();
    let exe = if cfg!(windows) { "dotnet.exe" } else { "dotnet" };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(exe);
            if candidate.is_file()
                && let Some(parent) = std::fs::canonicalize(&candidate).ok().and_then(|p| p.parent().map(Path::to_path_buf))
            {
                roots.push(parent);
            }
        }
    }
    roots.extend(
        if cfg!(windows) { vec![r"C:\Program Files\dotnet"] } else { vec!["/usr/share/dotnet", "/usr/lib/dotnet", "/usr/local/share/dotnet", "/opt/dotnet"] }
            .into_iter()
            .map(PathBuf::from),
    );
    roots
}

/// A free local port (the proxy host is told which one to listen on).
pub fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Starts the debug proxy host on `port`, in front of the browser's DevTools at `devtools`; it ends
/// with `owner` (this adapter) at the latest.
pub fn start_debug_host(dll: &Path, devtools: &str, port: u16, output: impl Fn(String) + Send + Sync + 'static) -> Result<Child> {
    let mut command = Command::new("dotnet");
    command
        .arg(dll)
        .arg(format!("--DevToolsUrl=http://{devtools}"))
        .arg(format!("--DevToolsProxyPort={port}"))
        .arg(format!("--OwnerPid={}", std::process::id()))
        .current_dir(dll.parent().unwrap_or(Path::new(".")))
        .env("DOTNET_NOLOGO", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    own_group(&mut command);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().context("could not start the WebAssembly debug proxy")?;
    let streams: Vec<Box<dyn std::io::Read + Send>> = [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten().collect();
    let output = std::sync::Arc::new(output);
    for stream in streams {
        let output = output.clone();
        std::thread::Builder::new().name("bf-wasm-proxy-out".into()).spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                output(line);
            }
        })?;
    }
    // Ready once it accepts connections.
    let deadline = Instant::now() + Duration::from_secs(30);
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    while std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err() {
        if let Ok(Some(status)) = child.try_wait() {
            bail!("the WebAssembly debug proxy exited at once ({status})");
        }
        if Instant::now() > deadline {
            kill_tree(&mut child);
            bail!("the WebAssembly debug proxy didn't start within 30 s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(child)
}

/// The address to open from a "Now listening on: …" line, preferring http (the proxy is reached over
/// plain ws://).
pub fn listening_url(line: &str) -> Option<String> {
    let url = line.split("Now listening on:").nth(1)?.trim();
    (url.starts_with("http://") || url.starts_with("https://")).then(|| url.replace("://[::]", "://localhost").replace("://0.0.0.0", "://localhost"))
}

/// Starts the process in its own process group (Unix), so [`kill_tree`] can end what it started too.
fn own_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

/// Ends `child` and every process it started (`dotnet run` starts the app as its child; browsers start
/// helper processes).
pub fn kill_tree(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-KILL", &format!("-{}", child.id())]).stderr(Stdio::null()).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_target_framework() {
        let dir = std::env::temp_dir().join(format!("bf-wasm-tfm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = dir.join("App.csproj");
        std::fs::write(&project, "<Project Sdk=\"Microsoft.NET.Sdk.BlazorWebAssembly\"><PropertyGroup><TargetFramework>net11.0</TargetFramework></PropertyGroup></Project>").unwrap();
        assert_eq!(target_major(&project), Some(11));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_the_listening_address() {
        assert_eq!(listening_url("info: Microsoft.Hosting.Lifetime[14]\n      Now listening on: http://localhost:5123").as_deref(), Some("http://localhost:5123"));
        assert_eq!(listening_url("      Now listening on: http://[::]:5000").as_deref(), Some("http://localhost:5000"));
        assert_eq!(listening_url("Application started."), None);
    }
}
