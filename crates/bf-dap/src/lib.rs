//! `bf-dap`: a small Debug Adapter Protocol client.
//!
//! Like `bf-lsp`, it talks to the adapter over stdio with plain std threads and hands results back
//! through [`flume`] channels, so any executor can drive it. It is tuned for `netcoredbg` (the
//! open-source .NET debugger) but speaks generic DAP.

mod client;
mod types;

use std::path::PathBuf;

pub use client::DapClient;
pub use types::*;

/// Locates `netcoredbg`: an explicit path, then `PATH`, then the usual install folders
/// (`/usr/share/netcoredbg`, `/opt/netcoredbg`, `~/.local/share/netcoredbg`, `~/.dotnet/tools`).
pub fn netcoredbg_command(configured: Option<&str>) -> Option<String> {
    if let Some(path) = configured.filter(|c| !c.is_empty()) {
        return Some(path.to_owned());
    }
    let exe = if cfg!(windows) { "netcoredbg.exe" } else { "netcoredbg" };
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).map(|dir| dir.join(exe)).find(|candidate| candidate.is_file())
    });
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let mut folders: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            folders.push(PathBuf::from(local).join("netcoredbg"));
        }
        folders.push(PathBuf::from(r"C:\Program Files\netcoredbg"));
    } else {
        folders.extend(["/usr/share/netcoredbg", "/usr/lib/netcoredbg", "/opt/netcoredbg", "/usr/local/share/netcoredbg", "/usr/local/netcoredbg"].map(PathBuf::from));
    }
    if let Some(home) = &home {
        folders.push(home.join(".local").join("share").join("netcoredbg"));
        folders.push(home.join(".dotnet").join("tools"));
    }
    on_path
        .or_else(|| folders.into_iter().map(|dir| dir.join(exe)).find(|candidate| candidate.is_file()))
        .map(|p| p.to_string_lossy().into_owned())
}
