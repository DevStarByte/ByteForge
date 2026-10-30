use std::path::{Path, PathBuf};

/// Well-known locations. User data lives in the platform config dir
/// (`~/.config/byteforge` on Linux, `%APPDATA%\byteforge` on Windows).
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

    pub fn user_modules(&self) -> PathBuf {
        self.config_dir.join("modules")
    }

    /// Built-in themes: next to the executable, or the source tree when running via `cargo run`.
    pub fn builtin_themes(&self) -> PathBuf {
        let beside = self.install_dir.join("themes");
        if beside.is_dir() {
            return beside;
        }
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes");
        if dev.is_dir() { dev } else { beside }
    }

    pub fn builtin_modules(&self) -> PathBuf {
        self.install_dir.join("modules")
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
