use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// File system watcher that coalesces bursts of events into one callback with the changed paths.
pub struct DebouncedWatcher {
    _watcher: RecommendedWatcher,
}

impl DebouncedWatcher {
    pub fn new(
        path: &Path,
        recursive: bool,
        delay: Duration,
        callback: impl Fn(Vec<PathBuf>) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let (tx, rx) = flume::unbounded::<PathBuf>();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                for path in event.paths {
                    let _ = tx.send(path);
                }
            }
        })?;
        watcher.watch(path, if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive })?;

        std::thread::Builder::new().name("bf-watch".into()).spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut batch: HashSet<PathBuf> = HashSet::from([first]);
                while let Ok(path) = rx.recv_timeout(delay) {
                    batch.insert(path);
                }
                callback(batch.into_iter().collect());
            }
        })?;
        Ok(Self { _watcher: watcher })
    }
}
