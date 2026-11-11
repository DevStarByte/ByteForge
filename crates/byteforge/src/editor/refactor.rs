//! Refactoring: the Refactor menu (Ctrl+Shift+R), one command per refactoring (Extract Method, Introduce
//! Variable, Inline, …), renaming the name a refactoring made up, and sorting `using` directives.
//!
//! The refactorings themselves come from the language server (Roslyn's, through csharp-ls) as code actions;
//! the editor asks for the ones at the selection and keeps those the command is about.

use super::*;

/// A refactoring with its own command. Each matches the code actions whose title starts with one of its
/// [`prefixes`](Refactoring::prefixes) (Roslyn sends refactorings without a code action kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refactoring {
    ExtractMethod,
    ExtractLocalFunction,
    IntroduceVariable,
    IntroduceConstant,
    IntroduceField,
    IntroduceParameter,
    Inline,
    ExtractInterface,
    ExtractBaseClass,
    MoveTypeToFile,
    RemoveAndSortUsings,
}

impl Refactoring {
    fn prefixes(self) -> &'static [&'static str] {
        match self {
            Self::ExtractMethod => &["Extract method"],
            Self::ExtractLocalFunction => &["Extract local function"],
            Self::IntroduceVariable => &["Introduce local", "Introduce 'var'", "Deconstruct"],
            Self::IntroduceConstant => &["Introduce constant", "Introduce local constant"],
            Self::IntroduceField => &["Introduce field", "Introduce readonly field"],
            Self::IntroduceParameter => &["Introduce parameter"],
            Self::Inline => &["Inline"],
            Self::ExtractInterface => &["Extract interface"],
            Self::ExtractBaseClass => &["Extract base class"],
            Self::MoveTypeToFile => &["Move type to", "Rename file to", "Rename type to"],
            Self::RemoveAndSortUsings => &["Remove unnecessary usings", "Remove Unnecessary Usings"],
        }
    }

    pub fn matches(self, action: &bf_lsp::CodeAction) -> bool {
        // "Introduce local constant" is a constant, not a local variable.
        if self == Self::IntroduceVariable && action.title.starts_with("Introduce local constant") {
            return false;
        }
        self.prefixes().iter().any(|p| action.title.starts_with(p))
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ExtractMethod => "Extract Method",
            Self::ExtractLocalFunction => "Extract Local Function",
            Self::IntroduceVariable => "Introduce Local Variable",
            Self::IntroduceConstant => "Introduce Constant",
            Self::IntroduceField => "Introduce Field",
            Self::IntroduceParameter => "Introduce Parameter",
            Self::Inline => "Inline",
            Self::ExtractInterface => "Extract Interface",
            Self::ExtractBaseClass => "Extract Base Class",
            Self::MoveTypeToFile => "Move Type to Matching File",
            Self::RemoveAndSortUsings => "Remove and Sort Usings",
        }
    }

    /// What to do when the refactoring isn't offered where the caret is.
    fn hint(self) -> &'static str {
        match self {
            Self::ExtractMethod | Self::ExtractLocalFunction => "Select the statements or the expression to extract.",
            Self::IntroduceVariable | Self::IntroduceConstant | Self::IntroduceField | Self::IntroduceParameter => "Select an expression.",
            Self::Inline => "Put the caret on a local variable, or on a call of the method to inline.",
            Self::ExtractInterface | Self::ExtractBaseClass => "Put the caret on the name of a class.",
            Self::MoveTypeToFile => "Put the caret on the name of a type whose file has another name.",
            Self::RemoveAndSortUsings => "",
        }
    }

    /// Whether the refactoring is offered on an empty selection (the others need code selected).
    fn needs_selection(self) -> bool {
        matches!(self, Self::ExtractMethod | Self::ExtractLocalFunction)
    }
}

/// What the code-action menu lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MenuMode {
    /// Ctrl+.: fixes first, then refactorings.
    QuickFix,
    /// Ctrl+Shift+R: refactorings only (and Rename), grouped.
    Refactor,
    /// One refactoring's command: applied at once when the server offers exactly one variant.
    Only(Refactoring),
}

/// Done after a code action's edit is applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AfterApply {
    #[default]
    Nothing,
    /// Sort the `using` directives (after "Remove unnecessary usings").
    SortUsings,
}

/// Code-action kind of the Refactor menu's own Rename entry.
const RENAME_KIND: &str = "byteforge.rename";

