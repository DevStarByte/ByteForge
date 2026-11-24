//! The form's code file (`Form1.cs`): finding it and its `.Designer.cs`, its base class, and adding event
//! handlers to it.

use std::path::{Path, PathBuf};

use super::lex;

/// The code file and the designer file of a form, from either of them: `Form1.cs` ↔ `Form1.Designer.cs`.
/// `None` when the designer file doesn't exist or has no InitializeComponent.
pub fn designer_pair(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let name = path.file_name()?.to_str()?;
    let lower = name.to_ascii_lowercase();
    let (code, designer) = if lower.ends_with(".designer.cs") {
        let stem = &name[..name.len() - ".designer.cs".len()];
        (path.with_file_name(format!("{stem}.cs")), path.to_path_buf())
    } else if lower.ends_with(".cs") {
        let stem = &name[..name.len() - 3];
        (path.to_path_buf(), path.with_file_name(format!("{stem}.Designer.cs")))
    } else {
        return None;
    };
    let text = std::fs::read_to_string(&designer).ok()?;
    text.contains("InitializeComponent").then_some((code, designer))
}

/// The base class of `class_name` in its code file (`Form`, `UserControl`), without namespace.
pub fn base_type(code: &str, class_name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(p) = code[from..].find("class ") {
        let at = from + p + "class ".len();
        from = at;
        let rest = code[at..].trim_start();
        let Some(after) = rest.strip_prefix(class_name) else { continue };
        if after.starts_with(lex::is_ident_char) {
            continue;
        }
        let after = after.trim_start();
        let bases = after.strip_prefix(':')?;
        let first: String = bases.trim_start().chars().take_while(|c| lex::is_ident_char(*c) || *c == '.').collect();
        return Some(first.rsplit('.').next().unwrap_or("").to_owned()).filter(|s| !s.is_empty());
    }
    None
}

/// The line (0-based) of a method's declaration, if the code has it.
pub fn method_line(code: &str, method: &str) -> Option<usize> {
    for (ix, line) in code.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        if let Some(p) = t.find(method) {
            let before = t[..p].trim_end();
            let after = t[p + method.len()..].trim_start();
            if after.starts_with('(') && before.ends_with("void") {
                return Some(ix);
            }
        }
    }
    None
}

/// Adds an event handler to `class_name`: `private void button1_Click(object sender, EventArgs e) { }` at the
/// end of the class. Returns the new code (unchanged when the method is there already) and where to put the
/// caret: (line, column), 0-based, in its body.
pub fn add_handler(code: &str, class_name: &str, method: &str, args: &str) -> Option<(String, usize, usize)> {
    if let Some(line) = method_line(code, method) {
        let body = code.lines().skip(line).position(|l| l.contains('{')).map_or(line, |p| line + p + 1);
        let col = code.lines().nth(body).map_or(0, |l| l.len() - l.trim_start().len());
        return Some((code.to_owned(), body, col));
    }
    let mut from = 0;
    let open = loop {
        let p = code[from..].find("class ")? + from;
        from = p + 6;
        let rest = code[from..].trim_start();
        if rest.strip_prefix(class_name).is_some_and(|a| !a.starts_with(lex::is_ident_char)) {
            break code[from..].find('{')? + from;
        }
    };
    let close = lex::matching_brace(code, open)?;
    let class_indent: String = {
        let start = code[..open].rfind('\n').map_or(0, |p| p + 1);
        code[start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    };
    // The indentation of the class's members, from its first indented line (else one level more).
    let member_indent = code[open + 1..close]
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').collect::<String>())
        .filter(|i| i.len() > class_indent.len())
        .unwrap_or_else(|| format!("{class_indent}    "));
    let line_start = code[..close].rfind('\n').map_or(0, |p| p + 1);
    let (insert_at, lead) = if code[line_start..close].trim().is_empty() { (line_start, "") } else { (close, "\n") };
    let has_members = !code[open + 1..insert_at].trim().is_empty();
    let blank = if has_members { "\n" } else { "" };
    // The body's line is indented, so typing starts at the right column.
    let unit = &member_indent[class_indent.len().min(member_indent.len())..];
    let body_indent = format!("{member_indent}{}", if unit.is_empty() { "    " } else { unit });
    // Handlers written for nullable reference types (`object? sender`) get the same.
    let sender = if code.contains("object? sender") { "object?" } else { "object" };
    let method_text = format!("{lead}{blank}{member_indent}private void {method}({sender} sender, {args} e)\n{member_indent}{{\n{body_indent}\n{member_indent}}}\n");
    let mut out = String::with_capacity(code.len() + method_text.len());
    out.push_str(&code[..insert_at]);
    out.push_str(&method_text);
    out.push_str(&code[insert_at..]);
    let line = out[..insert_at].matches('\n').count() + method_text.matches('\n').count() - 2;
    Some((out, line, body_indent.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "namespace App\n{\n    public partial class Form1 : Form\n    {\n        public Form1()\n        {\n            InitializeComponent();\n        }\n    }\n}\n";

    #[test]
    fn adds_a_handler_at_the_end_of_the_class() {
        let (code, line, col) = add_handler(CODE, "Form1", "button1_Click", "EventArgs").unwrap();
        assert_eq!(
            code,
            "namespace App\n{\n    public partial class Form1 : Form\n    {\n        public Form1()\n        {\n            InitializeComponent();\n        }\n\n        private void button1_Click(object sender, EventArgs e)\n        {\n            \n        }\n    }\n}\n"
        );
        assert_eq!((code.lines().nth(line), col), (Some("            "), 12), "the caret goes into the body");
        assert_eq!(code.lines().nth(line - 1).map(str::trim), Some("{"));
        // Asked again: the method is there; the caret goes to it.
        let (again, at, _) = add_handler(&code, "Form1", "button1_Click", "EventArgs").unwrap();
        assert_eq!((again, at), (code.clone(), line));
    }

    #[test]
    fn file_scoped_namespaces_and_base_types() {
        let code = "namespace App;\n\npublic partial class Main : System.Windows.Forms.UserControl\n{\n}\n";
        assert_eq!(base_type(code, "Main").as_deref(), Some("UserControl"));
        assert_eq!(base_type(CODE, "Form1").as_deref(), Some("Form"));
        let (out, line, col) = add_handler(code, "Main", "Main_Load", "EventArgs").unwrap();
        assert_eq!(out, "namespace App;\n\npublic partial class Main : System.Windows.Forms.UserControl\n{\n    private void Main_Load(object sender, EventArgs e)\n    {\n        \n    }\n}\n");
        assert_eq!((out.lines().nth(line), col), (Some("        "), 8));
        // A class whose handlers take `object? sender` gets one like them.
        let nullable = out.replace("object sender", "object? sender");
        let (out, _, _) = add_handler(&nullable, "Main", "Main_Paint", "PaintEventArgs").unwrap();
        assert!(out.contains("    private void Main_Paint(object? sender, PaintEventArgs e)\n"), "{out}");
    }
}
