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

/// One entry of the stash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stash {
    /// 0 is the newest (`stash@{0}`).
    pub index: usize,
    /// "On main: message" / "WIP on main: 1a2b3c subject".
    pub message: String,
    /// When it was stashed, relative ("2 hours ago").
    pub when: String,
}

/// What to do with a stash entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StashOp {
    Pop,
    Apply,
    Drop,
}

fn parse_stashes(text: &str) -> Vec<Stash> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\0');
            let reference = parts.next()?;
            let index = reference.strip_prefix("stash@{")?.strip_suffix('}')?.parse().ok()?;
            Some(Stash { index, message: parts.next().unwrap_or_default().to_owned(), when: parts.next().unwrap_or_default().to_owned() })
        })
        .collect()
}

/// One commit of the history list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author date, relative ("3 days ago").
    pub when: String,
    /// Author date, absolute ("2026-11-08 14:02").
    pub date: String,
    /// Branch and tag names pointing at this commit ("HEAD -> master, origin/master, tag: v0.3.1").
    pub refs: String,
    pub subject: String,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.id[..self.id.len().min(7)]
    }
}

/// Who last changed a line, from `git blame`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    pub commit: String,
    pub author: String,
    /// Author time (Unix seconds).
    pub time: i64,
    pub summary: String,
    /// The file's path in that commit (relative, forward slashes).
    pub filename: String,
    /// Not committed yet (the line is new or changed in the working tree or the editor).
    pub uncommitted: bool,
}

impl BlameLine {
    /// "Ann, 3 days ago • Fix the parser" (or "You • Uncommitted changes").
    pub fn label(&self, now: i64) -> String {
        if self.uncommitted {
            return "You \u{2022} Uncommitted changes".to_owned();
        }
        format!("{}, {} \u{2022} {}", self.author, relative_time(now - self.time), self.summary)
    }
}

/// A short age for narrow columns: "now", "5m", "3h", "2d", "3w", "4mo", "2y".
pub fn short_age(seconds: i64) -> String {
    let s = seconds.max(0);
    match s {
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        86_400..604_800 => format!("{}d", s / 86_400),
        604_800..2_629_746 => format!("{}w", s / 604_800),
        2_629_746..31_556_952 => format!("{}mo", s / 2_629_746),
        _ => format!("{}y", s / 31_556_952),
    }
}

impl BlameLine {
    /// The blame gutter's text, in a fixed width: commit, author, age ("a1b2c3d Ann Example    3d").
    pub fn gutter_label(&self, now: i64) -> String {
        if self.uncommitted {
            return format!("{:<26}", "        not committed");
        }
        let author: String = self.author.chars().take(12).collect();
        format!("{} {:<12} {:>5}", &self.commit[..self.commit.len().min(7)], author, short_age(now - self.time))
    }
}