/// Order of the groups in the Refactor menu.
const GROUP_ORDER: &[&str] = &["Rename", "Extract", "Introduce", "Inline", "Move", "Generate", "Rewrite", "Source", "Fix"];

pub(super) fn group_rank(action: &bf_lsp::CodeAction) -> usize {
    let group = group_of(action);
    GROUP_ORDER.iter().position(|g| *g == group).unwrap_or(GROUP_ORDER.len())
}

pub(super) fn group_of(action: &bf_lsp::CodeAction) -> &'static str {
    match action.kind.as_deref() {
        Some(RENAME_KIND) => "Rename",
        Some(k) if k.starts_with("byteforge.merge.") => "Merge",
        _ => action.group(),
    }
}

/// Whether the action makes up a name (`NewMethod`, `value`, `IFoo`) that the user should get to pick.
pub fn invents_name(title: &str) -> bool {
    ["Extract method", "Extract local function", "Extract interface", "Extract base class", "Introduce "].iter().any(|p| title.starts_with(p))
}

impl EditorView {
    /// The commands: Refactor… opens the menu; the others ask for their refactoring.
    pub(super) fn refactor(&mut self, mode: MenuMode, cx: &mut Context<Self>) {
        if self.lsp.is_none() || self.read_only || self.path.is_none() {
            if mode == MenuMode::Only(Refactoring::RemoveAndSortUsings) {
                self.sort_usings(cx);
            } else {
                cx.emit(EditorEvent::Log("Refactoring needs the C# language server (open a .cs file in a project).".into()));
            }
            return;
        }
        if let MenuMode::Only(r) = mode
            && r.needs_selection()
            && !self.state.has_selection()
        {
            // Without a selection, Roslyn extracts nothing; take the caret's statement line instead.
            let line = self.state.caret().line;
            let text = self.state.buffer.line(line);
            let start = text.chars().take_while(|c| c.is_whitespace()).count();
            let end = text.trim_end().chars().count();
            if end <= start {
                self.show_menu_message(r.hint(), cx);
                return;
            }
            self.state.select(Point::new(line, start), Point::new(line, end));
            self.after_move(cx);
        }
        self.request_code_actions(mode, cx);
    }

    /// Opens the action menu with only a message in it (no request).
    fn show_menu_message(&mut self, text: &str, cx: &mut Context<Self>) {
        let at = self.state.caret();
        self.action_menu = Some(ActionMenu { at, items: Some(Vec::new()), selected: 0, mode: MenuMode::Refactor, empty: Some(text.to_owned()) });
        cx.notify();
    }

    /// Keeps the server's actions this menu is about, in menu order. With one variant of a single
    /// refactoring, applies it right away and returns `None`.
    pub(super) fn filter_actions(&mut self, mode: MenuMode, mut items: Vec<bf_lsp::CodeAction>, cx: &mut Context<Self>) -> Option<(Vec<bf_lsp::CodeAction>, Option<String>)> {
        match mode {
            MenuMode::QuickFix => {
                // Preferred fixes first, then other fixes, then refactorings and source actions.
                items.sort_by_key(|a| (!a.preferred, !a.is_quick_fix(), a.disabled.is_some()));
                Some((items, None))
            }
            MenuMode::Refactor => {
                items.retain(|a| !a.is_quick_fix());
                if let Some(rename) = self.rename_entry() {
                    items.insert(0, rename);
                }
                items.sort_by_key(|a| (group_rank(a), a.disabled.is_some()));
                let empty = (!self.state.has_selection()).then(|| "No refactorings here. Select code to extract or introduce it.".to_owned());
                Some((items, empty))
            }
            MenuMode::Only(r) => {
                items.retain(|a| r.matches(a));
                let then = if r == Refactoring::RemoveAndSortUsings { AfterApply::SortUsings } else { AfterApply::Nothing };
                let enabled: Vec<&bf_lsp::CodeAction> = items.iter().filter(|a| a.disabled.is_none()).collect();
                if enabled.len() == 1 {
                    let action = enabled[0].clone();
                    self.action_menu = None;
                    cx.emit(EditorEvent::ApplyCodeAction(Box::new(action), then));
                    return None;
                }
                if items.is_empty() && r == Refactoring::RemoveAndSortUsings {
                    // Nothing unused: sorting is all there is to do.
                    self.action_menu = None;
                    self.sort_usings(cx);
                    return None;
                }
                Some((items, Some(format!("{} isn't available here. {}", r.label(), r.hint()))))
            }
        }
    }

