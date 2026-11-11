//! Small line-based languages built into ByteForge, highlighted with regexes like module languages:
//! ignore files (`.gitignore`, `.dockerignore`, …), `.gitattributes`, INI-style config (`.editorconfig`,
//! `.gitconfig`, `.ini`, …) and `.env` files. A module language that claims a file still wins.

use std::sync::OnceLock;

use crate::module_lang::{ModuleLanguage, RegexRule};

struct Def {
    id: &'static str,
    /// Extra ids for fenced code blocks (```` ```env ````).
    aliases: &'static [&'static str],
    name: &'static str,
    line_comment: Option<&'static str>,
    matches: fn(&str) -> bool,
    /// (pattern, capture group, token); for overlapping matches the earlier rule wins.
    rules: &'static [(&'static str, usize, &'static str)],
}

const DEFS: &[Def] = &[
    Def {
        id: "ignore",
        aliases: &["gitignore", "dockerignore"],
        name: "Ignore",
        line_comment: Some("#"),
        matches: is_ignore_file,
        rules: &[
            (r"^#.*$", 0, "comment"),
            (r"\\.", 0, "string - escape character"),
            (r"^!", 0, "operator"),
            (r"\[(?:[^\]\\]|\\.)*\]", 0, "string"),
            (r"\*\*|[*?]", 0, "operator"),
            (r"/", 0, "punctuation"),
        ],
    },
    Def {
        id: "gitattributes",
        aliases: &[],
        name: "Git Attributes",
        line_comment: Some("#"),
        matches: |name| name == ".gitattributes" || ext(name) == Some("gitattributes"),
        rules: &[
            (r"^\s*#.*$", 0, "comment"),
            (r"^\s*(\[attr\])", 1, "keyword"),
            (r"^\s*\[attr\](\S+)", 1, "label name"),
            (r#"^\s*("(?:[^"\\]|\\.)*"|\S+)"#, 1, "string"),
            (r"(?:^|\s)([-!])[A-Za-z_]", 1, "operator"),
            (r"(?:^|\s)[-!]?([A-Za-z_][\w.-]*)", 1, "property name"),
            (r"=", 0, "operator"),
            (r"([^=\s]+)", 1, "constant name"),
        ],
    },
    Def {
        id: "ini",
        aliases: &["editorconfig", "gitconfig", "cfg", "desktop", "systemd"],
        name: "INI",
        line_comment: Some("#"),
        matches: is_ini_file,
        rules: &[
            (r"^\s*[#;].*$", 0, "comment"),
            (r"^\s*(\[)", 1, "punctuation"),
            (r"^\s*\[[^\]]*(\])", 1, "punctuation"),
            (r"^\s*\[([^\]]*)\]", 1, "class name"),
            (r"^\s*([^=:\s\[#;][^=:]*?)\s*[=:]", 1, "property name"),
            (r"^\s*[^=:\s\[#;][^=:]*?\s*([=:])", 1, "operator"),
            (r#""(?:[^"\\]|\\.)*"?"#, 0, "string"),
            (r"\\.", 0, "string - escape character"),
            (r"\s([#;].*)$", 1, "comment"),
            (r"(?i)[=:]\s*(true|false|yes|no|on|off)\s*$", 1, "keyword"),
            (r"[=:]\s*(-?\d+(?:\.\d+)?)\s*$", 1, "number"),
        ],
    },
    Def {
        id: "dotenv",
        aliases: &["env"],
        name: "Environment Variables",
        line_comment: Some("#"),
        matches: |name| name == ".env" || name.starts_with(".env.") || ext(name) == Some("env"),
        rules: &[
            (r"^\s*#.*$", 0, "comment"),
            (r"^\s*(export)\s", 1, "keyword"),
            (r"^\s*(?:export\s+)?([A-Za-z_][\w.-]*)\s*=", 1, "property name"),
            (r"^\s*(?:export\s+)?[A-Za-z_][\w.-]*\s*(=)", 1, "operator"),
            (r"'[^']*'?", 0, "string"),
            (r"\\.", 0, "string - escape character"),
            (r"\$\{[^}]*\}?|\$[A-Za-z_]\w*", 0, "local name"),
            (r#""(?:[^"\\]|\\.)*"?"#, 0, "string"),
            (r"\s(#.*)$", 1, "comment"),
        ],
    },
];

fn ext(name: &str) -> Option<&str> {
    name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map(|(_, e)| e)
}

/// `.gitignore`, `.dockerignore`, `.npmignore`, `.prettierignore`, … and `*.gitignore` templates.
fn is_ignore_file(name: &str) -> bool {
    let dotted = name.strip_prefix('.').is_some_and(|rest| rest.ends_with("ignore") && !rest.contains('.'));
    dotted || ext(name).is_some_and(|e| e.eq_ignore_ascii_case("gitignore"))
}

fn is_ini_file(name: &str) -> bool {
    const NAMES: &[&str] = &[".editorconfig", ".gitconfig", ".gitmodules", ".npmrc", ".pypirc", ".flake8", ".pylintrc", ".coveragerc"];
    const EXTENSIONS: &[&str] = &["ini", "cfg", "editorconfig", "gitconfig", "desktop", "service", "timer", "socket", "mount"];
    NAMES.contains(&name) || ext(name).is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn languages() -> &'static [ModuleLanguage] {
    static LANGUAGES: OnceLock<Vec<ModuleLanguage>> = OnceLock::new();
    LANGUAGES.get_or_init(|| {
        DEFS.iter()
            .map(|d| ModuleLanguage {
                id: d.id,
                name: d.name,
                line_comment: d.line_comment,
                matches: Box::new(d.matches),
                rules: d.rules.iter().filter_map(|&(pattern, group, token)| RegexRule::new(pattern, group, token)).collect(),
            })
            .collect()
    })
}

pub(crate) fn get(index: u8) -> Option<&'static ModuleLanguage> {
    languages().get(index as usize)
}

/// The built-in language for a file name, as its index.
pub(crate) fn find(file_name: &str) -> Option<u8> {
    languages().iter().position(|l| (l.matches)(file_name)).map(|i| i as u8)
}

pub(crate) fn find_id(id: &str) -> Option<u8> {
    DEFS.iter().position(|d| d.id == id || d.aliases.contains(&id)).map(|i| i as u8)
}
