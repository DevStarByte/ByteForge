use std::path::{Path, PathBuf};

/// Well-known locations. User data lives in the platform config dir (`~/.config/byteforge` on Linux,
/// `%APPDATA%\byteforge` on Windows, `~/Library/Application Support/byteforge` on macOS).
#[derive(Clone, Debug)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub install_dir: PathBuf,
}

impl AppPaths {
    pub fn detect() -> Self {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("byteforge");
        let install_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        Self { config_dir, install_dir }
    }

    pub fn ensure_created(&self) {
        for dir in [&self.config_dir, &self.user_themes(), &self.user_modules()] {
            let _ = std::fs::create_dir_all(dir);
        }
    }

    /// Downloads ByteForge keeps (e.g. netcoredbg for containers): the OS cache folder, else the config folder.
    pub fn cache_dir(&self) -> PathBuf {
        dirs::cache_dir().map(|d| d.join("byteforge")).unwrap_or_else(|| self.config_dir.join("cache"))
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn state_file(&self) -> PathBuf {
        self.config_dir.join("state.json")
    }

    pub fn crash_log(&self) -> PathBuf {
        self.config_dir.join("crash.log")
    }

    pub fn user_themes(&self) -> PathBuf {
        self.config_dir.join("themes")
    }

    /// User snippets, one VS Code-format file per language (`csharp.json`, …).
    pub fn user_snippets(&self) -> PathBuf {
        self.config_dir.join("snippets")
    }

    pub fn user_modules(&self) -> PathBuf {
        self.config_dir.join("modules")
    }

    /// Built-in themes: next to the executable (or in the app bundle's Resources), or the source tree when running via `cargo run`.
    pub fn builtin_themes(&self) -> PathBuf {
        let beside = self.install_dir.join("themes");
        if beside.is_dir() {
            return beside;
        }
        // Inside a macOS app bundle: Contents/MacOS/byteforge, Contents/Resources/themes.
        let bundled = self.install_dir.join("../Resources/themes");
        if bundled.is_dir() {
            return bundled;
        }
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes");
        if dev.is_dir() { dev } else { beside }
    }

    /// Built-in modules: next to the executable (or in the app bundle's Resources), or the source tree when running via `cargo run`.
    pub fn builtin_modules(&self) -> PathBuf {
        let beside = self.install_dir.join("modules");
        if beside.is_dir() {
            return beside;
        }
        // Inside a macOS app bundle: Contents/MacOS/byteforge, Contents/Resources/modules.
        let bundled = self.install_dir.join("../Resources/modules");
        if bundled.is_dir() {
            return bundled;
        }
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
        if dev.is_dir() { dev } else { beside }
    }
}

/// Paths compare case-insensitively on Windows and macOS.
pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

/// macOS starts apps from Finder or the Dock with launchd's bare PATH (`/usr/bin:/bin:/usr/sbin:/sbin`),
/// so `dotnet`, Homebrew's `git`, `csharp-ls` and the rest wouldn't be found. This takes the PATH of the
/// user's login shell (unless started from a terminal, which already has it) and adds the usual .NET and
/// Homebrew folders. Does nothing on other platforms.
///
/// Call it first thing in `main`, before any other thread starts: it changes the process environment.
pub fn import_login_shell_path() {
    #[cfg(target_os = "macos")]
    mac_path::import();
}

#[cfg(target_os = "macos")]
mod mac_path {
    use std::io::{IsTerminal, Read};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const MARKER: &str = "__BYTEFORGE_PATH__";

    pub fn import() {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs: Vec<PathBuf> = Vec::new();
        if !std::io::stdin().is_terminal()
            && let Some(shell_path) = login_shell_path()
        {
            dirs.extend(std::env::split_paths(&shell_path));
        }
        dirs.extend(std::env::split_paths(&current));
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut extra: Vec<PathBuf> = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/local/share/dotnet"].map(PathBuf::from).into();
        if let Some(home) = &home {
            extra.push(home.join(".dotnet"));
            extra.push(home.join(".dotnet").join("tools"));
        }
        dirs.extend(extra.into_iter().filter(|d| d.is_dir()));
        let mut seen = std::collections::HashSet::new();
        dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
        if let Ok(joined) = std::env::join_paths(dirs)
            && joined != current
        {
            // SAFETY: called at startup before any other thread exists (see `import_login_shell_path`).
            unsafe { std::env::set_var("PATH", joined) };
        }
    }

    /// Runs `$SHELL -l -i -c 'printenv PATH'`; gives up after a few seconds so a slow or interactive
    /// shell profile can't keep the IDE from starting.
    fn login_shell_path() -> Option<String> {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
        let mut child = Command::new(shell)
            .args(["-l", "-i", "-c", &format!("echo {MARKER}; printenv PATH")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let mut out = String::new();
        child.stdout.take()?.read_to_string(&mut out).ok()?;
        // Profiles may print banners; the PATH is the line after the marker.
        let after = out.split_once(MARKER)?.1;
        after.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
    }
}
