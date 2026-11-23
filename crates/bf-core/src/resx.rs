//! `.resx` resource files: the string entries of a file, the files of one resource in every language
//! (`Strings.resx`, `Strings.de.resx`, `Strings.fr-CA.resx`, …), and edits that change only the
//! `<data>` entries they touch (the rest of the file stays byte for byte).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A string resource of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub value: String,
    pub comment: Option<String>,
    /// The 0-based line of its `<data>` element.
    pub line: usize,
}

/// The string entries of a `.resx` file, in file order. Entries with a `type` or `mimetype` (images,
/// files, …) aren't strings and are left out.
pub fn parse(text: &str) -> anyhow::Result<Vec<Entry>> {
    let doc = roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })?;
    let mut out = Vec::new();
    for data in doc.root_element().children().filter(|n| n.has_tag_name("data")) {
        if data.attribute("type").is_some() || data.attribute("mimetype").is_some() {
            continue;
        }
        let Some(name) = data.attribute("name") else { continue };
        let child_text = |tag: &str| data.children().find(|c| c.has_tag_name(tag)).map(|c| c.text().unwrap_or_default().to_owned());
        out.push(Entry {
            name: name.to_owned(),
            value: child_text("value").unwrap_or_default(),
            comment: child_text("comment"),
            line: text[..data.range().start].matches('\n').count(),
        });
    }
    Ok(out)
}

/// The culture of a resource file name's stem: "de" for `Strings.de`, "zh-Hans" for `Strings.zh-Hans`,
/// none for `Strings` (the neutral file) or `Some.Dotted.Name`.
fn culture_of(stem: &str) -> Option<(&str, &str)> {
    let (base, last) = stem.rsplit_once('.')?;
    let mut parts = last.split('-');
    let lang = parts.next()?;
    let lang_ok = (2..=3).contains(&lang.len()) && lang.chars().all(|c| c.is_ascii_lowercase());
    let rest_ok = parts.all(|p| (2..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()));
    (lang_ok && rest_ok && !base.is_empty()).then_some((base, last))
}

/// One resource in all its languages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Family {
    /// `Strings` of `Strings.resx`.
    pub name: String,
    /// The neutral file (`Strings.resx`); it may not exist when only translations do.
    pub neutral: PathBuf,
    /// (culture, file), sorted by culture.
    pub cultures: Vec<(String, PathBuf)>,
}

impl Family {
    /// The files that exist: the neutral one first, then each language.
    pub fn files(&self) -> Vec<(Option<String>, PathBuf)> {
        let mut out = Vec::new();
        if self.neutral.is_file() {
            out.push((None, self.neutral.clone()));
        }
        out.extend(self.cultures.iter().map(|(c, p)| (Some(c.clone()), p.clone())));
        out
    }
}

/// The resource `path` belongs to: its neutral file and the language files next to it.
pub fn family(path: &Path) -> Option<Family> {
    let dir = path.parent()?;
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let name = culture_of(&stem).map_or(stem.clone(), |(base, _)| base.to_owned());
    let mut cultures = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("resx")) {
            continue;
        }
        let s = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some((base, culture)) = culture_of(&s)
            && base == name
        {
            cultures.push((culture.to_owned(), p.clone()));
        }
    }
    cultures.sort();
    Some(Family { neutral: dir.join(format!("{name}.resx")), name, cultures })
}

/// Every resource among `files` (each once), for the Problems check.
pub fn families(files: &[PathBuf]) -> Vec<Family> {
    let mut seen = BTreeMap::new();
    for f in files.iter().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("resx"))) {
        let Some(dir) = f.parent() else { continue };
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = culture_of(&stem).map_or(stem.clone(), |(b, _)| b.to_owned());
        seen.entry(dir.join(&name)).or_insert_with(|| f.clone());
    }
    seen.into_values().filter_map(|f| family(&f)).filter(|f| !f.cultures.is_empty()).collect()
}

/// A key without a value in one language: (key, culture, line of the key in the neutral file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub key: String,
    pub culture: String,
    pub line: usize,
}