    /// The Refactor menu's Rename entry, when the caret is on a name.
    fn rename_entry(&self) -> Option<bf_lsp::CodeAction> {
        let (start, end) = self.state.word_at(self.state.caret());
        let word = self.state.buffer.slice(start, end);
        let is_name = word.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') && word.chars().all(|c| c.is_alphanumeric() || c == '_');
        is_name.then(|| bf_lsp::CodeAction {
            title: format!("Rename '{word}'…"),
            kind: Some(RENAME_KIND.into()),
            preferred: false,
            disabled: None,
            edit: None,
            command: None,
            raw: serde_json::Value::Null,
        })
    }

    /// Runs the picked menu entry; `false` when it is one of the server's code actions.
    pub(super) fn run_builtin_entry(&mut self, action: &bf_lsp::CodeAction, cx: &mut Context<Self>) -> bool {
        if action.kind.as_deref() == Some(RENAME_KIND) {
            self.request_rename(cx);
            return true;
        }
        action.kind.as_deref().is_some_and(|kind| self.run_conflict_entry(kind, cx))
    }

    /// After a refactoring made up `name`: puts the caret on it (on `line`, near `col`) and asks for the
    /// real name, like Visual Studio does after Extract Method.
    pub fn rename_generated(&mut self, name: &str, line: usize, col: usize, cx: &mut Context<Self>) {
        let Some(at) = find_word_near(&self.state.buffer, name, line, col) else { return };
        let end = Point::new(at.line, at.col + name.chars().count());
        self.state.select(at, end);
        self.after_move(cx);
        self.request_rename(cx);
    }

    /// Sorts the `using` directives at the top of the file (each blank-line separated group on its own):
    /// `System` first, then alphabetically; `using static` and aliases after the namespaces. One undo step.
    pub fn sort_usings(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let text = self.state.buffer.text();
        let Some((first, last, sorted)) = sort_usings_block(&text) else { return };
        let end = Point::new(last, self.state.buffer.line(last).chars().count());
        let caret = self.state.caret();
        self.state.buffer.undo_stack().begin_group();
        self.state.buffer.replace(Point::new(first, 0), end, &sorted);
        self.state.buffer.undo_stack().end_group();
        self.state.set_caret(self.state.buffer.clamp(caret), false);
        self.after_edit(cx);
        cx.notify();
    }
}

/// The whole-word occurrence of `name` closest to (`line`, `col`), looking a few lines around `line`.
fn find_word_near(buffer: &Buffer, name: &str, line: usize, col: usize) -> Option<Point> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let last = buffer.line_count().saturating_sub(1);
    let mut best: Option<(usize, Point)> = None;
    for l in line.saturating_sub(3)..=(line + 3).min(last) {
        let text: Vec<char> = buffer.line(l).chars().collect();
        let needle: Vec<char> = name.chars().collect();
        if needle.is_empty() || text.len() < needle.len() {
            continue;
        }
        for c in 0..=text.len() - needle.len() {
            if text[c..c + needle.len()] != needle[..] || (c > 0 && is_word(text[c - 1])) || text.get(c + needle.len()).is_some_and(|&ch| is_word(ch)) {
                continue;
            }
            let distance = l.abs_diff(line) * 10_000 + c.abs_diff(col);
            if best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, Point::new(l, c)));
            }
        }
    }
    best.map(|(_, p)| p)
}

const CSHARP_KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "base", "bool", "byte", "char", "class", "const", "decimal", "default", "double", "dynamic", "else",
    "false", "float", "for", "foreach", "global", "if", "in", "int", "interface", "internal", "is", "long", "namespace", "new", "null",
    "object", "out", "override", "params", "partial", "private", "protected", "public", "readonly", "record", "ref", "return", "sealed",
    "static", "string", "struct", "this", "throw", "true", "uint", "ulong", "using", "var", "virtual", "void", "where", "yield",
];

fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_'))
}

