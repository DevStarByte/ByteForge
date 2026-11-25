//! Items of strips, menus and grids: menu items (`menuStrip1.Items.AddRange(new ToolStripItem[] { … })`,
//! `fileToolStripMenuItem.DropDownItems.AddRange(…)`) and grid columns (`dataGridView1.Columns.AddRange(…)`).
//! Each item is a component of its own, set up in its own block; the owner's `AddRange` statement lists
//! them in order.

use std::ops::Range;

use super::*;

/// The names an `AddRange(new T[] { a, b })` statement lists, the byte range between its braces, and
/// whether the list writes `this.`.
fn list_of(text: &str) -> Option<(Vec<String>, Range<usize>, bool)> {
    let open = text.find('{')?;
    let close = text.rfind('}')?;
    if close <= open {
        return None;
    }
    let inner = &text[open + 1..close];
    let names = lex::split_args(inner).iter().map(|a| strip_this(a).to_owned()).filter(|a| !a.is_empty()).collect();
    Some((names, open + 1..close, inner.contains("this.")))
}

/// The statement with its list replaced by `names` (on one line).
fn with_list(text: &str, range: Range<usize>, names: &[String], this: bool) -> String {
    let list: Vec<String> = names.iter().map(|n| if this { format!("this.{n}") } else { n.clone() }).collect();
    format!("{} {} {}", &text[..range.start], list.join(", "), &text[range.end..])
}

/// The statement without `doomed` in its list: `None` when nothing is left in it (the statement goes).
pub(super) fn without_items(text: &str, doomed: &[String]) -> Option<String> {
    let Some((names, range, this)) = list_of(text) else { return Some(text.to_owned()) };
    if !names.iter().any(|n| doomed.contains(n)) {
        return Some(text.to_owned());
    }
    let left: Vec<String> = names.into_iter().filter(|n| !doomed.contains(n)).collect();
    (!left.is_empty()).then(|| with_list(text, range, &left, this))
}

impl Designer {
    /// Whether `name` is an item of another component (a menu's item, a grid's column).
    pub fn is_item(&self, name: &str) -> bool {
        self.item_owner(name).is_some()
    }

    /// The strip, menu item or grid an item belongs to.
    pub fn item_owner(&self, name: &str) -> Option<String> {
        self.comps.iter().find_map(|c| {
            (c.name != name
                && c.members.iter().any(|m| match m {
                    Member::Raw(text) => text.contains("AddRange(") && list_of(text).is_some_and(|(names, ..)| names.iter().any(|n| n == name)),
                    _ => false,
                }))
            .then(|| c.name.clone())
        })
    }