/// The keys of the neutral file that a language lacks (no entry, or an empty value).
pub fn missing(neutral: &[Entry], cultures: &[(String, Vec<Entry>)]) -> Vec<Missing> {
    let mut out = Vec::new();
    for e in neutral {
        for (culture, entries) in cultures {
            if !entries.iter().any(|x| x.name == e.name && !x.value.is_empty()) {
                out.push(Missing { key: e.name.clone(), culture: culture.clone(), line: e.line });
            }
        }
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(text: &str) -> String {
    escape(text).replace('"', "&quot;")
}

/// The file's line ending.
fn eol(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Byte ranges of an entry: the whole `<data>` element and its `<value>` / `<comment>` children.
struct Spans {
    data: std::ops::Range<usize>,
    value: Option<std::ops::Range<usize>>,
    comment: Option<std::ops::Range<usize>>,
}

fn spans(text: &str, name: &str) -> anyhow::Result<Option<Spans>> {
    let doc = roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })?;
    let found = doc.root_element().children().find(|n| n.has_tag_name("data") && n.attribute("name") == Some(name));
    Ok(found.map(|data| Spans {
        data: data.range(),
        value: data.children().find(|c| c.has_tag_name("value")).map(|c| c.range()),
        comment: data.children().find(|c| c.has_tag_name("comment")).map(|c| c.range()),
    }))
}

/// The indentation of the line `at` is on.
fn indent_at(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[start..at];
    &line[..line.len() - line.trim_start().len()]
}

/// Sets the value of `name` (adding the entry when the file lacks it).
pub fn set_value(text: &str, name: &str, value: &str) -> anyhow::Result<String> {
    let Some(s) = spans(text, name)? else { return add(text, name, value, None) };
    let element = format!("<value>{}</value>", escape(value));
    Ok(match s.value {
        Some(r) => format!("{}{element}{}", &text[..r.start], &text[r.end..]),
        None => {
            // `<data name="x" />` or one without a value: write it out whole.
            let indent = indent_at(text, s.data.start).to_owned();
            let comment = s.comment.map(|r| text[r].to_owned());
            format!("{}{}{}", &text[..s.data.start], data_element(name, &element, comment.as_deref(), &indent, eol(text)), &text[s.data.end..])
        }
    })
}

fn data_element(name: &str, value_element: &str, comment_element: Option<&str>, indent: &str, nl: &str) -> String {
    let mut out = format!("<data name=\"{}\" xml:space=\"preserve\">{nl}{indent}  {value_element}", escape_attr(name));
    if let Some(c) = comment_element {
        out.push_str(&format!("{nl}{indent}  {c}"));
    }
    out.push_str(&format!("{nl}{indent}</data>"));
    out
}

/// Sets (or with `None` / empty, removes) the comment of `name`. A file without the entry is left as is.
pub fn set_comment(text: &str, name: &str, comment: Option<&str>) -> anyhow::Result<String> {
    let Some(s) = spans(text, name)? else { return Ok(text.to_owned()) };
    let comment = comment.filter(|c| !c.is_empty());
    Ok(match (s.comment, comment) {
        (Some(r), Some(c)) => format!("{}<comment>{}</comment>{}", &text[..r.start], escape(c), &text[r.end..]),
        (Some(r), None) => {
            // With the whitespace before it.
            let start = text[..r.start].trim_end().len();
            format!("{}{}", &text[..start], &text[r.end..])
        }
        (None, Some(c)) => {
            let element = format!("<comment>{}</comment>", escape(c));
            match s.value {
                Some(v) => {
                    let indent = indent_at(text, v.start).to_owned();
                    format!("{}{}{}{element}{}", &text[..v.end], eol(text), indent, &text[v.end..])
                }
                None => {
                    let indent = indent_at(text, s.data.start).to_owned();
                    format!("{}{}{}", &text[..s.data.start], data_element(name, "<value></value>", Some(&element), &indent, eol(text)), &text[s.data.end..])
                }
            }
        }
        (None, None) => text.to_owned(),
    })
}

/// Adds an entry at the end of the file (before `</root>`), indented like the entries before it.
pub fn add(text: &str, name: &str, value: &str, comment: Option<&str>) -> anyhow::Result<String> {
    let doc = roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })?;
    let root = doc.root_element();
    let last = root.children().rfind(|n| n.is_element());
    let nl = eol(text);
    let comment = comment.filter(|c| !c.is_empty()).map(|c| format!("<comment>{}</comment>", escape(c)));
    let element = |indent: &str| data_element(name, &format!("<value>{}</value>", escape(value)), comment.as_deref(), indent, nl);
    Ok(match last {
        Some(last) => {
            let indent = indent_at(text, last.range().start).to_owned();
            let at = last.range().end;
            format!("{}{nl}{indent}{}{}", &text[..at], element(&indent), &text[at..])
        }
        None => {
            // An empty `<root>`: before its end tag (or turn `<root/>` into one with the entry).
            let r = root.range();
            let inner = &text[r.clone()];
            if inner.ends_with("/>") {
                let open = inner.trim_end_matches("/>").trim_end();
                format!("{}{open}>{nl}  {}{nl}</root>{}", &text[..r.start], element("  "), &text[r.end..])
            } else {
                let close = r.start + inner.rfind("</").unwrap_or(inner.len());
                let before = text[..close].trim_end_matches([' ', '\t']);
                format!("{before}  {}{nl}{}", element("  "), &text[close..])
            }
        }
    })
}