/// The name a refactoring made up, and where it ends up (0-based line, UTF-16 column of the edit), found
/// in its edits to the file whose text was `old`: the last new identifier of the shortest edit that adds
/// one. For Extract Method that is the call (`NewMethod`), for Introduce Local the declaration
/// (`… value = `), for Extract Interface the base list (` : IFoo`).
pub fn generated_name(old: &str, edits: &[bf_lsp::TextEdit]) -> Option<(String, usize, usize)> {
    let known: std::collections::HashSet<&str> = identifiers(old).collect();
    let mut order: Vec<&bf_lsp::TextEdit> = edits.iter().collect();
    order.sort_by_key(|e| (e.new_text.len(), e.range.start.line, e.range.start.character));
    for edit in order {
        let Some(name) = identifiers(&edit.new_text).filter(|w| !known.contains(w) && !CSHARP_KEYWORDS.contains(w)).last() else { continue };
        let start = (edit.range.start.line, edit.range.start.character);
        // Lines added or removed by the edits before this one move it.
        let shift: i64 = edits
            .iter()
            .filter(|e| (e.range.start.line, e.range.start.character) < start)
            .map(|e| e.new_text.matches('\n').count() as i64 - (e.range.end.line - e.range.start.line) as i64)
            .sum();
        let before = &edit.new_text[..edit.new_text.find(name).unwrap_or(0)];
        let (line, col) = match before.rfind('\n') {
            Some(nl) => (start.0 as i64 + before.matches('\n').count() as i64, before[nl + 1..].encode_utf16().count()),
            None => (start.0 as i64, start.1 as usize + before.encode_utf16().count()),
        };
        return Some((name.to_owned(), (line + shift).max(0) as usize, col));
    }
    None
}

/// The first block of `using` directives sorted: `(first line, last line, sorted text)`, or `None` when
/// there is none or it is sorted already. Comments, `#` directives and blank lines may come before it.
pub fn sort_usings_block(text: &str) -> Option<(usize, usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let is_using = |l: &str| {
        let t = l.trim();
        let t = t.strip_prefix("global ").unwrap_or(t);
        t.starts_with("using ") && t.ends_with(';') && !t.contains('(') && !t.starts_with("using var ")
    };
    let mut i = 0;
    let mut in_comment = false;
    while i < lines.len() {
        let t = lines[i].trim();
        if in_comment || t.starts_with("/*") {
            in_comment = !t.contains("*/");
        } else if !(t.is_empty() || t.starts_with("//") || t.starts_with('#')) {
            break;
        }
        i += 1;
    }
    if i >= lines.len() || !is_using(lines[i]) {
        return None;
    }
    let first = i;
    let mut last = i;
    while i < lines.len() && (is_using(lines[i]) || lines[i].trim().is_empty()) {
        if is_using(lines[i]) {
            last = i;
        }
        i += 1;
    }
    let block = &lines[first..=last];
    let mut out: Vec<String> = Vec::new();
    for group in block.split(|l| l.trim().is_empty()) {
        if !out.is_empty() {
            out.push(String::new());
        }
        let mut group: Vec<&str> = group.to_vec();
        group.sort_by_key(|l| using_key(l));
        group.dedup_by_key(|l| l.trim().to_owned());
        out.extend(group.iter().map(|l| l.to_string()));
    }
    let sorted = out.join("\n");
    (sorted != block.join("\n")).then_some((first, last, sorted))
}

