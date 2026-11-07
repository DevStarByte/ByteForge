//! File references in program output, so the Output panel, the Debug Console and the terminal can
//! open them: MSBuild diagnostics (`File.cs(12,5)`), .NET stack traces (`in /src/File.cs:line 42`),
//! `path:line:col` (compilers, grep) and plain paths. Only references to existing files count.

use std::path::{Path, PathBuf};

/// A file reference in a line of text. Columns are char indices into the line; `line` and `column` are
/// 1-based positions in the file (0 when not given).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLink {
    pub start: usize,
    pub end: usize,
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
}

/// Characters that end a path (paths with spaces only link when quoted output gives them no other end).
fn is_path_char(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, '"' | '\'' | '`' | '<' | '>' | '|' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '=')
}

/// Finds the file references in `text`. Relative paths are resolved against `cwd`.
pub fn find_links(text: &str, cwd: Option<&Path>) -> Vec<FileLink> {
    let chars: Vec<char> = text.chars().collect();
    let mut links = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_path_char(chars[i]) || (i > 0 && is_path_char(chars[i - 1])) {
            i += 1;
            continue;
        }
        let mut end = i;
        while end < chars.len() && is_path_char(chars[end]) {
            end += 1;
        }
        // The candidate may carry ":line:col" or ":line" (and trailing punctuation) at its end.
        let token: String = chars[i..end].iter().collect();
        if let Some(link) = parse_candidate(&token, &chars, i, end, cwd) {
            let next = link.end;
            links.push(link);
            i = next;
        } else {
            i = end;
        }
    }
    links
}

fn parse_candidate(token: &str, chars: &[char], start: usize, end: usize, cwd: Option<&Path>) -> Option<FileLink> {
    let trimmed = token.trim_end_matches(['.', ':']);
    // Stack traces: the token is "/src/File.cs:line" and the number follows after a space.
    let trimmed = trimmed.strip_suffix(":line").unwrap_or(trimmed);
    // "path:line:col" / "path:line"
    let mut parts: Vec<&str> = trimmed.rsplitn(3, ':').collect();
    parts.reverse();
    let (path_text, line, column) = match parts.as_slice() {
        [p, l, c] if is_num(l) && is_num(c) => (*p, l.parse().ok()?, c.parse().ok()?),
        [p, l, c] if is_num(c) => (&trimmed[..p.len() + 1 + l.len()], c.parse().ok()?, 0),
        [p, l] if is_num(l) => (*p, l.parse().ok()?, 0),
        _ => (trimmed, 0, 0),
    };
    if path_text.is_empty() || !path_text.contains(['/', '\\', '.']) || path_text.starts_with("http") {
        return None;
    }
    let path = resolve(path_text, cwd)?;
    let mut link = FileLink { start, end: start + path_text.chars().count(), path, line, column };
    if line > 0 {
        link.end = start + trimmed.chars().count();
        return Some(link);
    }
    // "File.cs(12,5)" / "File.cs(12,5,12,9)" / "File.cs(12)" right after the path.
    let rest: String = chars[end..].iter().take(40).collect();
    if end == link.end
        && let Some(inner) = rest.strip_prefix('(').and_then(|r| r.split_once(')')).map(|(inner, _)| inner)
    {
        let nums: Vec<u32> = inner.split(',').filter_map(|n| n.trim().parse().ok()).collect();
        if !nums.is_empty() && nums.len() == inner.split(',').count() {
            link.line = nums[0];
            link.column = nums.get(1).copied().unwrap_or(0);
            link.end = end + inner.chars().count() + 2;
            return Some(link);
        }
    }
    // ".NET stack trace: "in /src/File.cs:line 42"
    let after: String = chars[start + trimmed.chars().count()..].iter().take(20).collect();
    if let Some(number) = after.strip_prefix(":line ") {
        let digits: String = number.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse() {
            link.line = n;
            link.end = start + trimmed.chars().count() + ":line ".len() + digits.len();
        }
    }
    Some(link)
}

fn is_num(s: &str) -> bool {
    !s.is_empty() && s.len() < 8 && s.bytes().all(|b| b.is_ascii_digit())
}

fn resolve(text: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let expanded = match text.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
        None => PathBuf::from(text),
    };
    let path = if expanded.is_absolute() { expanded } else { cwd?.join(expanded) };
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_references_to_existing_files() {
        let dir = std::env::temp_dir().join(format!("bf-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Pages")).unwrap();
        std::fs::write(dir.join("Program.cs"), "").unwrap();
        std::fs::write(dir.join("Pages").join("Index.cshtml"), "").unwrap();
        let root = Some(dir.as_path());
        let abs = dir.join("Program.cs").display().to_string();

        let msbuild = format!("{abs}(12,5): error CS0103: The name 'x' does not exist [{}]", dir.join("A.csproj").display());
        let links = find_links(&msbuild, root);
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!((links[0].line, links[0].column, links[0].start), (12, 5, 0));
        assert_eq!(links[0].end, abs.chars().count() + "(12,5)".len());

        let trace = format!("   at Demo.Main() in {abs}:line 42");
        let link = &find_links(&trace, root)[0];
        assert_eq!((link.line, link.column), (42, 0));
        assert_eq!(&trace.chars().skip(link.start).take(link.end - link.start).collect::<String>(), &format!("{abs}:line 42"));

        let grep = "Pages/Index.cshtml:3:7: warning";
        let link = &find_links(grep, root)[0];
        assert_eq!(link.path, dir.join("Pages").join("Index.cshtml"));
        assert_eq!((link.line, link.column, link.start, link.end), (3, 7, 0, "Pages/Index.cshtml:3:7".len()));

        let plain = find_links("see Program.cs.", root);
        assert_eq!((plain[0].line, plain[0].end), (0, "see Program.cs".len()));

        assert!(find_links("Missing.cs(1,1): error", root).is_empty());
        assert!(find_links("http://localhost:5000", root).is_empty());
        assert!(find_links("12:30:01 done", root).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
