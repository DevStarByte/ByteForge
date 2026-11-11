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

/// `text` with the matches replaced (`template`, with regex groups filled in), on every line or only on
/// `only_line` (0-based): the new text and how many matches were replaced. Line endings (LF or CRLF)
/// stay as they are.
pub fn replace_in_text(text: &str, matcher: &bf_text::Matcher, template: &str, only_line: Option<usize>) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    for (ix, piece) in text.split_inclusive('\n').enumerate() {
        let (line, ending) = match piece.strip_suffix("\r\n") {
            Some(l) => (l, "\r\n"),
            None => match piece.strip_suffix('\n') {
                Some(l) => (l, "\n"),
                None => (piece, ""),
            },
        };
        if only_line.is_some_and(|l| l != ix) {
            out.push_str(piece);
            continue;
        }
        let mut last = 0;
        for (start, end) in matcher.find_in(line) {
            out.push_str(&line[last..start]);
            out.push_str(&matcher.replacement(line, start, template));
            last = end;
            count += 1;
        }
        out.push_str(&line[last..]);
        out.push_str(ending);
    }
    (out, count)
}

/// Replaces the matches in a file on disk (`only_line`: just that line), keeping a UTF-8 BOM.
pub fn replace_in_file(path: &std::path::Path, matcher: &bf_text::Matcher, template: &str, only_line: Option<usize>) -> std::io::Result<usize> {
    let bytes = std::fs::read(path)?;
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]) {
        Some(rest) => (true, rest),
        None => (false, &bytes[..]),
    };
    let Ok(text) = std::str::from_utf8(body) else { return Ok(0) };
    let (new, count) = replace_in_text(text, matcher, template, only_line);
    if count > 0 {
        let mut out = Vec::with_capacity(new.len() + 3);
        if bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(new.as_bytes());
        std::fs::write(path, out)?;
    }
    Ok(count)
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

    #[test]
    fn replaces_in_text_and_files() {
        let m = bf_text::Matcher::new(&bf_text::Query { text: r"(\w+)\.Count\(\)".into(), regex: true, ..Default::default() }).unwrap().unwrap();
        let text = "a = xs.Count();\r\nb = ys.Count() + zs.Count();\r\nlast";
        let (new, n) = replace_in_text(text, &m, "$1.Length", None);
        assert_eq!((new.as_str(), n), ("a = xs.Length;\r\nb = ys.Length + zs.Length;\r\nlast", 3));
        let (one, n) = replace_in_text(text, &m, "$1.Length", Some(1));
        assert_eq!((one.as_str(), n), ("a = xs.Count();\r\nb = ys.Length + zs.Length;\r\nlast", 2));

        let dir = std::env::temp_dir().join(format!("bf-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("B.cs");
        std::fs::write(&file, b"\xEF\xBB\xBFvar n = xs.Count();\r\n").unwrap();
        assert_eq!(replace_in_file(&file, &m, "$1.Length", None).unwrap(), 1);
        assert_eq!(std::fs::read(&file).unwrap(), b"\xEF\xBB\xBFvar n = xs.Length;\r\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