/// "just now", "5 minutes ago", "3 days ago", "2 years ago".
pub fn relative_time(seconds: i64) -> String {
    let s = seconds.max(0);
    let (n, unit) = match s {
        0..60 => return "just now".to_owned(),
        60..3600 => (s / 60, "minute"),
        3600..86_400 => (s / 3600, "hour"),
        86_400..604_800 => (s / 86_400, "day"),
        604_800..2_629_746 => (s / 604_800, "week"),
        2_629_746..31_556_952 => (s / 2_629_746, "month"),
        _ => (s / 31_556_952, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// A file changed by a commit (compared with its first parent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFile {
    /// Absolute path.
    pub path: PathBuf,
    /// Original path for renames/copies.
    pub orig: Option<PathBuf>,
    /// 'A', 'M', 'D', 'R', 'C', 'T'.
    pub status: char,
}

#[derive(Clone, Debug)]
pub struct Git {
    root: PathBuf,
}

fn git(dir: &Path) -> Command {
    let mut command = crate::process::command("git");
    command
        .current_dir(dir)
        .args(["-c", "core.quotepath=false", "-c", "color.ui=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        // English messages, like the rest of ByteForge (some, like "would be overwritten", are matched).
        .env("LANG", "en_US.UTF-8")
        .env("LANGUAGE", "en")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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

/// Like [`run`], with `input` on stdin.
fn run_with_input(dir: &Path, args: &[&str], input: &str) -> anyhow::Result<String> {
    use std::io::Write;
    let mut child = git(dir).args(args).stdin(Stdio::piped()).spawn().map_err(|e| anyhow::anyhow!("could not run git (is it installed?): {e}"))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input = input.to_owned();
    // Written from another thread: git may fill stdout before it has read all of stdin.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output()?;
    let _ = writer.join();
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        anyhow::bail!("git {}: {}", args.first().copied().unwrap_or_default(), String::from_utf8_lossy(&output.stderr).trim())
    }
}

/// Output of a command whose result is shown to the user (stdout + stderr, e.g. pull/push progress).
/// The `--rebase` / `--no-rebase` argument for `git pull`, from the configured rebase setting. With neither
/// it nor `pull.ff` set it means merge (git itself would refuse a pull of diverged branches then). A
/// configured value is left to git (`--no-rebase` would override `pull.ff = only` in newer git), except
/// "interactive", which would wait for an editor.
fn pull_rebase_arg(config: Option<&str>, ff_configured: bool) -> Option<&'static str> {
    match config {
        None if ff_configured => None,
        None => Some("--no-rebase"),
        Some("interactive" | "i") => Some("--rebase=true"),
        Some(_) => None,
    }
}

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
        Ok(parse_branches(&text))
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

    /// A file's content at `spec` (`:path` for the index, `HEAD:path`), exactly as stored.
    fn raw_content(&self, spec: &str) -> Option<String> {
        let output = git(&self.root).args(["show", spec]).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Makes `content` the staged version of `path` (keeping its file mode).
    fn write_index(&self, path: &Path, content: &str) -> anyhow::Result<()> {
        let rel = self.rel(path);
        let sha = run_with_input(&self.root, &["hash-object", "-w", "--stdin"], content)?.trim().to_owned();
        let listed = run(&self.root, &["ls-files", "-s", "--", &rel]).unwrap_or_default();
        let mode = listed.split_whitespace().next().filter(|m| m.len() == 6).unwrap_or("100644").to_owned();
        run(&self.root, &["update-index", "--add", "--cacheinfo", &format!("{mode},{sha},{rel}")])?;
        Ok(())
    }

    /// Stages one change of a file (index ↔ working tree): the index gets its content with just that
    /// change applied, in the index's line endings.
    pub fn stage_change(&self, path: &Path, change: &crate::diff::LineChange) -> anyhow::Result<()> {
        let rel = self.rel(path);
        let old = self.raw_content(&format!(":{rel}")).unwrap_or_default();
        let mut new = std::fs::read_to_string(path)?;
        if !old.contains("\r\n") && !old.is_empty() {
            new = new.replace("\r\n", "\n"); // autocrlf: the index keeps LF
        }
        self.write_index(path, &crate::diff::apply_change(&old, &new, change))
    }

    /// Unstages one change of a file (HEAD ↔ index): the index gets its content with that change taken back.
    pub fn unstage_change(&self, path: &Path, change: &crate::diff::LineChange) -> anyhow::Result<()> {
        let rel = self.rel(path);
        let old = self.raw_content(&format!("HEAD:{rel}")).unwrap_or_default();
        let new = self.raw_content(&format!(":{rel}")).ok_or_else(|| anyhow::anyhow!("{rel} isn't staged"))?;
        self.write_index(path, &crate::diff::revert_change(&old, &new, change))
    }

    /// Takes one change of a file's working copy back to what the index has (in the file's line endings).
    pub fn revert_change(&self, path: &Path, change: &crate::diff::LineChange) -> anyhow::Result<()> {
        let rel = self.rel(path);
        let new = std::fs::read_to_string(path)?;
        let tracked = self.raw_content(&format!(":{rel}")).or_else(|| self.raw_content(&format!("HEAD:{rel}")));
        let untracked = tracked.is_none();
        let mut old = tracked.unwrap_or_default();
        if new.contains("\r\n") && !old.contains("\r\n") {
            old = old.replace('\n', "\r\n");
        }
        let reverted = crate::diff::revert_change(&old, &new, change);
        // git has no version of an untracked file: reverting all of it means the file goes, not an empty file.
        if untracked && reverted.is_empty() {
            std::fs::remove_file(path)?;
        } else {
            std::fs::write(path, reverted)?;
        }
        Ok(())
    }

    /// (old, new) texts of a file for the diff editor, by path: HEAD ↔ index when `staged`, otherwise
    /// index ↔ working tree (line endings normalized, as [`Git::diff_texts`]).
    pub fn diff_texts_of(&self, path: &Path, staged: bool) -> (String, String) {
        let read = |p: &Path| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).replace("\r\n", "\n")).unwrap_or_default();
        if staged {
            (self.head_content(path).unwrap_or_default(), self.index_content(path).unwrap_or_default())
        } else {
            (self.index_content(path).or_else(|| self.head_content(path)).unwrap_or_default(), read(path))
        }
    }

    /// The sides of a conflicted file as git keeps them: the common base, the current side (ours) and
    /// the incoming side (theirs). A side is `None` when the file doesn't exist on it.
    pub fn conflict_sides(&self, path: &Path) -> (Option<String>, Option<String>, Option<String>) {
        let rel = self.rel(path);
        let stage = |n: u8| self.raw_content(&format!(":{n}:{rel}")).map(|t| t.replace("\r\n", "\n"));
        (stage(1), stage(2), stage(3))
    }

    /// Puts the uncommitted changes (untracked files included) on the stash.
    pub fn stash_push(&self, message: &str) -> anyhow::Result<String> {
        run_verbose(&self.root, &["stash", "push", "--include-untracked", "-m", message])
    }

    /// The stash, newest first.
    pub fn stashes(&self) -> anyhow::Result<Vec<Stash>> {
        let text = run(&self.root, &["stash", "list", "--format=%gd%x00%gs%x00%cr"])?;
        Ok(parse_stashes(&text))
    }

    /// `pop` (apply and drop), `apply` or `drop` the stash entry `index` (0 is the newest).
    pub fn stash_op(&self, op: StashOp, index: usize) -> anyhow::Result<String> {
        let entry = format!("stash@{{{index}}}");
        let verb = match op {
            StashOp::Pop => "pop",
            StashOp::Apply => "apply",
            StashOp::Drop => "drop",
        };
        // --index brings staged changes back staged (pop and apply only).
        if op == StashOp::Drop { run_verbose(&self.root, &["stash", verb, &entry]) } else { run_verbose(&self.root, &["stash", verb, "--index", &entry]) }
    }

    /// "Undo Last Commit": HEAD goes back one commit and its changes stay, staged. Returns the commit's
    /// message (to edit and commit again). The first commit is undone by removing the branch ref.
    pub fn undo_last_commit(&self) -> anyhow::Result<String> {
        let message = run(&self.root, &["log", "-1", "--format=%B", "HEAD"])?.trim().to_owned();
        if run(&self.root, &["rev-parse", "--verify", "-q", "HEAD~1"]).is_ok() {
            run(&self.root, &["reset", "--soft", "HEAD~1"])?;
        } else {
            run(&self.root, &["update-ref", "-d", "HEAD"])?;
        }
        Ok(message)
    }

    /// The last commit's full message.
    pub fn last_commit_message(&self) -> anyhow::Result<String> {
        Ok(run(&self.root, &["log", "-1", "--format=%B", "HEAD"])?.trim().to_owned())
    }

    /// Adds the staged changes to the last commit; with an empty `message` it keeps the old one.
    pub fn amend(&self, message: &str) -> anyhow::Result<String> {
        if message.trim().is_empty() { run_verbose(&self.root, &["commit", "--amend", "--no-edit"]) } else { self.commit(message, true) }
    }

    /// Whether a merge is in progress (stopped at conflicts).
    pub fn is_merging(&self) -> bool {
        run(&self.root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok()
    }

    /// Merges `branch` into the current branch (a merge commit with git's default message when needed).
    pub fn merge(&self, branch: &str) -> anyhow::Result<String> {
        run_verbose(&self.root, &["merge", "--no-edit", branch])
    }

    pub fn abort_merge(&self) -> anyhow::Result<String> {
        run_verbose(&self.root, &["merge", "--abort"])
    }

    /// Renames the current branch.
    pub fn rename_branch(&self, name: &str) -> anyhow::Result<String> {
        run_verbose(&self.root, &["branch", "-m", name])
    }

    /// A lightweight tag `name` at `at` (a commit id), or at HEAD.
    pub fn create_tag(&self, name: &str, at: Option<&str>) -> anyhow::Result<String> {
        let mut args = vec!["tag", name];
        args.extend(at);
        run_verbose(&self.root, &args)
    }

    pub fn push_tags(&self) -> anyhow::Result<String> {
        run_verbose(&self.root, &["push", "--tags"])
    }

    /// A new commit that undoes `id` (the history keeps both).
    pub fn revert_commit(&self, id: &str) -> anyhow::Result<String> {
        run_verbose(&self.root, &["revert", "--no-edit", id])
    }

    pub fn create_branch(&self, name: &str) -> anyhow::Result<String> {
        run_verbose(&self.root, &["switch", "-c", name])
    }

    pub fn fetch(&self) -> anyhow::Result<String> {
        run_verbose(&self.root, &["fetch", "--all", "--prune"])
    }

    /// Pulls the current branch. When it and the remote both have new commits, it merges them, or rebases
    /// when `pull.rebase` (or `branch.<name>.rebase`) says so; `pull.ff = only` keeps it fast-forward only.
    /// Conflicts stop it like `git pull` does: [`is_merging`](Self::is_merging) or
    /// [`is_rebasing`](Self::is_rebasing) is then true.
    pub fn pull(&self) -> anyhow::Result<String> {
        let mut args = vec!["pull", "--no-edit"];
        let ff_configured = run(&self.root, &["config", "--get", "pull.ff"]).is_ok();
        args.extend(pull_rebase_arg(self.rebase_config().as_deref(), ff_configured));
        run_verbose(&self.root, &args)
    }

    /// The `rebase` setting that applies to a pull of the current branch: `branch.<name>.rebase`, else
    /// `pull.rebase`.
    fn rebase_config(&self) -> Option<String> {
        let get = |key: &str| run(&self.root, &["config", "--get", key]).ok().map(|v| v.trim().to_lowercase()).filter(|v| !v.is_empty());
        let branch = run(&self.root, &["symbolic-ref", "--short", "-q", "HEAD"]).ok().map(|b| b.trim().to_owned());
        branch.and_then(|b| get(&format!("branch.{b}.rebase"))).or_else(|| get("pull.rebase"))
    }

    /// Whether a rebase is in progress (stopped at conflicts).
    pub fn is_rebasing(&self) -> bool {
        ["rebase-merge", "rebase-apply"].iter().any(|dir| {
            run(&self.root, &["rev-parse", "--git-path", dir]).is_ok_and(|p| {
                let p = PathBuf::from(p.trim());
                if p.is_absolute() { p.is_dir() } else { self.root.join(p).is_dir() }
            })
        })
    }

    /// Goes on with a rebase after its conflicts are resolved and staged (each commit keeps its message).
    pub fn continue_rebase(&self) -> anyhow::Result<String> {
        run_verbose(&self.root, &["-c", "core.editor=true", "rebase", "--continue"])
    }

    pub fn abort_rebase(&self) -> anyhow::Result<String> {
        run_verbose(&self.root, &["rebase", "--abort"])
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
        let read = |p: &Path| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).replace("\r\n", "\n")).unwrap_or_default();
        let orig = change.orig.as_deref().unwrap_or(&change.path);
        if staged {
            (self.head_content(orig).unwrap_or_default(), self.index_content(&change.path).unwrap_or_default())
        } else if change.untracked {
            (String::new(), read(&change.path))
        } else {
            (self.index_content(&change.path).or_else(|| self.head_content(orig)).unwrap_or_default(), read(&change.path))
        }
    }

    /// Commits reachable from HEAD, newest first (`skip` pages through older ones).
    pub fn log(&self, limit: usize, skip: usize) -> anyhow::Result<Vec<Commit>> {
        if !self.has_commits() {
            return Ok(Vec::new());
        }
        let (limit, skip) = (format!("-n{limit}"), format!("--skip={skip}"));
        let text = run(
            &self.root,
            &["log", "-z", &limit, &skip, "--date=format:%Y-%m-%d %H:%M", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%ar%x1f%ad%x1f%D%x1f%s", "HEAD"],
        )?;
        Ok(parse_log(&text))
    }

    /// The history for the commit graph: HEAD's commits and those of its upstream (when it has one), so a
    /// branch and its diverged remote both show; children before their parents (`--topo-order`).
    pub fn graph_log(&self, limit: usize, skip: usize) -> anyhow::Result<Vec<Commit>> {
        if !self.has_commits() {
            return Ok(Vec::new());
        }
        let (limit, skip) = (format!("-n{limit}"), format!("--skip={skip}"));
        let upstream = run(&self.root, &["rev-parse", "--verify", "--quiet", "@{upstream}"]).is_ok_and(|s| !s.trim().is_empty());
        let mut args = vec!["log", "-z", "--topo-order", &limit, &skip, "--date=format:%Y-%m-%d %H:%M", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%ar%x1f%ad%x1f%D%x1f%s", "HEAD"];
        if upstream {
            args.push("@{upstream}");
        }
        Ok(parse_log(&run(&self.root, &args)?))
    }

    /// The commits the upstream has that HEAD doesn't (what a pull brings), newest first; empty without an
    /// upstream.
    pub fn incoming(&self) -> Vec<String> {
        run(&self.root, &["rev-list", "HEAD..@{upstream}"]).map(|t| t.lines().map(str::to_owned).collect()).unwrap_or_default()
    }

    /// The commits HEAD has that its upstream doesn't (what a push sends), newest first; empty without an
    /// upstream.
    pub fn outgoing(&self) -> Vec<String> {
        run(&self.root, &["rev-list", "@{upstream}..HEAD"]).map(|t| t.lines().map(str::to_owned).collect()).unwrap_or_default()
    }

    /// Files a commit changed, compared with its first parent (or everything, for a root commit).
    pub fn commit_files(&self, commit: &Commit) -> anyhow::Result<Vec<CommitFile>> {
        let mut args = vec!["diff-tree", "-r", "-z", "-M", "--name-status", "--no-commit-id"];
        match commit.parents.first() {
            Some(parent) => args.extend([parent.as_str(), commit.id.as_str()]),
            None => args.extend(["--root", commit.id.as_str()]),
        }
        let text = run(&self.root, &args)?;
        Ok(parse_name_status(&text, &self.root))
    }

    /// Content of a file at a revision (None if it does not exist there).
    pub fn content_at(&self, rev: &str, path: &Path) -> Option<String> {
        let spec = format!("{rev}:{}", self.rel(path));
        let output = git(&self.root).args(["show", &spec]).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"))
    }

    /// (old, new) texts of a file changed by a commit: first parent ↔ commit.
    pub fn commit_diff_texts(&self, commit: &Commit, file: &CommitFile) -> (String, String) {
        let orig = file.orig.as_deref().unwrap_or(&file.path);
        let old = commit.parents.first().and_then(|p| self.content_at(p, orig)).unwrap_or_default();
        let new = if file.status == 'D' { String::new() } else { self.content_at(&commit.id, &file.path).unwrap_or_default() };
        (old, new)
    }

    /// Relative path inside the repository, with forward slashes.
    pub fn relative(&self, path: &Path) -> String {
        self.rel(path)
    }

    /// Who last changed each line of `text` (the file's current content, unsaved edits included), by
    /// line. Lines not committed yet are marked `uncommitted`.
    pub fn blame(&self, path: &Path, text: &str) -> anyhow::Result<Vec<BlameLine>> {
        let rel = self.rel(path);
        let out = run_with_input(&self.root, &["blame", "--porcelain", "--contents", "-", "--", &rel], text)?;
        Ok(parse_blame(&out))
    }

    /// The commits that changed a file, newest first, following renames; each with the file as that commit
    /// changed it (its path then, and the status letter).
    pub fn file_history(&self, path: &Path, limit: usize) -> anyhow::Result<Vec<(Commit, CommitFile)>> {
        if !self.has_commits() {
            return Ok(Vec::new());
        }
        let rel = self.rel(path);
        let limit = format!("-n{limit}");
        let text = run(
            &self.root,
            &["log", "--follow", "-M", &limit, "--date=format:%Y-%m-%d %H:%M", "--name-status", "--format=%x1e%H%x1f%P%x1f%an%x1f%ae%x1f%ar%x1f%ad%x1f%D%x1f%s%x1f", "--", &rel],
        )?;
        Ok(parse_file_history(&text, &self.root))
    }

    /// One commit's details.
    pub fn commit_info(&self, id: &str) -> Option<Commit> {
        let text = run(&self.root, &["log", "-z", "-n1", "--date=format:%Y-%m-%d %H:%M", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%ar%x1f%ad%x1f%D%x1f%s", id]).ok()?;
        parse_log(&text).into_iter().next()
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

fn parse_log(text: &str) -> Vec<Commit> {
    text.split('\0')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let mut f = record.split('\x1f');
            let id = f.next()?.to_owned();
            if id.is_empty() {
                return None;
            }
            Some(Commit {
                id,
                parents: f.next()?.split_whitespace().map(str::to_owned).collect(),
                author: f.next()?.to_owned(),
                email: f.next()?.to_owned(),
                when: f.next()?.to_owned(),
                date: f.next()?.to_owned(),
                refs: f.next()?.to_owned(),
                subject: f.next().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// Parses `git blame --porcelain`: a header per line (`<sha> <orig> <final> [<count>]`), the commit's
/// fields the first time it appears, then the line itself after a tab.
fn parse_blame(text: &str) -> Vec<BlameLine> {
    use std::collections::HashMap;
    let mut commits: HashMap<String, BlameLine> = HashMap::new();
    let mut out: Vec<(usize, BlameLine)> = Vec::new();
    let mut current: Option<(String, usize)> = None;
    for line in text.lines() {
        if line.starts_with('\t') {
            if let Some((sha, final_line)) = current.take() {
                let entry = commits.get(&sha).cloned().unwrap_or_else(|| BlameLine {
                    commit: sha.clone(),
                    author: String::new(),
                    time: 0,
                    summary: String::new(),
                    filename: String::new(),
                    uncommitted: sha.bytes().all(|b| b == b'0'),
                });
                out.push((final_line, entry));
            }
            continue;
        }
        let mut parts = line.split(' ');
        let first = parts.next().unwrap_or_default();
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            let final_line = parts.nth(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(1);
            commits.entry(first.to_owned()).or_insert_with(|| BlameLine {
                commit: first.to_owned(),
                author: String::new(),
                time: 0,
                summary: String::new(),
                filename: String::new(),
                uncommitted: first.bytes().all(|b| b == b'0'),
            });
            current = Some((first.to_owned(), final_line));
            continue;
        }
        let Some((sha, _)) = &current else { continue };
        let Some(entry) = commits.get_mut(sha) else { continue };
        let value = line.split_once(' ').map_or("", |(_, v)| v);
        match first {
            "author" => entry.author = value.to_owned(),
            "author-time" => entry.time = value.parse().unwrap_or(0),
            "summary" => entry.summary = value.to_owned(),
            "filename" => entry.filename = value.to_owned(),
            _ => {}
        }
    }
    out.sort_by_key(|(l, _)| *l);
    out.into_iter().map(|(_, b)| b).collect()
}

/// Parses `git log --name-status` with a `\x1e`-started, `\x1f`-separated header per commit.
fn parse_file_history(text: &str, root: &Path) -> Vec<(Commit, CommitFile)> {
    let abs = |rel: &str| root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    text.split('\x1e')
        .filter_map(|record| {
            let fields: Vec<&str> = record.splitn(9, '\x1f').collect();
            if fields.len() < 9 {
                return None;
            }
            let commit = Commit {
                id: fields[0].trim().to_owned(),
                parents: fields[1].split_whitespace().map(str::to_owned).collect(),
                author: fields[2].to_owned(),
                email: fields[3].to_owned(),
                when: fields[4].to_owned(),
                date: fields[5].to_owned(),
                refs: fields[6].to_owned(),
                subject: fields[7].to_owned(),
            };
            let change = fields[8].lines().find(|l| !l.trim().is_empty())?;
            let parts: Vec<&str> = change.split('\t').collect();
            let status = parts.first()?.chars().next()?;
            let file = match parts.as_slice() {
                [_, old, new] => CommitFile { path: abs(new), orig: Some(abs(old)), status },
                [_, path] => CommitFile { path: abs(path), orig: None, status },
                _ => return None,
            };
            Some((commit, file))
        })
        .collect()
}

/// Parses `git diff-tree --name-status -z` output ("M\0path\0", "R100\0old\0new\0").
fn parse_name_status(text: &str, root: &Path) -> Vec<CommitFile> {
    let abs = |rel: &str| root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let mut out = Vec::new();
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    while let Some(code) = fields.next() {
        let status = code.chars().next().unwrap_or('M');
        let Some(first) = fields.next() else { break };
        if matches!(status, 'R' | 'C') {
            let Some(second) = fields.next() else { break };
            out.push(CommitFile { path: abs(second), orig: Some(abs(first)), status });
        } else {
            out.push(CommitFile { path: abs(first), orig: None, status });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
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

/// Parses `git branch -a --format=%(HEAD)%00%(refname:short)%00%(refname)`. Remote HEAD pointers are left
/// out by their full name: newer git shortens `refs/remotes/origin/HEAD` to just `origin`.
fn parse_branches(text: &str) -> Vec<Branch> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\0');
            let head = parts.next()?;
            let name = parts.next()?.to_owned();
            let full = parts.next().unwrap_or_default();
            (!full.ends_with("/HEAD") && !name.is_empty()).then(|| Branch { name, current: head == "*", remote: full.starts_with("refs/remotes/") })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn remote_head_is_not_a_branch() {
        let text = "*\0master\0refs/heads/master\n \0origin\0refs/remotes/origin/HEAD\n \0origin/master\0refs/remotes/origin/master\n";
        let names: Vec<String> = super::parse_branches(text).into_iter().map(|b| b.name).collect();
        assert_eq!(names, ["master", "origin/master"]);
    }

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
    fn parses_log_and_name_status() {
        let text = "a1\x1fp1 p2\x1fAnn\x1fann@x.org\x1f2 days ago\x1f2026-11-06 10:00\x1fHEAD -> main\x1fMerge it\0\
\nb2\x1f\x1fBob\x1fbob@x.org\x1f3 weeks ago\x1f2026-10-01 09:00\x1f\x1fInitial";
        let log = parse_log(text);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].parents, ["p1", "p2"]);
        assert_eq!(log[0].refs, "HEAD -> main");
        assert!(log[1].parents.is_empty() && log[1].id == "b2");

        let files = parse_name_status("M\0src/a.cs\0R087\0old.cs\0new.cs\0D\0gone.cs\0", Path::new("/repo"));
        assert_eq!(files.len(), 3);
        let renamed = files.iter().find(|f| f.status == 'R').unwrap();
        assert!(renamed.path.ends_with("new.cs") && renamed.orig.as_ref().unwrap().ends_with("old.cs"));
    }

    #[test]
    fn parses_blame_porcelain() {
        let a = "a".repeat(40);
        let z = "0".repeat(40);
        let text = format!(
            "{a} 1 1 2\nauthor Ann\nauthor-time 1700000000\nsummary Initial\nfilename src/a.cs\n\tline one\n{a} 2 2\n\tline two\n\
             {z} 3 3 1\nauthor Not Committed Yet\nauthor-time 1800000000\nsummary Version of a.cs from a.cs\nfilename src/a.cs\n\tnew line\n"
        );
        let blame = parse_blame(&text);
        assert_eq!(blame.len(), 3);
        assert_eq!((blame[0].author.as_str(), blame[0].summary.as_str(), blame[0].time), ("Ann", "Initial", 1_700_000_000));
        assert_eq!(blame[1].commit, a);
        assert!(blame[2].uncommitted && !blame[0].uncommitted);
        assert_eq!(blame[0].label(1_700_000_000 + 3 * 86_400), "Ann, 3 days ago \u{2022} Initial");
        assert_eq!(blame[2].label(0), "You \u{2022} Uncommitted changes");
    }

    #[test]
    fn blame_gutter_labels_have_one_width() {
        let b = BlameLine { commit: "a1b2c3d4e5f6".into(), author: "Ann Example-Longname".into(), time: 0, summary: "x".into(), filename: "a".into(), uncommitted: false };
        assert_eq!(b.gutter_label(3 * 86_400), "a1b2c3d Ann Example-    3d");
        let u = BlameLine { uncommitted: true, ..b.clone() };
        assert_eq!(u.gutter_label(0).chars().count(), b.gutter_label(0).chars().count());
        assert_eq!((short_age(30), short_age(7200), short_age(40 * 86_400), short_age(800 * 86_400)), ("now".into(), "2h".into(), "1mo".into(), "2y".into()));
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(5), "just now");
        assert_eq!(relative_time(60), "1 minute ago");
        assert_eq!(relative_time(7200), "2 hours ago");
        assert_eq!(relative_time(40 * 86_400), "1 month ago");
        assert_eq!(relative_time(800 * 86_400), "2 years ago");
    }

    #[test]
    fn parses_file_history() {
        let text = "\x1eh2\x1fh1\x1fAnn\x1fa@x\x1f2 days ago\x1f2026-11-09 10:00\x1f\x1fRename it\x1f\n\nR100\tsrc/old.cs\tsrc/new.cs\n                    \x1eh1\x1f\x1fBob\x1fb@x\x1f3 weeks ago\x1f2026-10-01 09:00\x1f\x1fAdd it\x1f\n\nA\tsrc/old.cs\n";
        let history = parse_file_history(text, Path::new("/repo"));
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].0.subject, "Rename it");
        assert_eq!(history[0].1.status, 'R');
        assert!(history[0].1.path.ends_with("new.cs") && history[0].1.orig.as_ref().unwrap().ends_with("old.cs"));
        assert_eq!((history[1].1.status, history[1].0.author.as_str()), ('A', "Bob"));
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
        // the files below are LF; with a global core.autocrlf=true, git would check them out as CRLF
        run(repo.root(), &["config", "core.autocrlf", "false"]).unwrap();
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

        std::fs::write(&file, "class A { int y; }\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("second", false).unwrap();
        let log = repo.log(50, 0).unwrap();
        assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["second", "initial"]);
        assert_eq!(repo.log(50, 1).unwrap().len(), 1);
        let blame = repo.blame(&file, "class A { int y; }\nnew line\n").unwrap();
        assert_eq!(blame.len(), 2);
        assert_eq!((blame[0].summary.as_str(), blame[0].uncommitted, blame[1].uncommitted), ("second", false, true));
        let history = repo.file_history(&file, 50).unwrap();
        assert_eq!(history.iter().map(|(c, f)| (c.subject.as_str(), f.status)).collect::<Vec<_>>(), [("second", 'M'), ("initial", 'A')]);
        assert_eq!(repo.commit_info(&log[0].id).map(|c| c.subject), Some("second".to_owned()));
        let files = repo.commit_files(&log[0]).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(repo.commit_diff_texts(&log[0], &files[0]), ("class A {}\n".to_owned(), "class A { int y; }\n".to_owned()));
        assert_eq!(repo.commit_files(&log[1]).unwrap()[0].status, 'A');

        // CRLF in the working tree is normalized, like the index side.
        std::fs::write(&file, "class A {\r\n}\r\n").unwrap();
        let change = repo.status().unwrap().change_for(&file).cloned().unwrap();
        assert_eq!(repo.diff_texts(&change, false).1, "class A {\n}\n");
        repo.discard(&[change]).unwrap();

        repo.create_branch("feature/x").unwrap();
        let branches = repo.branches().unwrap();
        assert!(branches.iter().any(|b| b.name == "feature/x" && b.current));

        // Staging, unstaging and reverting single changes of a file.
        run(repo.root(), &["checkout", "-q", "-"]).unwrap();
        let multi = repo.root().join("Multi.cs");
        std::fs::write(&multi, "one\ntwo\nthree\nfour\nfive\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("multi", false).unwrap();
        std::fs::write(&multi, "one\nTWO\nthree\nfour\nfive\nsix\n").unwrap();
        let (old, new) = repo.diff_texts_of(&multi, false);
        let d = crate::diff::compare(&old, &new);
        assert_eq!(d.changes.len(), 2);
        repo.stage_change(&multi, &d.changes[1]).unwrap();
        assert_eq!(repo.index_content(&multi).as_deref(), Some("one\ntwo\nthree\nfour\nfive\nsix\n"), "only the second change is staged");
        assert_eq!(std::fs::read_to_string(&multi).unwrap(), "one\nTWO\nthree\nfour\nfive\nsix\n", "the working file is untouched");
        let staged = crate::diff::compare(&repo.diff_texts_of(&multi, true).0, &repo.diff_texts_of(&multi, true).1);
        repo.unstage_change(&multi, &staged.changes[0]).unwrap();
        assert_eq!(repo.index_content(&multi).as_deref(), Some("one\ntwo\nthree\nfour\nfive\n"), "unstaged again");
        let (old, new) = repo.diff_texts_of(&multi, false);
        let d = crate::diff::compare(&old, &new);
        repo.revert_change(&multi, &d.changes[0]).unwrap();
        assert_eq!(std::fs::read_to_string(&multi).unwrap(), "one\ntwo\nthree\nfour\nfive\nsix\n", "TWO is reverted, six stays");
        std::fs::write(&multi, "one\ntwo\nthree\nfour\nfive\n").unwrap();
        // An untracked file is one change; reverting it deletes the file rather than emptying it.
        let fresh = repo.root().join("Fresh.cs");
        std::fs::write(&fresh, "class Fresh {}\n").unwrap();
        let (old, new) = repo.diff_texts_of(&fresh, false);
        repo.revert_change(&fresh, &crate::diff::compare(&old, &new).changes[0]).unwrap();
        assert!(!fresh.exists(), "the untracked file is gone");
        run(repo.root(), &["checkout", "-q", "-"]).unwrap();

        // A real merge conflict, as git writes it: found and resolved.
        std::fs::write(&file, "class A { int incoming; }\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("incoming", false).unwrap();
        run(repo.root(), &["checkout", "-q", "-"]).unwrap();
        std::fs::write(&file, "class A { int current; }\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("current", false).unwrap();
        assert!(run(repo.root(), &["merge", "feature/x"]).is_err());
        let text = std::fs::read_to_string(&file).unwrap();
        let conflicts = crate::conflicts::find(&text);
        assert_eq!(conflicts.len(), 1, "{text}");
        assert_eq!((conflicts[0].current_label.as_str(), conflicts[0].incoming_label.as_str()), ("HEAD", "feature/x"));
        assert!(repo.status().unwrap().change_for(&file).unwrap().conflict);
        let (base, ours, theirs) = repo.conflict_sides(&file);
        assert!(base.is_some());
        assert_eq!((ours.as_deref(), theirs.as_deref()), (Some("class A { int current; }\n"), Some("class A { int incoming; }\n")));
        let both = crate::conflicts::apply(&text, &conflicts[0], crate::conflicts::Resolution::Both);
        assert_eq!(both, "class A { int current; }\nclass A { int incoming; }\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pull_merges_unless_rebase_is_configured() {
        assert_eq!(pull_rebase_arg(None, false), Some("--no-rebase"));
        assert_eq!(pull_rebase_arg(None, true), None, "pull.ff is left to git");
        assert_eq!(pull_rebase_arg(Some("true"), false), None);
        assert_eq!(pull_rebase_arg(Some("merges"), false), None);
        assert_eq!(pull_rebase_arg(Some("false"), false), None);
        assert_eq!(pull_rebase_arg(Some("interactive"), false), Some("--rebase=true"));
    }

    /// Pulling diverged branches from a real remote: merge, rebase (`pull.rebase`), a rebase stopped at a
    /// conflict that is continued or aborted, and `pull.ff = only`.
    #[test]
    fn pull_of_diverged_branches() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("bf-git-pull-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "--bare", "-b", "main", "remote.git"]).unwrap();
        let configure = |root: &Path| {
            for (k, v) in [("user.name", "Test"), ("user.email", "test@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
                run(root, &["config", k, v]).unwrap();
            }
        };
        std::fs::create_dir_all(dir.join("theirs")).unwrap();
        run(&dir.join("theirs"), &["init", "-q", "-b", "main"]).unwrap();
        let theirs = Git::discover(&dir.join("theirs")).unwrap();
        configure(theirs.root());
        let commit = |repo: &Git, name: &str, text: &str| {
            std::fs::write(repo.root().join(name), text).unwrap();
            repo.stage_all().unwrap();
            repo.commit(&format!("{name}: {}", text.trim()), false).unwrap();
        };
        commit(&theirs, "A.cs", "a\n");
        run(theirs.root(), &["remote", "add", "origin", dir.join("remote.git").to_str().unwrap()]).unwrap();
        run(theirs.root(), &["push", "-q", "-u", "origin", "main"]).unwrap();
        run(&dir, &["clone", "-q", dir.join("remote.git").to_str().unwrap(), "mine"]).unwrap();
        let mine = Git::discover(&dir.join("mine")).unwrap();
        configure(mine.root());
        let diverge = |name: &str, theirs_text: &str, mine_text: &str| {
            run(theirs.root(), &["pull", "-q", "--ff-only"]).unwrap();
            commit(&theirs, name, theirs_text);
            run(theirs.root(), &["push", "-q"]).unwrap();
            commit(&mine, if name == "A.cs" { name } else { "Mine.cs" }, mine_text);
        };
        let parents = |repo: &Git| repo.log(1, 0).unwrap()[0].parents.len();
        let read = |name: &str| std::fs::read_to_string(mine.root().join(name)).unwrap();

        // No setting: a merge commit with both sides.
        diverge("B.cs", "b\n", "m1\n");
        let head = mine.log(1, 0).unwrap()[0].id.clone();
        assert_eq!(mine.outgoing(), std::slice::from_ref(&head), "my commit is outgoing");
        // The graph's history has both sides: mine, the remote's (fetched) and what they share.
        run(mine.root(), &["fetch", "-q"]).unwrap();
        let incoming = mine.incoming();
        assert_eq!(incoming.len(), 1, "their commit is incoming");
        let graph: Vec<String> = mine.graph_log(50, 0).unwrap().into_iter().map(|c| c.id).collect();
        assert_eq!(graph.len(), 3, "{graph:?}");
        assert!(graph.contains(&head) && graph.contains(&incoming[0]));
        mine.pull().unwrap();
        assert!(mine.incoming().is_empty(), "pulled");
        assert_eq!(parents(&mine), 2, "a merge commit");
        assert_eq!((read("B.cs").as_str(), read("Mine.cs").as_str()), ("b\n", "m1\n"));
        assert_eq!(mine.outgoing().len(), 2, "my commit and the merge");
        run(mine.root(), &["push", "-q"]).unwrap();
        assert!(mine.outgoing().is_empty(), "pushed");

        // pull.rebase: my commit goes on top of theirs, no merge commit.
        run(mine.root(), &["config", "pull.rebase", "true"]).unwrap();
        diverge("C.cs", "c\n", "m2\n");
        mine.pull().unwrap();
        assert_eq!(parents(&mine), 1, "rebased");
        assert_eq!(mine.log(2, 0).unwrap()[1].subject, "C.cs: c");
        run(mine.root(), &["push", "-q"]).unwrap();

        // A conflicting rebase stops; resolved and staged, it continues.
        diverge("A.cs", "theirs\n", "mine\n");
        assert!(mine.pull().is_err());
        assert!(mine.is_rebasing());
        std::fs::write(mine.root().join("A.cs"), "both\n").unwrap();
        mine.stage_all().unwrap();
        mine.continue_rebase().unwrap();
        assert!(!mine.is_rebasing());
        assert_eq!(mine.log(1, 0).unwrap()[0].subject, "A.cs: mine", "my commit keeps its message");
        assert_eq!(read("A.cs"), "both\n");
        run(mine.root(), &["push", "-q"]).unwrap();

        // Aborted, everything is as before the pull.
        diverge("A.cs", "theirs again\n", "mine again\n");
        assert!(mine.pull().is_err() && mine.is_rebasing());
        mine.abort_rebase().unwrap();
        assert!(!mine.is_rebasing());
        assert_eq!(read("A.cs"), "mine again\n");

        // pull.ff = only (and no rebase): diverged branches are refused, nothing changes.
        run(mine.root(), &["config", "--unset", "pull.rebase"]).unwrap();
        run(mine.root(), &["config", "pull.ff", "only"]).unwrap();
        let before = mine.log(1, 0).unwrap()[0].id.clone();
        assert!(mine.pull().is_err());
        assert!(!mine.is_rebasing() && !mine.is_merging());
        assert_eq!(mine.log(1, 0).unwrap()[0].id, before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stash_list_parses() {
        let text = "stash@{0}\0On main: wip\x003 minutes ago\nstash@{1}\0WIP on main: 1a2b3c first\x002 days ago\n";
        let stashes = parse_stashes(text);
        assert_eq!(stashes.len(), 2);
        assert_eq!((stashes[0].index, stashes[0].message.as_str(), stashes[0].when.as_str()), (0, "On main: wip", "3 minutes ago"));
        assert_eq!(stashes[1].index, 1);
    }

    /// The commands of the Source Control "…" menu, on a real repository.
    #[test]
    fn undo_amend_stash_merge_tag_and_revert() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("bf-git-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Git::init(&dir).unwrap();
        for (k, v) in [("user.name", "Test"), ("user.email", "test@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false"), ("tag.gpgsign", "false")] {
            run(repo.root(), &["config", k, v]).unwrap();
        }
        let file = repo.root().join("A.cs");
        let commit = |text: &str, message: &str| {
            std::fs::write(&file, text).unwrap();
            repo.stage_all().unwrap();
            repo.commit(message, false).unwrap();
        };
        let subjects = || repo.log(50, 0).unwrap().into_iter().map(|c| c.subject).collect::<Vec<_>>();

        // Undo the first commit: no commits left, the file stays staged.
        commit("one\n", "first");
        assert_eq!(repo.undo_last_commit().unwrap(), "first");
        let s = repo.status().unwrap();
        assert!(!s.has_commits && s.change_for(&file).unwrap().is_staged());
        repo.commit("first", false).unwrap();

        // Undo a later commit: back one, its change staged, the message returned.
        commit("two\n", "second\n\nwith a body");
        assert_eq!(repo.undo_last_commit().unwrap(), "second\n\nwith a body");
        assert_eq!(subjects(), ["first"]);
        assert!(repo.status().unwrap().change_for(&file).unwrap().is_staged());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two\n", "the change stays in the file");

        // Amend: keeps the message when none is given, takes a new one otherwise.
        repo.amend("").unwrap();
        assert_eq!(subjects(), ["first"]);
        assert!(repo.status().unwrap().changes.is_empty());
        assert_eq!(repo.content_at("HEAD", &file).as_deref(), Some("two\n"));
        repo.amend("first, amended").unwrap();
        assert_eq!(repo.last_commit_message().unwrap(), "first, amended");

        // Stash: push, list, apply (kept), drop, pop (removed).
        std::fs::write(&file, "stashed\n").unwrap();
        repo.stash_push("mine").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two\n");
        let stashes = repo.stashes().unwrap();
        assert_eq!(stashes.len(), 1);
        assert!(stashes[0].message.ends_with("mine"), "{:?}", stashes[0]);
        repo.stash_op(StashOp::Apply, 0).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "stashed\n");
        assert_eq!(repo.stashes().unwrap().len(), 1, "apply keeps the entry");
        repo.discard(&repo.status().unwrap().changes).unwrap();
        repo.stash_op(StashOp::Pop, 0).unwrap();
        assert!(repo.stashes().unwrap().is_empty(), "pop removes it");
        repo.stash_push("again").unwrap();
        repo.stash_op(StashOp::Drop, 0).unwrap();
        assert!(repo.stashes().unwrap().is_empty() && repo.status().unwrap().changes.is_empty());

        // Rename the branch; merge another one, then a conflicting merge that is aborted.
        repo.rename_branch("trunk").unwrap();
        assert_eq!(repo.status().unwrap().branch.as_deref(), Some("trunk"));
        repo.create_branch("side").unwrap();
        let other = repo.root().join("B.cs");
        std::fs::write(&other, "b\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("side work", false).unwrap();
        run(repo.root(), &["switch", "-q", "trunk"]).unwrap();
        repo.merge("side").unwrap();
        assert!(other.exists() && !repo.is_merging());
        run(repo.root(), &["switch", "-q", "side"]).unwrap();
        commit("side\n", "side change");
        run(repo.root(), &["switch", "-q", "trunk"]).unwrap();
        commit("trunk\n", "trunk change");
        assert!(repo.merge("side").is_err(), "conflict");
        assert!(repo.is_merging());
        repo.abort_merge().unwrap();
        assert!(!repo.is_merging());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "trunk\n");

        // Tags, at HEAD and at an older commit; revert makes a new commit that undoes one.
        repo.create_tag("v1", None).unwrap();
        let log = repo.log(50, 0).unwrap();
        repo.create_tag("v0", Some(&log[1].id)).unwrap();
        let tags = run(repo.root(), &["tag", "--list"]).unwrap();
        assert!(tags.contains("v0") && tags.contains("v1"), "{tags}");
        repo.revert_commit(&log[0].id).unwrap();
        assert!(subjects()[0].starts_with("Revert \"trunk change\""), "{:?}", subjects());
        assert_ne!(std::fs::read_to_string(&file).unwrap(), "trunk\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
