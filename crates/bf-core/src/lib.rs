//! UI-independent kernel of ByteForge: paths, settings, themes, workspace indexing and watching,
//! search, the dotnet CLI runner and the out-of-process module (plugin) host.

pub mod dotnet;
pub mod git;
pub mod paths;
pub mod plugins;
pub mod search;
pub mod settings;
pub mod theme;
pub mod watch;
pub mod workspace;

pub use paths::AppPaths;
pub use settings::Settings;
pub use theme::{ThemeData, ThemeInfo, ThemeService, TokenStyle};
pub use workspace::Workspace;

/// A line of text for the Output panel.
pub type OutputSender = flume::Sender<String>;

/// Formats a timestamped log line.
pub fn log_line(message: impl std::fmt::Display) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let (h, m, s) = ((now / 3600) % 24, (now / 60) % 60, now % 60);
    format!("[{h:02}:{m:02}:{s:02} UTC] {message}")
}
