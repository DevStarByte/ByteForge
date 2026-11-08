//! Languages contributed by modules: file matching, a line comment and line-based regex highlighting.
//! The IDE registers them with [`set_module_languages`]; [`crate::Language::Module`] refers to one by index.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::{Arc, Mutex, RwLock};

use crate::Span;

/// A module's language, ready to use.
pub struct ModuleLanguage {
    pub id: &'static str,
    pub name: &'static str,
    pub line_comment: Option<&'static str>,
    pub matches: Box<dyn Fn(&str) -> bool + Send + Sync>,
    pub rules: Vec<RegexRule>,
}

/// Matches of `regex` (or of its capture group `group`) get the theme token `token`.
pub struct RegexRule {
    pub regex: regex::Regex,
    pub group: usize,
    pub token: &'static str,
}

impl RegexRule {
    /// Compiles a rule; `None` (and a log line) when the pattern is invalid.
    pub fn new(pattern: &str, group: usize, token: &str) -> Option<Self> {
        match regex::Regex::new(pattern) {
            Ok(regex) => Some(Self { regex, group, token: intern(token) }),
            Err(e) => {
                log::warn!("invalid highlight rule {pattern:?}: {e}");
                None
            }
        }
    }
}

static LANGUAGES: RwLock<Vec<Arc<ModuleLanguage>>> = RwLock::new(Vec::new());

/// Replaces the module languages (the turned-on modules' languages, in module order).
pub fn set_module_languages(languages: Vec<ModuleLanguage>) {
    *LANGUAGES.write().unwrap_or_else(|e| e.into_inner()) = languages.into_iter().map(Arc::new).collect();
}

pub(crate) fn get(index: u16) -> Option<Arc<ModuleLanguage>> {
    LANGUAGES.read().unwrap_or_else(|e| e.into_inner()).get(index as usize).cloned()
}

/// The first module language for a file name, as its index.
pub(crate) fn find(file_name: &str) -> Option<u16> {
    LANGUAGES.read().unwrap_or_else(|e| e.into_inner()).iter().position(|l| (l.matches)(file_name)).map(|i| i as u16)
}

pub(crate) fn find_id(id: &str) -> Option<u16> {
    LANGUAGES.read().unwrap_or_else(|e| e.into_inner()).iter().position(|l| l.id.eq_ignore_ascii_case(id)).map(|i| i as u16)
}

/// A `'static` copy of `s`, shared between all callers (languages are re-registered when modules change).
pub fn intern(s: &str) -> &'static str {
    static STRINGS: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut guard = STRINGS.lock().unwrap_or_else(|e| e.into_inner());
    let set = guard.get_or_insert_with(HashSet::new);
    if let Some(existing) = set.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    set.insert(leaked);
    leaked
}

/// Spans for the lines of `text` that intersect `range`. Within a line, every rule's matches are
/// collected and the earlier rule wins where they overlap.
pub(crate) fn highlight(language: &ModuleLanguage, text: &str, range: Range<usize>) -> Vec<Span> {
    let start = range.start.min(text.len());
    let end = range.end.min(text.len());
    let mut spans = Vec::new();
    if start >= end {
        return spans;
    }
    let mut line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    while line_start < end {
        let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
        let line = &text[line_start..line_end];
        let mut paint: Vec<Option<&'static str>> = vec![None; line.len()];
        for rule in &language.rules {
            for caps in rule.regex.captures_iter(line) {
                let Some(m) = caps.get(rule.group) else { continue };
                for slot in &mut paint[m.start()..m.end()] {
                    slot.get_or_insert(rule.token);
                }
            }
        }
        let mut i = 0;
        while i < paint.len() {
            let kind = paint[i];
            let run = i;
            while i < paint.len() && paint[i] == kind {
                i += 1;
            }
            if let Some(kind) = kind {
                let (s, e) = ((line_start + run).max(start), (line_start + i).min(end));
                if s < e {
                    spans.push(Span { start: s, end: e, kind });
                }
            }
        }
        line_start = line_end + 1;
    }
    spans
}
