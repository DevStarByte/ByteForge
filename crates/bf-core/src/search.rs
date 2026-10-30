use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    /// Column in chars.
    pub col: usize,
    pub preview: String,
}

#[derive(Debug)]
pub enum SearchMessage {
    Hits(Vec<SearchHit>),
    Done { hits: usize, files: usize, millis: u128, truncated: bool },
}

const MAX_HITS: usize = 5000;
const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;

/// Plain-text search over `files` on a background thread; results stream in batches.
/// Set `cancel` to stop early.
pub fn search(files: Arc<Vec<PathBuf>>, query: String, match_case: bool, cancel: Arc<AtomicBool>) -> flume::Receiver<SearchMessage> {
    let (tx, rx) = flume::unbounded();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let needle = if match_case { query.clone() } else { query.to_lowercase() };
        let (mut hits, mut matched_files) = (0usize, 0usize);
        let mut batch = Vec::new();
        for path in files.iter() {
            if cancel.load(Ordering::Relaxed) || hits >= MAX_HITS {
                break;
            }
            if std::fs::metadata(path).map(|m| m.len() > MAX_FILE_SIZE).unwrap_or(true) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            let mut matched = false;
            for (line_no, line) in text.lines().enumerate() {
                let hay = if match_case { line.to_owned() } else { line.to_lowercase() };
                if let Some(byte) = hay.find(&needle) {
                    matched = true;
                    hits += 1;
                    let col = hay[..byte].chars().count();
                    let preview: String = line.trim().chars().take(200).collect();
                    batch.push(SearchHit { path: path.clone(), line: line_no, col, preview });
                    if hits >= MAX_HITS {
                        break;
                    }
                }
            }
            if matched {
                matched_files += 1;
            }
            if batch.len() >= 200 {
                let _ = tx.send(SearchMessage::Hits(std::mem::take(&mut batch)));
            }
        }
        if !batch.is_empty() {
            let _ = tx.send(SearchMessage::Hits(batch));
        }
        let _ = tx.send(SearchMessage::Done {
            hits,
            files: matched_files,
            millis: started.elapsed().as_millis(),
            truncated: hits >= MAX_HITS,
        });
    });
    rx
}
