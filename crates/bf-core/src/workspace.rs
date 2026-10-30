use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};

use crate::watch::DebouncedWatcher;

const MAX_FILES: usize = 200_000;

/// The opened folder: root, exclusion rules, a cached file list and change notifications.
#[derive(Clone, Default)]
pub struct Workspace {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    root: RwLock<Option<PathBuf>>,
    excludes: RwLock<HashSet<String>>,
    files: Mutex<Option<Arc<Vec<PathBuf>>>>,
    watcher: Mutex<Option<DebouncedWatcher>>,
}

impl Workspace {
    pub fn new(excludes: Vec<String>) -> Self {
        let ws = Self::default();
        ws.set_excludes(excludes);
        ws
    }

    pub fn root(&self) -> Option<PathBuf> {
        self.inner.root.read().clone()
    }

    pub fn set_excludes(&self, excludes: Vec<String>) {
        *self.inner.excludes.write() = excludes.into_iter().collect();
        *self.inner.files.lock() = None;
    }

    pub fn is_excluded_name(&self, name: &str) -> bool {
        self.inner.excludes.read().contains(name)
    }

    pub fn is_excluded_path(&self, path: &Path) -> bool {
        let Some(root) = self.root() else { return false };
        let Ok(relative) = path.strip_prefix(&root) else { return false };
        let excludes = self.inner.excludes.read();
        relative.components().any(|c| excludes.contains(c.as_os_str().to_string_lossy().as_ref()))
    }

    /// Opens a folder; `on_change` receives relevant changed paths from a background thread.
    pub fn open(&self, root: PathBuf, on_change: impl Fn(Vec<PathBuf>) + Send + 'static) -> anyhow::Result<()> {
        anyhow::ensure!(root.is_dir(), "{} is not a folder", root.display());
        *self.inner.root.write() = Some(root.clone());
        *self.inner.files.lock() = None;
        let this = self.clone();
        let watcher = DebouncedWatcher::new(&root, true, Duration::from_millis(200), move |paths| {
            let relevant: Vec<PathBuf> = paths.into_iter().filter(|p| !this.is_excluded_path(p)).collect();
            if !relevant.is_empty() {
                *this.inner.files.lock() = None;
                on_change(relevant);
            }
        });
        match watcher {
            Ok(w) => *self.inner.watcher.lock() = Some(w),
            Err(e) => log::warn!("file watching unavailable (raise fs.inotify.max_user_watches on Linux): {e}"),
        }
        Ok(())
    }

    /// All non-excluded files below the root (cached until something changes).
    pub fn files(&self) -> Arc<Vec<PathBuf>> {
        if let Some(files) = self.inner.files.lock().clone() {
            return files;
        }
        let Some(root) = self.root() else { return Arc::new(Vec::new()) };
        let excludes = self.inner.excludes.read().clone();
        let mut result = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if excludes.contains(&name) {
                    continue;
                }
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    stack.push(entry.path());
                } else if kind.is_file() {
                    result.push(entry.path());
                    if result.len() >= MAX_FILES {
                        break;
                    }
                }
            }
        }
        result.sort();
        let files = Arc::new(result);
        *self.inner.files.lock() = Some(files.clone());
        files
    }

    /// Directory listing for the explorer: folders first, then files, case-insensitive, excludes applied.
    pub fn list_dir(&self, dir: &Path) -> Vec<(PathBuf, bool)> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let excludes = self.inner.excludes.read();
        let mut items: Vec<(PathBuf, bool)> = entries
            .flatten()
            .filter(|e| !excludes.contains(e.file_name().to_string_lossy().as_ref()))
            .map(|e| (e.path(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
            .collect();
        items.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| {
                let an = a.0.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                let bn = b.0.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                an.cmp(&bn)
            })
        });
        items
    }
}