    /// Every component an `AddRange` of `name` lists (whatever the collection).
    pub fn owned_items(&self, name: &str) -> Vec<String> {
        let Some(c) = self.get(name) else { return Vec::new() };
        let mut out: Vec<String> = Vec::new();
        for m in &c.members {
            if let Member::Raw(text) = m
                && text.contains("AddRange(")
                && let Some((names, ..)) = list_of(text)
            {
                for n in names {
                    if n != name && !self.is_root(&n) && self.get(&n).is_some() && !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }
        out
    }

    /// The statement that lists `owner`'s items (its `Items`, `DropDownItems` or `Columns`).
    fn collection_statement(&self, owner: &str) -> Option<(usize, &str)> {
        let c = self.get(owner)?;
        let (coll, _) = item_collection(&c.type_name)?;
        let prefix = format!("{owner}.{coll}.AddRange(");
        c.members.iter().enumerate().find_map(|(i, m)| match m {
            Member::Raw(text) if strip_this(text).starts_with(&prefix) => Some((i, text.as_str())),
            _ => None,
        })
    }

    /// A strip's or menu item's items, or a grid's columns, in order.
    pub fn items(&self, owner: &str) -> Vec<String> {
        self.collection_statement(owner)
            .and_then(|(_, text)| list_of(text))
            .map(|(names, ..)| names.into_iter().filter(|n| self.get(n).is_some()).collect())
            .unwrap_or_default()
    }

    fn is_free(&self, name: &str) -> bool {
        self.get(name).is_none() && name != "components" && !self.suffix.contains(&format!(" {name};"))
    }

    /// Adds an item (`type_name`: a ToolStripMenuItem, a grid column type, …) at the end of `owner`'s
    /// items, and returns its name: `toolStripMenuItem1`, or `Column1` for a column (as Visual Studio names
    /// them). Its Text (a column's HeaderText) is its name until it gets another.
    pub fn add_item(&mut self, owner: &str, type_name: &str) -> Option<String> {
        let owner_type = self.get(owner)?.type_name.clone();
        let (coll, array) = item_collection(&owner_type)?;
        let short = short_type(type_name).to_owned();
        let column = short.starts_with("DataGridView");
        let name = if column { (1..).map(|n| format!("Column{n}")).find(|n| self.is_free(n))? } else { self.unique_name(&short) };
        let written = if self.style.qualified { format!("System.Windows.Forms.{short}") } else { short.clone() };
        self.create(&name, &written, false, false);
        if column {
            self.set_prop(&name, "HeaderText", Some(&format_string(&name)));
        }
        self.set_prop(&name, "Name", Some(&format_string(&name)));
        if !column && short != "ToolStripSeparator" {
            self.set_prop(&name, "Text", Some(&format_string(&name)));
        }
        // Into the list (or a new one).
        let this = self.style.this;
        let item = if this { format!("this.{name}") } else { name.clone() };
        match self.collection_statement(owner).map(|(i, t)| (i, t.to_owned())) {
            Some((i, text)) => {
                let new = match list_of(&text) {
                    Some((mut names, range, uses_this)) => {
                        names.push(name.clone());
                        with_list(&text, range, &names, uses_this || (names.len() == 1 && this))
                    }
                    None => text,
                };
                if let Some(Member::Raw(t)) = self.get_mut(owner).and_then(|c| c.members.get_mut(i)) {
                    *t = new;
                }
            }
            None => {
                let text = format!("{}{coll}.AddRange(new {}[] {{ {item} }});", self.target(owner), type_ref(self.style, array));
                let c = self.get_mut(owner).expect("owner exists");
                let at = sorted_position(&c.members, &coll.to_ascii_lowercase(), owner, false);
                c.members.insert(at, Member::Raw(text));
            }
        }
        // Strips suspend their layout while their items are set up (menu items have none).
        if self.parent(owner).is_some() {
            self.ensure_layout_calls(owner);
        }
        Some(name)
    }

    /// Moves an item `delta` places in its owner's list (-1: before the one before it). False when it
    /// can't go further.
    pub fn move_item(&mut self, name: &str, delta: isize) -> bool {
        let Some(owner) = self.item_owner(name) else { return false };
        let Some(c) = self.get_mut(&owner) else { return false };
        for m in &mut c.members {
            if let Member::Raw(text) = m
                && text.contains("AddRange(")
                && let Some((mut names, range, this)) = list_of(text)
                && let Some(i) = names.iter().position(|n| n == name)
            {
                let j = i as isize + delta;
                if j < 0 || j >= names.len() as isize {
                    return false;
                }
                let item = names.remove(i);
                names.insert(j as usize, item);
                *text = with_list(text, range, &names, this);
                return true;
            }
        }
        false
    }

    /// The name Visual Studio gives a menu item for its text: `&File` → `fileToolStripMenuItem`, `Save
    /// As…` → `saveAsToolStripMenuItem`. `None` when the text has no letters.
    pub fn item_name_for_text(&self, text: &str, type_name: &str) -> Option<String> {
        // `&` marks the mnemonic letter inside a word (`E&xit`): it doesn't split it.
        let text = text.replace('&', "");
        let words: Vec<String> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect();
        let first = words.first()?;
        if !first.starts_with(char::is_alphabetic) {
            return None;
        }
        let mut name = lower_first(first);
        for w in &words[1..] {
            let mut chars = w.chars();
            if let Some(c) = chars.next() {
                name.extend(c.to_uppercase());
                name.push_str(chars.as_str());
            }
        }
        name.push_str(short_type(type_name));
        Some(name).filter(|n| lex::is_identifier(n))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{framework, net8};
    use super::*;

    #[test]
    fn adds_moves_and_removes_menu_items() {
        let mut d = net8();
        assert_eq!(d.items("menuStrip1"), ["fileToolStripMenuItem"]);
        assert_eq!(d.item_owner("fileToolStripMenuItem").as_deref(), Some("menuStrip1"));
        let edit = d.add_item("menuStrip1", "ToolStripMenuItem").unwrap();
        assert_eq!(edit, "toolStripMenuItem1");
        assert_eq!(d.items("menuStrip1"), ["fileToolStripMenuItem", "toolStripMenuItem1"]);
        assert_eq!(d.text_of(&edit).as_deref(), Some("toolStripMenuItem1"));
        // A submenu: File gets DropDownItems.
        let open = d.add_item("fileToolStripMenuItem", "ToolStripMenuItem").unwrap();
        let sep = d.add_item("fileToolStripMenuItem", "ToolStripSeparator").unwrap();
        assert_eq!((open.as_str(), sep.as_str()), ("toolStripMenuItem2", "toolStripSeparator1"));
        assert_eq!(d.items("fileToolStripMenuItem"), [open.clone(), sep.clone()]);
        assert!(d.tray().iter().all(|c| c.name != open), "items aren't in the tray");
        let text = d.to_text();
        assert!(text.contains("            menuStrip1.Items.AddRange(new ToolStripItem[] { fileToolStripMenuItem, toolStripMenuItem1 });"), "{text}");
        assert!(text.contains("            fileToolStripMenuItem.DropDownItems.AddRange(new ToolStripItem[] { toolStripMenuItem2, toolStripSeparator1 });"), "{text}");
        assert!(text.contains("        private ToolStripMenuItem toolStripMenuItem2;\n"), "{text}");
        assert!(!text.contains("toolStripSeparator1.Text"), "separators have no text");
        // Reordered.
        assert!(d.move_item("toolStripMenuItem1", -1));
        assert_eq!(d.items("menuStrip1"), ["toolStripMenuItem1", "fileToolStripMenuItem"]);
        assert!(!d.move_item("toolStripMenuItem1", -1), "already first");
        // Removing one keeps the others in the list; removing File takes its submenu with it.
        d.remove("toolStripMenuItem1");
        assert_eq!(d.items("menuStrip1"), ["fileToolStripMenuItem"]);
        d.remove("fileToolStripMenuItem");
        assert!(d.get(&open).is_none() && d.get(&sep).is_none());
        assert!(!d.to_text().contains("menuStrip1.Items.AddRange"), "an empty list goes");
        // Read back, it is the same form.
        let text = d.to_text();
        assert_eq!(Designer::parse(&text, "Form").unwrap().to_text(), text);
    }

    #[test]
    fn grid_columns_in_the_files_style() {
        let mut d = framework();
        let grid = d.add_component("DataGridView", None, (10, 10), None);
        let a = d.add_item(&grid, "DataGridViewTextBoxColumn").unwrap();
        let b = d.add_item(&grid, "DataGridViewCheckBoxColumn").unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("Column1", "Column2"));
        assert_eq!(d.items(&grid), ["Column1", "Column2"]);
        assert!(d.is_item("Column1") && !d.is_item(&grid));
        let text = d.to_text();
        assert!(text.contains("            this.dataGridView1.Columns.AddRange(new System.Windows.Forms.DataGridViewColumn[] { this.Column1, this.Column2 });"), "{text}");
        assert!(text.contains("            this.Column1.HeaderText = \"Column1\";"), "{text}");
        assert!(text.contains("            this.Column2 = new System.Windows.Forms.DataGridViewCheckBoxColumn();"), "{text}");
        assert_eq!(Designer::parse(&text, "Form").unwrap().items("dataGridView1"), ["Column1", "Column2"]);
        // A grid goes with its columns.
        d.remove(&grid);
        assert!(d.get("Column1").is_none() && !d.to_text().contains("Column2"));
    }

    #[test]
    fn names_from_text() {
        let d = net8();
        assert_eq!(d.item_name_for_text("&File", "ToolStripMenuItem").as_deref(), Some("fileToolStripMenuItem"));
        assert_eq!(d.item_name_for_text("Save &As…", "ToolStripMenuItem").as_deref(), Some("saveAsToolStripMenuItem"));
        assert_eq!(d.item_name_for_text("E&xit", "ToolStripMenuItem").as_deref(), Some("exitToolStripMenuItem"));
        assert_eq!(d.item_name_for_text("…", "ToolStripMenuItem"), None);
        assert_eq!(d.item_name_for_text("2nd", "ToolStripMenuItem"), None);
    }
}
