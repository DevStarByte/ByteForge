//! Git integration through the `git` CLI (works identically on Linux and Windows, uses the user's
//! git config, credential helpers and hooks). Parsing uses the stable porcelain v2 format.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Absolute path.
    pub path: PathBuf,
    /// Original path for renames/copies.
    pub orig: Option<PathBuf>,
    /// Index (staged) status: '.', 'M', 'A', 'D', 'R', 'C', 'T', 'U'.
    pub index: char,
    /// Worktree status: '.', 'M', 'D', 'T', 'U'.
    pub worktree: char,
    pub untracked: bool,
    pub conflict: bool,
}

impl FileChange {
    pub fn is_staged(&self) -> bool {
        !self.untracked && !self.conflict && self.index != '.'
    }

    pub fn is_unstaged(&self) -> bool {
        self.untracked || self.conflict || self.worktree != '.'
    }

    /// Letter shown in lists (VS Code style): U untracked, A added, M modified, D deleted, R renamed, ! conflict.
    pub fn letter(&self, staged: bool) -> char {
        if self.conflict {
            return '!';
        }
        if self.untracked {
            return 'U';
        }
        match if staged { self.index } else { self.worktree } {
            'A' => 'A',
            'D' => 'D',
            'R' => 'R',
            'C' => 'C',
            '.' => ' ',
            _ => 'M',
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// Commit id of HEAD (None before the first commit).
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub has_commits: bool,
    pub changes: Vec<FileChange>,
}

impl GitStatus {
    pub fn change_for(&self, path: &Path) -> Option<&FileChange> {
        self.changes.iter().find(|c| crate::paths::same_path(&c.path, path))
    }

    /// Number of changed files (the activity bar badge).
    pub fn count(&self) -> usize {
        self.changes.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub remote: bool,
}

#[derive(Clone, Debug)]
pub struct Git {
    root: PathBuf,
}

fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .args(["-c", "core.quotepath=false", "-c", "color.ui=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

fn run(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = git(dir).args(args).output().map_err(|e| anyhow::anyhow!("could not run git (is it installed?): {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = if stderr.trim().is_empty() { stdout.trim().to_owned() } else { stderr.trim().to_owned() };
        anyhow::bail!("git {}: {message}", args.first().copied().unwrap_or_default())
    }
}

/// Output of a command whose result is shown to the user (stdout + stderr, e.g. pull/push progress).
fn run_verbose(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = git(dir).args(args).output().map_err(|e| anyhow::anyhow!("could not run git: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)).trim().to_owned();
    if output.status.success() { Ok(text) } else { anyhow::bail!("{text}") }
}

impl Git {
    /// Finds the repository containing `path`, if any.
    pub fn discover(path: &Path) -> Option<Git> {
        let dir = if path.is_dir() { path } else { path.parent()? };
        let top = run(dir, &["rev-parse", "--show-toplevel"]).ok()?;
        let root = PathBuf::from(top.trim());
        // git prints forward slashes on Windows; normalize to the platform form.
        let root = std::fs::canonicalize(&root).map(strip_verbatim).unwrap_or(root);
        Some(Git { root })
    }

    pub fn init(path: &Path) -> anyhow::Result<Git> {
        run(path, &["init"])?;
        Git::discover(path).ok_or_else(|| anyhow::anyhow!("git init did not create a repository"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy().replace('\\', "/")
    }

    fn rels(&self, paths: &[PathBuf]) -> Vec<String> {
        paths.iter().map(|p| self.rel(p)).collect()
    }

    fn run_paths(&self, args: &[&str], paths: &[PathBuf]) -> anyhow::Result<String> {
        let rels = self.rels(paths);
        let mut all: Vec<&str> = args.to_vec();
        all.push("--");
        all.extend(rels.iter().map(String::as_str));
        run(&self.root, &all)
    }

    pub fn status(&self) -> anyhow::Result<GitStatus> {
        let text = run(&self.root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?;
        Ok(parse_status(&text, &self.root))
    }

    pub fn stage(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        self.run_paths(&["add", "-A"], paths).map(drop)
    }

    pub fn stage_all(&self) -> anyhow::Result<()> {
        run(&self.root, &["add", "-A"]).map(drop)
    }

    pub fn unstage(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        if self.has_commits() {
            self.run_paths(&["restore", "--staged"], paths).map(drop)
        } else {
            self.run_paths(&["rm", "--cached", "-r", "-q"], paths).map(drop)
        }
    }

    pub fn unstage_all(&self) -> anyhow::Result<()> {
        if self.has_commits() { run(&self.root, &["reset", "-q"]).map(drop) } else { run(&self.root, &["rm", "--cached", "-r", "-q", "."]).map(drop) }
    }

    /// Discards working tree changes. Untracked files are deleted.
    pub fn discard(&self, changes: &[FileChange]) -> anyhow::Result<()> {
        let (untracked, tracked): (Vec<&FileChange>, Vec<&FileChange>) = changes.iter().partition(|c| c.untracked);
        for c in untracked {
            if c.path.is_dir() { std::fs::remove_dir_all(&c.path)? } else { std::fs::remove_file(&c.path)? }
        }
        let tracked: Vec<PathBuf> = tracked.iter().map(|c| c.path.clone()).collect();
        if !tracked.is_empty() {
            self.run_paths(&["restore", "--worktree"], &tracked)?;
        }
        Ok(())
    }

    pub fn commit(&self, message: &str, amend: bool) -> anyhow::Result<String> {
        let mut args = vec!["commit", "-m", message];
        if amend {
            args.push("--amend");
        }
        run_verbose(&self.root, &args)
    }

    pub fn has_commits(&self) -> bool {
        run(&self.root, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok()
    }

    pub fn branches(&self) -> anyhow::Result<Vec<Branch>> {
        let text = run(&self.root, &["branch", "-a", "--format=%(HEAD)%00%(refname:short)%00%(refname)"])?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut parts = line.split('\0');
                let head = parts.next()?;
                let name = parts.next()?.to_owned();
                let full = parts.next().unwrap_or_default();
                (!name.ends_with("/HEAD") && !name.is_empty()).then(|| Branch { name, current: head == "*", remote: full.starts_with("refs/remotes/") })
            })
            .collect())
    }

    pub fn checkout(&self, branch: &Branch) -> anyhow::Result<String> {
        if branch.remote {
            // "origin/feature" → local tracking branch "feature".
            let local = branch.name.split_once('/').map_or(branch.name.as_str(), |(_, b)| b);
            run_verbose(&self.root, &["switch", local])
                .or_else(|_| run_verbose(&self.root, &["switch", "--track", &branch.name]))
        } else {
            run_verbose(&self.root, &["switch", &branch.name])
        }
    }

    pub fn create_branch(&self, name: &str) -> anyhow::Result<String> {
        run_verbose(&self.root, &["switch", "-c", name])
    }

    pub fn fetch(&self) -> anyhow::Result<String> {
        run_verbose(&self.root, &["fetch", "--all", "--prune"])
    }

    pub fn pull(&self) -> anyhow::Result<String> {
        run_verbose(&self.root, &["pull", "--ff-only"])
    }

    /// Pushes the current branch; publishes it to `origin` under its own name if it has no upstream yet.
    pub fn push(&self, status: &GitStatus) -> anyhow::Result<String> {
        if status.upstream.is_some() {
            run_verbose(&self.root, &["push"])
        } else {
            let branch = status.branch.clone().ok_or_else(|| anyhow::anyhow!("cannot push a detached HEAD"))?;
            run_verbose(&self.root, &["push", "-u", "origin", &format!("HEAD:{branch}")])
        }
    }

    /// File content at HEAD (None if the file is new or there are no commits).
    pub fn head_content(&self, path: &Path) -> Option<String> {
        let spec = format!("HEAD:{}", self.rel(path));
        let output = git(&self.root).args(["show", &spec]).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"))
    }

    /// (old, new) texts for a change: HEAD ↔ index when `staged`, otherwise index ↔ working tree.
    pub fn diff_texts(&self, change: &FileChange, staged: bool) -> (String, String) {
        let read = |p: &Path| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).replace("
", "
")).unwrap_or_default();
        let orig = change.orig.as_deref().unwrap_or(&change.path);
        if staged {
            (self.head_content(orig).unwrap_or_default(), self.index_content(&change.path).unwrap_or_default())
        } else if change.untracked {
            (String::new(), read(&change.path))
        } else {
            (self.index_content(&change.path).or_else(|| self.head_content(orig)).unwrap_or_default(), read(&change.path))
        }
    }

    /// Staged (index) content of a file.
    pub fn index_content(&self, path: &Path) -> Option<String> {
        let spec = format!(":{}", self.rel(path));
        let output = git(&self.root).args(["show", &spec]).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"))
    }
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

fn parse_status(text: &str, root: &Path) -> GitStatus {
    let mut status = GitStatus { has_commits: true, ..Default::default() };
    let abs = |rel: &str| root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let mut fields = text.split('\0').peekable();
    while let Some(entry) = fields.next() {
        if entry.is_empty() {
            continue;
        }
        if let Some(header) = entry.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" => {
                    status.has_commits = value != "(initial)";
                    status.head = status.has_commits.then(|| value.to_owned());
                }
                "branch.head" => {
                    status.detached = value == "(detached)";
                    status.branch = (!status.detached).then(|| value.to_owned());
                }
                "branch.upstream" => status.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = entry.chars().next().unwrap_or(' ');
        match kind {
            '1' | '2' | 'u' => {
                let field_count = match kind {
                    '1' => 8,
                    '2' => 9,
                    _ => 10,
                };
                let parts: Vec<&str> = entry.splitn(field_count + 1, ' ').collect();
                let Some(path) = parts.get(field_count) else { continue };
                let xy: Vec<char> = parts.get(1).map(|s| s.chars().collect()).unwrap_or_default();
                let orig = if kind == '2' { fields.next().map(abs) } else { None };
                status.changes.push(FileChange {
                    path: abs(path),
                    orig,
                    index: *xy.first().unwrap_or(&'.'),
                    worktree: *xy.get(1).unwrap_or(&'.'),
                    untracked: false,
                    conflict: kind == 'u',
                });
            }
            '?' => status.changes.push(FileChange {
                path: abs(&entry[2..]),
                orig: None,
                index: '.',
                worktree: '?',
                untracked: true,
                conflict: false,
            }),
            _ => {}
        }
    }
    status.changes.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChangeKind {
    Added,
    Modified,
    /// Lines were removed just above this line.
    Deleted,
}

/// Per-line gutter markers comparing `current` against `base` (lines are 0-based in `current`).
pub fn line_changes(base: &str, current: &str) -> Vec<(usize, LineChangeKind)> {
    use similar::{DiffOp, TextDiff};
    let diff = TextDiff::configure().algorithm(similar::Algorithm::Myers).timeout(std::time::Duration::from_millis(500)).diff_lines(base, current);
    let mut out = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Insert { new_index, new_len, .. } => out.extend((new_index..new_index + new_len).map(|l| (l, LineChangeKind::Added))),
            DiffOp::Replace { new_index, new_len, old_len, .. } => {
                out.extend((new_index..new_index + new_len).map(|l| (l, LineChangeKind::Modified)));
                if old_len > new_len {
                    out.push((new_index + new_len, LineChangeKind::Deleted));
                }
            }
            DiffOp::Delete { new_index, .. } => out.push((new_index, LineChangeKind::Deleted)),
            DiffOp::Equal { .. } => {}
        }
    }
    out
}

/// Unified diff text for the diff view.
pub fn unified_diff(base: &str, current: &str, old_name: &str, new_name: &str) -> String {
    similar::TextDiff::from_lines(base, current).unified_diff().context_radius(3).header(old_name, new_name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let text = "# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 h1 h2 src/a.cs\0\
1 A. N... 000000 100644 100644 h1 h2 b.cs\0\
2 R. N... 100644 100644 100644 h1 h2 R100 new name.cs\0old.cs\0\
u UU N... 100644 100644 100644 100644 h1 h2 h3 conflict.cs\0\
? notes.txt\0";
        let s = parse_status(text, Path::new("/repo"));
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!(s.changes.len(), 5);
        let renamed = s.changes.iter().find(|c| c.orig.is_some()).unwrap();
        assert!(renamed.path.ends_with("new name.cs"));
        assert!(s.changes.iter().any(|c| c.conflict));
        assert!(s.changes.iter().any(|c| c.untracked && c.letter(false) == 'U'));
        let a = s.changes.iter().find(|c| c.path.ends_with("a.cs")).unwrap();
        assert!(!a.is_staged() && a.is_unstaged() && a.letter(false) == 'M');
    }

    #[test]
    fn gutter_line_changes() {
        let base = "a\nb\nc\nd\n";
        let current = "a\nB\nc\nnew\nd\n";
        let changes = line_changes(base, current);
        assert!(changes.contains(&(1, LineChangeKind::Modified)));
        assert!(changes.contains(&(3, LineChangeKind::Added)));
        assert_eq!(line_changes("a\nb\n", "a\n"), vec![(1, LineChangeKind::Deleted)]);
    }

    #[test]
    fn real_repository_round_trip() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("bf-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Git::init(&dir).unwrap();
        run(repo.root(), &["config", "user.name", "Test"]).unwrap();
        run(repo.root(), &["config", "user.email", "test@example.com"]).unwrap();
        run(repo.root(), &["config", "commit.gpgsign", "false"]).unwrap();
        let file = repo.root().join("Program.cs");
        std::fs::write(&file, "class A {}\n").unwrap();

        let s = repo.status().unwrap();
        assert!(!s.has_commits);
        assert!(s.change_for(&file).unwrap().untracked);

        repo.stage(std::slice::from_ref(&file)).unwrap();
        assert!(repo.status().unwrap().change_for(&file).unwrap().is_staged());
        repo.unstage(std::slice::from_ref(&file)).unwrap();
        assert!(repo.status().unwrap().change_for(&file).unwrap().untracked);

        repo.stage_all().unwrap();
        repo.commit("initial", false).unwrap();
        let s = repo.status().unwrap();
        assert!(s.changes.is_empty() && s.has_commits && s.branch.is_some());

        std::fs::write(&file, "class A { int x; }\n").unwrap();
        let change = repo.status().unwrap().change_for(&file).cloned().unwrap();
        assert_eq!(change.letter(false), 'M');
        assert_eq!(repo.head_content(&file).as_deref(), Some("class A {}\n"));
        repo.discard(&[change]).unwrap();
        assert!(repo.status().unwrap().changes.is_empty());

        repo.create_branch("feature/x").unwrap();
        let branches = repo.branches().unwrap();
        assert!(branches.iter().any(|b| b.name == "feature/x" && b.current));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