/// Sort key of a using directive: global ones first, then namespaces (`System` before the rest), then
/// `using static`, then aliases; alphabetically within each.
fn using_key(line: &str) -> (u8, u8, u8, String) {
    let t = line.trim();
    let (global, t) = match t.strip_prefix("global ") {
        Some(rest) => (0, rest.trim_start()),
        None => (1, t),
    };
    let t = t.trim_start_matches("using ").trim_end_matches(';').trim();
    let (kind, name) = if let Some(rest) = t.strip_prefix("static ") {
        (1, rest.trim())
    } else if t.contains('=') {
        (2, t)
    } else {
        (0, t)
    };
    let system = u8::from(!(name == "System" || name.starts_with("System.")));
    (global, kind, if kind == 0 { system } else { 0 }, name.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bf_lsp::{Position, Range, TextEdit};

    fn edit(line: u32, ch: u32, end_line: u32, end_ch: u32, text: &str) -> TextEdit {
        TextEdit { range: Range { start: Position { line, character: ch }, end: Position { line: end_line, character: end_ch } }, new_text: text.into() }
    }

    const OLD: &str = "class Program\n{\n    static void Main()\n    {\n        Console.WriteLine(greeting);\n    }\n}\n";

    #[test]
    fn finds_the_name_extract_method_made_up() {
        // csharp-ls: the call replaces `Console.WriteLine`, the method goes in before the closing brace.
        let edits = [
            edit(4, 8, 4, 25, "NewMethod"),
            edit(6, 0, 6, 0, "private static void NewMethod(global::System.String greeting)\n    {\n        Console.WriteLine(greeting);\n    }\n\n"),
        ];
        assert_eq!(generated_name(OLD, &edits), Some(("NewMethod".into(), 4, 8)));
    }

    #[test]
    fn finds_the_name_introduce_local_made_up() {
        let edits = [edit(4, 8, 4, 8, "global::System.Object value = ")];
        assert_eq!(generated_name(OLD, &edits), Some(("value".into(), 4, 30)));
    }

    #[test]
    fn earlier_edits_that_add_lines_move_the_name() {
        let edits = [edit(0, 0, 0, 0, "interface IProgram\n{\n}\n\n"), edit(0, 13, 0, 13, " : IProgram")];
        assert_eq!(generated_name(OLD, &edits), Some(("IProgram".into(), 4, 16)));
        let buffer = Buffer::new("interface IProgram\n{\n}\n\nclass Program : IProgram\n{\n}\n");
        assert_eq!(find_word_near(&buffer, "IProgram", 4, 16), Some(Point::new(4, 16)));
    }

    #[test]
    fn no_name_when_nothing_new() {
        assert_eq!(generated_name(OLD, &[edit(4, 8, 4, 25, "Console.WriteLine")]), None);
    }

    #[test]
    fn whole_words_only() {
        let buffer = Buffer::new("var values = value + 1;\n");
        assert_eq!(find_word_near(&buffer, "value", 0, 0), Some(Point::new(0, 13)));
    }

    #[test]
    fn sorts_usings_system_first_then_static_and_aliases() {
        let text = "// header\nusing Zeta;\nusing static System.Math;\nusing System.Linq;\nusing Alpha;\nusing IO = System.IO;\nusing System;\n\nnamespace Demo;\n";
        let (first, last, sorted) = sort_usings_block(text).unwrap();
        assert_eq!((first, last), (1, 6));
        assert_eq!(sorted, "using System;\nusing System.Linq;\nusing Alpha;\nusing Zeta;\nusing static System.Math;\nusing IO = System.IO;");
    }

    #[test]
    fn keeps_groups_and_drops_duplicates() {
        let text = "using B;\nusing A;\nusing A;\n\nusing D;\nusing C;\nclass X {}\n";
        let (_, last, sorted) = sort_usings_block(text).unwrap();
        assert_eq!(last, 5);
        assert_eq!(sorted, "using A;\nusing B;\n\nusing C;\nusing D;");
    }

    #[test]
    fn sorted_or_missing_usings_change_nothing() {
        assert_eq!(sort_usings_block("using System;\nusing Foo;\n\nclass X {}\n"), None);
        assert_eq!(sort_usings_block("class X { void F() { using var s = Open(); } }\n"), None);
        assert_eq!(sort_usings_block(""), None);
    }

    #[test]
    fn global_usings_come_first() {
        let (_, _, sorted) = sort_usings_block("using Foo;\nglobal using System;\n").unwrap();
        assert_eq!(sorted, "global using System;\nusing Foo;");
    }

    #[test]
    fn commands_match_roslyn_titles() {
        let action = |title: &str| bf_lsp::CodeAction { title: title.into(), kind: None, preferred: false, disabled: None, edit: None, command: None, raw: serde_json::Value::Null };
        assert!(Refactoring::ExtractMethod.matches(&action("Extract method")));
        assert!(!Refactoring::ExtractMethod.matches(&action("Extract local function")));
        assert!(Refactoring::IntroduceVariable.matches(&action("Introduce local for 'a + b'")));
        assert!(!Refactoring::IntroduceVariable.matches(&action("Introduce local constant for '1'")));
        assert!(Refactoring::IntroduceConstant.matches(&action("Introduce local constant for '1'")));
        assert!(Refactoring::Inline.matches(&action("Inline temporary variable")));
        assert!(Refactoring::ExtractInterface.matches(&action("Extract interface...")));
        assert!(Refactoring::MoveTypeToFile.matches(&action("Move type to Foo.cs")));
        assert!(invents_name("Extract method") && invents_name("Introduce local for 'x'") && !invents_name("Inline temporary variable"));
    }
}
