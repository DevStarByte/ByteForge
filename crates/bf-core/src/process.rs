//! Starting programs. Everything ByteForge starts goes through [`command`]: on Windows the program runs
//! without a console window. ByteForge is a GUI app without a console of its own, so each console program it
//! starts (git, dotnet, curl, taskkill, a module's python, …) would otherwise pop up a console window.
//!
//! The program still gets a console, just an invisible one, and whatever it starts in turn shares it: a
//! module's own tools (`jb cleanupcode`, `sqlcmd`, …) stay hidden too.
//!
//! A test in this module fails when code calls `Command::new` directly.

use std::ffi::OsStr;
use std::process::Command;

/// `CREATE_NO_WINDOW`: a console program runs with a console that has no window.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A [`Command`] for `program` that shows no console window on Windows (elsewhere a plain one).
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every program ByteForge starts goes through `command`, so none of them opens a console window on
    /// Windows. (Tests may start programs directly; their code comes after `#[cfg(test)]`.)
    #[test]
    fn programs_are_started_through_command() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut offenders = Vec::new();
        for krate in ["bf-core", "byteforge"] {
            let mut files = Vec::new();
            collect(&root.join(krate).join("src"), &mut files);
            for file in files {
                if file.ends_with("process.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&file).unwrap();
                for (n, line) in text.lines().enumerate().take_while(|(_, l)| !l.starts_with("#[cfg(test)]")) {
                    if line.contains("Command::new(") {
                        offenders.push(format!("{}:{}", file.display(), n + 1));
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "start programs with bf_core::process::command, not Command::new:\n{}", offenders.join("\n"));
    }

    fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
}