/// Removes the entry `name` (with its line); a file without it is left as is.
pub fn remove(text: &str, name: &str) -> anyhow::Result<String> {
    let Some(s) = spans(text, name)? else { return Ok(text.to_owned()) };
    let start = text[..s.data.start].rfind('\n').filter(|&i| text[i + 1..s.data.start].trim().is_empty()).unwrap_or(s.data.start);
    // Drop a `\r` before that newline too.
    let start = if start > 0 && start < s.data.start && text.as_bytes()[start - 1] == b'\r' { start - 1 } else { start };
    Ok(format!("{}{}", &text[..start], &text[s.data.end..]))
}

/// A new language file for `neutral_text`'s resource: the same header and schema, no entries.
pub fn empty_like(neutral_text: &str) -> anyhow::Result<String> {
    let mut text = neutral_text.to_owned();
    for e in parse(neutral_text)? {
        text = remove(&text, &e.name)?;
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESX: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<root>
  <resheader name="resmimetype">
    <value>text/microsoft-resx</value>
  </resheader>
  <data name="Hello" xml:space="preserve">
    <value>Hello &amp; welcome</value>
    <comment>Shown on the start page</comment>
  </data>
  <data name="Bye" xml:space="preserve">
    <value>Goodbye</value>
  </data>
  <data name="Logo" type="System.Resources.ResXFileRef, System.Windows.Forms">
    <value>logo.png;System.Byte[]</value>
  </data>
</root>
"#;

    #[test]
    fn parses_string_entries() {
        let entries = parse(RESX).unwrap();
        assert_eq!(entries.len(), 2, "the file reference isn't a string");
        assert_eq!(entries[0], Entry { name: "Hello".into(), value: "Hello & welcome".into(), comment: Some("Shown on the start page".into()), line: 5 });
        assert_eq!(entries[1].comment, None);
    }

    #[test]
    fn tells_cultures_from_dotted_names() {
        assert_eq!(culture_of("Strings.de"), Some(("Strings", "de")));
        assert_eq!(culture_of("Strings.zh-Hans"), Some(("Strings", "zh-Hans")));
        assert_eq!(culture_of("Strings.fr-CA"), Some(("Strings", "fr-CA")));
        assert_eq!(culture_of("Strings"), None);
        assert_eq!(culture_of("My.Resources"), None, "not a language");
        assert_eq!(culture_of("Errors.en"), Some(("Errors", "en")));
    }

    #[test]
    fn finds_the_family_of_a_file() {
        let dir = std::env::temp_dir().join(format!("bf-resx-family-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["Strings.resx", "Strings.de.resx", "Strings.fr-CA.resx", "Other.resx", "Other.de.resx", "My.Resources.resx"] {
            std::fs::write(dir.join(f), RESX).unwrap();
        }
        let fam = family(&dir.join("Strings.de.resx")).unwrap();
        assert_eq!(fam.name, "Strings");
        assert_eq!(fam.neutral, dir.join("Strings.resx"));
        assert_eq!(fam.cultures.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), ["de", "fr-CA"]);
        let all: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
        let mut names: Vec<String> = families(&all).into_iter().map(|f| f.name).collect();
        names.sort();
        assert_eq!(names, ["Other", "Strings"], "a resource without translations has nothing to check");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edits_touch_only_their_entry() {
        let t = set_value(RESX, "Bye", "See you <soon>").unwrap();
        assert_eq!(t, RESX.replace("<value>Goodbye</value>", "<value>See you &lt;soon&gt;</value>"));
        let t = set_comment(RESX, "Hello", Some("Start page")).unwrap();
        assert_eq!(t, RESX.replace("Shown on the start page", "Start page"));
        let t = set_comment(RESX, "Hello", None).unwrap();
        assert_eq!(t, RESX.replace("\n    <comment>Shown on the start page</comment>", ""));
        let t = set_comment(RESX, "Bye", Some("Footer")).unwrap();
        assert_eq!(t, RESX.replace("<value>Goodbye</value>", "<value>Goodbye</value>\n    <comment>Footer</comment>"));
        assert_eq!(parse(&t).unwrap()[1].comment.as_deref(), Some("Footer"));
    }

    #[test]
    fn adds_and_removes_entries() {
        let t = add(RESX, "New", "Neu", Some("a note")).unwrap();
        let entries = parse(&t).unwrap();
        assert_eq!(entries.last().map(|e| (e.name.as_str(), e.value.as_str(), e.comment.as_deref())), Some(("New", "Neu", Some("a note"))));
        assert!(t.contains("  </data>\n  <data name=\"New\" xml:space=\"preserve\">\n    <value>Neu</value>\n    <comment>a note</comment>\n  </data>\n</root>"), "{t}");
        let back = remove(&t, "New").unwrap();
        assert_eq!(back, RESX);
        let t = remove(RESX, "Hello").unwrap();
        assert!(!t.contains("Hello"));
        assert!(t.contains("  </resheader>\n  <data name=\"Bye\""), "{t}");
        // Setting a value the file lacks adds the entry.
        let t = set_value(RESX, "Fresh", "x").unwrap();
        assert_eq!(parse(&t).unwrap().len(), 3);
    }

    #[test]
    fn keeps_crlf_and_fills_an_empty_file() {
        let crlf = RESX.replace('\n', "\r\n");
        let t = add(&crlf, "New", "v", None).unwrap();
        assert!(!t.replace("\r\n", "").contains('\n'), "only CRLF line ends");
        assert_eq!(remove(&t, "New").unwrap(), crlf);
        let empty = empty_like(RESX).unwrap();
        assert!(parse(&empty).unwrap().is_empty());
        assert!(empty.contains("<resheader"), "the header stays");
        assert!(empty.contains("Logo"), "non-string entries stay");
        let t = add("<?xml version=\"1.0\"?>\n<root>\n</root>\n", "A", "1", None).unwrap();
        assert_eq!(parse(&t).unwrap()[0].value, "1");
        let t = add("<root/>", "A", "1", None).unwrap();
        assert_eq!(parse(&t).unwrap()[0].value, "1");
    }

    #[test]
    fn lists_missing_translations() {
        let neutral = parse(RESX).unwrap();
        let de = parse(&set_value(&remove(RESX, "Bye").unwrap(), "Hello", "Hallo").unwrap()).unwrap();
        let fr = parse(&set_value(RESX, "Hello", "").unwrap()).unwrap();
        let m = missing(&neutral, &[("de".into(), de), ("fr".into(), fr)]);
        assert_eq!(m, vec![Missing { key: "Hello".into(), culture: "fr".into(), line: 5 }, Missing { key: "Bye".into(), culture: "de".into(), line: 9 }]);
    }
}
