use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    /// Column of the first match in the line, in chars.
    pub col: usize,
    /// Every match in the line, as char ranges of `text`.
    pub ranges: Vec<(usize, usize)>,
    /// The whole line.
    pub text: String,
    pub preview: String,
}

#[derive(Debug)]
pub enum SearchMessage {
    Hits(Vec<SearchHit>),
    Done { hits: usize, files: usize, millis: u128, truncated: bool },
}

const MAX_HITS: usize = 5000;
const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;

/// Searches `files` for `matcher` on a background thread (one hit per matching line); results stream in
/// batches. Set `cancel` to stop early.
pub fn search(files: Arc<Vec<PathBuf>>, matcher: bf_text::Matcher, cancel: Arc<AtomicBool>) -> flume::Receiver<SearchMessage> {
    let (tx, rx) = flume::unbounded();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
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
                let found = matcher.find_in(line);
                if let Some(&first) = found.first() {
                    matched = true;
                    hits += 1;
                    let ranges: Vec<(usize, usize)> = found.iter().map(|r| bf_text::matcher::char_range(line, *r)).collect();
                    let col = bf_text::matcher::char_range(line, first).0;
                    let preview: String = line.trim().chars().take(200).collect();
                    batch.push(SearchHit { path: path.clone(), line: line_no, col, ranges, text: line.to_owned(), preview });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_files_with_the_query_options() {
        let dir = std::env::temp_dir().join(format!("bf-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("A.cs");
        std::fs::write(&file, "int count = 1;\nvar counter = count + count;\n").unwrap();
        let run = |q: bf_text::Query| {
            let m = bf_text::Matcher::new(&q).unwrap().unwrap();
            let rx = search(Arc::new(vec![file.clone()]), m, Arc::new(AtomicBool::new(false)));
            let mut hits = Vec::new();
            while let Ok(msg) = rx.recv() {
                match msg {
                    SearchMessage::Hits(h) => hits.extend(h),
                    SearchMessage::Done { .. } => break,
                }
            }
            hits
        };
        let words = run(bf_text::Query { text: "count".into(), whole_word: true, ..Default::default() });
        assert_eq!(words.iter().map(|h| (h.line, h.ranges.clone())).collect::<Vec<_>>(), [(0, vec![(4, 9)]), (1, vec![(14, 19), (22, 27)])]);
        let re = run(bf_text::Query { text: r"count\w+".into(), regex: true, ..Default::default() });
        assert_eq!(re.len(), 1);
        assert_eq!(re[0].col, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
