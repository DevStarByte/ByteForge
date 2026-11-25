//! The Windows Forms designer's model: the components of a form or user control as `InitializeComponent` in
//! its `.Designer.cs` creates and sets them up, read from that code and written back to it.
//!
//! The method is read statement by statement. Creations, properties (`button1.Text = "OK";`), events
//! (`button1.Click += button1_Click;`) and `Controls.Add` calls become the model; everything else is kept as
//! written, in the block it was in. Writing it back follows the Visual Studio designer's layout (creations,
//! layout suspension, one commented block per component with the form last, layout resumption) in the file's
//! own style: `this.` and qualified type names (.NET Framework) or neither (.NET 6 and later). Statements
//! nothing changed are written exactly as they were read, so a save without changes changes nothing.

mod catalog;
mod clip;
mod code;
mod grid;
mod items;
mod layout;
mod lex;
mod values;

pub use catalog::*;
pub use clip::*;
pub use code::*;
pub use grid::*;
pub use layout::*;
pub use lex::is_identifier;
pub use values::*;

/// How the file writes its code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Style {
    /// Members are written with `this.`.
    pub this: bool,
    /// Types are written with their namespace (`System.Drawing.Point`).
    pub qualified: bool,
}

/// A statement of a component's block.
#[derive(Clone, Debug, PartialEq)]
pub enum Member {
    /// `<target>.Name = value;` (`text` is the whole statement).
    Prop { name: String, value: String, text: String },
    /// `<target>.Name += handler;`
    Event { name: String, handler: String, text: String },
    /// Where the block's `Controls.Add` calls are.
    Children,
    /// Any other statement, as written.
    Raw(String),
}

/// A control added to a container's `Controls` (or to a part of it, like a SplitContainer's `Panel1`).
#[derive(Clone, Debug, PartialEq)]
pub struct Child {
    pub name: String,
    /// The part of the container the control is in (`Panel1`), or empty.
    pub slot: String,
    /// What the call passes after the control, with its comma (`, 0, 1` for a TableLayoutPanel cell).
    pub args: String,
    text: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub name: String,
    /// The type as the code writes it.
    pub type_name: String,
    /// What its constructor is passed (`components` for a Timer).
    ctor: String,
    /// The statement that creates it, as written.
    create: Option<String>,
    pub members: Vec<Member>,
    /// Its controls, front (index 0) to back, as `Controls.Add` adds them.
    pub children: Vec<Child>,
}

impl Component {
    pub fn short_type(&self) -> &str {
        short_type(&self.type_name)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Setup {
    Create(String),
    Raw { owner: Option<String>, text: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Designer {
    /// The file up to the first line of InitializeComponent's body.
    prefix: String,
    /// The file from the line that closes InitializeComponent.
    suffix: String,
    /// Indentation of the statements.
    indent: String,
    /// The empty comment lines around a block's name: `// ` as Visual Studio writes them, or as the file has them.
    blank_comment: String,
    /// Blocks the file has without the comment with their name (hand-written code, `dotnet new`'s template):
    /// they stay that way.
    bare: Vec<String>,
    /// Empty lines after the last statement (.NET Framework's designer leaves one).
    tail: String,
    pub class_name: String,
    pub style: Style,
    /// The form (or user control) itself first, then the components in the order they are created.
    comps: Vec<Component>,
    /// Statements before the first block: creations, SuspendLayout, BeginInit, local declarations.
    setup: Vec<Setup>,
    /// The order of the components' blocks.
    blocks: Vec<String>,
    /// ResumeLayout, PerformLayout and EndInit after the last block (with the component each is about).
    teardown: Vec<(Option<String>, String)>,
}

fn strip_this(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix("this.").unwrap_or(s).trim_start()
}

fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_lowercase().chain(chars).collect()).unwrap_or_default()
}

const MODIFIERS: &[&str] = &["private", "protected", "internal", "public", "readonly", "static", "new"];

/// The name a line declares as a field (`private Button button1;`, `Container components = null;`).
fn field_decl(line: &str) -> Option<&str> {
    let t = line.trim();
    let t = t.strip_suffix(';')?;
    if t.contains('(') || t.starts_with("//") || t.starts_with("using ") || t.starts_with("return") {
        return None;
    }
    let decl = t.split('=').next()?.trim();
    let tokens: Vec<&str> = decl.split_whitespace().filter(|w| !MODIFIERS.contains(w)).collect();
    match tokens.as_slice() {
        [_ty, name] if lex::is_identifier(name) => Some(name),
        _ => None,
    }
}

fn is_layout_call(text: &str) -> bool {
    let s = strip_this(text);
    let calls = ["SuspendLayout();", "ResumeLayout(false);", "ResumeLayout(true);", "ResumeLayout();", "PerformLayout();"];
    calls.iter().any(|c| s == *c || s.ends_with(&format!(".{c}"))) || (s.contains("ISupportInitialize") && (s.ends_with(".BeginInit();") || s.ends_with(".EndInit();")))
}

/// Indentation of the line `at` is on.
fn line_indent(text: &str, at: usize) -> String {
    let start = text[..at].rfind('\n').map_or(0, |p| p + 1);
    text[start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

fn find_initialize(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(p) = text[from..].find("InitializeComponent") {
        let at = from + p;
        let before = text[..at].trim_end();
        let after = text[at + "InitializeComponent".len()..].trim_start();
        if before.ends_with("void") && after.starts_with('(') {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

impl Designer {
    /// Reads a `.Designer.cs` file. `root_type` is the base class of the form (`Form`, `UserControl`), from
    /// its code file.
    pub fn parse(text: &str, root_type: &str) -> Result<Self, String> {
        let text = text.replace("\r\n", "\n");
        let at = find_initialize(&text).ok_or("The file has no InitializeComponent method.")?;
        let open = at + text[at..].find('{').ok_or("InitializeComponent has no body.")?;
        let close = lex::matching_brace(&text, open).ok_or("InitializeComponent's braces don't match.")?;
        let after_open = &text[open + 1..close];
        let body_start = match after_open.find('\n') {
            Some(nl) if after_open[..nl].trim().is_empty() => open + 1 + nl + 1,
            _ => open + 1,
        };
        let line_start = text[..close].rfind('\n').map_or(0, |p| p + 1);
        let body_end = if line_start >= body_start && text[line_start..close].trim().is_empty() { line_start } else { close };
        let body = &text[body_start..body_end];
        let class_name = text[..at]
            .rmatch_indices("class ")
            .find_map(|(p, _)| {
                let name: String = text[p + 6..].trim_start().chars().take_while(|c| lex::is_ident_char(*c)).collect();
                lex::is_identifier(&name).then_some(name)
            })
            .ok_or("No class around InitializeComponent.")?;
        let indent = body
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').collect())
            .unwrap_or_else(|| format!("{}    ", line_indent(&text, open)));
        let this = body.contains("this.");
        let qualified = if body.contains("System.Drawing.") {
            true
        } else if body.contains("new Point(") || body.contains("new Size(") {
            false
        } else {
            this
        };
        let fields: Vec<&str> = text[..body_start].lines().chain(text[body_end..].lines()).filter_map(field_decl).collect();

        let root_type = if root_type.is_empty() { "Form" } else { root_type };
        let root = Component { name: class_name.clone(), type_name: root_type.to_owned(), ctor: String::new(), create: None, members: Vec::new(), children: Vec::new() };
        let mut d = Designer {
            prefix: text[..body_start].to_owned(),
            suffix: text[body_end..].to_owned(),
            indent,
            blank_comment: "// ".into(),
            bare: Vec::new(),
            tail: String::new(),
            class_name: class_name.clone(),
            style: Style { this, qualified },
            comps: vec![root],
            setup: Vec::new(),
            blocks: Vec::new(),
            teardown: Vec::new(),
        };

        let items = lex::items(body);
        if let Some(lex::Item::Stmt(last)) = items.iter().rev().find(|i| matches!(i, lex::Item::Stmt(_))) {
            let rest = &body[last.end..];
            if let Some(nl) = rest.find('\n')
                && rest[nl + 1..].trim().is_empty()
            {
                d.tail = rest[nl + 1..].to_owned();
            }
        }
        let stmt = |i: &lex::Item| match i {
            lex::Item::Stmt(r) => Some(&body[r.clone()]),
            lex::Item::Comment(_) => None,
        };
        // Setup: creations, layout calls and declarations before the first statement that sets something up.
        let (mut ix, mut setup_end) = (0, 0);
        while ix < items.len() {
            let Some(s) = stmt(&items[ix]) else {
                ix += 1;
                continue;
            };
            if let Some((name, type_name, ctor)) = creation(s, &fields) {
                d.comps.push(Component { name: name.clone(), type_name, ctor, create: Some(s.to_owned()), members: Vec::new(), children: Vec::new() });
                d.setup.push(Setup::Create(name));
            } else if is_layout_call(s) || is_declaration(s) {
                let owner = d.owner_of(s);
                d.setup.push(Setup::Raw { owner, text: s.to_owned() });
            } else {
                break;
            }
            ix += 1;
            setup_end = ix;
        }
        // The blocks start right after the setup, with the comment naming the first one.
        let ix = setup_end;
        // Teardown: the layout calls at the end.
        let mut end = items.len();
        while end > ix {
            match stmt(&items[end - 1]) {
                Some(s) if is_layout_call(s) => end -= 1,
                None => end -= 1,
                Some(_) => break,
            }
        }
        for item in &items[end..] {
            if let Some(s) = stmt(item) {
                d.teardown.push((d.owner_of(s), s.to_owned()));
            }
        }
        // The blocks.
        let mut current: Option<String> = None;
        for item in &items[ix..end] {
            let s = match item {
                lex::Item::Comment(c) => {
                    if c.trim().is_empty() {
                        d.blank_comment = format!("//{c}");
                    }
                    let c = c.trim();
                    if d.get(c).is_some() {
                        current = Some(c.to_owned());
                    }
                    continue;
                }
                lex::Item::Stmt(r) => &body[r.clone()],
            };
            let (target, member, child) = d.classify(s);
            let block = current.clone().unwrap_or_else(|| target.clone());
            if !d.blocks.contains(&block) {
                if current.is_none() {
                    d.bare.push(block.clone());
                }
                d.blocks.push(block.clone());
            }
            let same = block == target;
            let comp = d.get_mut(&block).expect("blocks are components");
            match (child, same) {
                (Some(child), true) => {
                    if !comp.members.contains(&Member::Children) {
                        comp.members.push(Member::Children);
                    }
                    comp.children.push(child);
                }
                (None, true) => comp.members.push(member),
                _ => comp.members.push(Member::Raw(s.to_owned())),
            }
        }
        Ok(d)
    }

    /// The component a statement is about: the one it starts with, else the first one it mentions.
    fn owner_of(&self, s: &str) -> Option<String> {
        let first = strip_this(s).split(['.', ' ', '(', '=', ';']).next().unwrap_or("");
        if self.get(first).is_some_and(|c| c.name != self.class_name) {
            return Some(first.to_owned());
        }
        lex::identifiers(s).into_iter().map(|(r, _)| &s[r]).find(|n| self.comps[1..].iter().any(|c| c.name == *n)).map(str::to_owned)
    }

    /// What a block statement is: (the component it sets up, the member, or the control it adds).
    fn classify(&self, s: &str) -> (String, Member, Option<Child>) {
        let root = self.class_name.clone();
        let raw = |d: &Self| (d.owner_of(s).unwrap_or_else(|| root.clone()), Member::Raw(s.to_owned()), None);
        let body = s.trim_end().trim_end_matches(';').trim_end();
        if let Some(eq) = lex::find_top(body, b'=') {
            let rhs = body[eq + 1..].trim();
            let mut lhs = body[..eq].trim_end();
            if rhs.starts_with(['=', '>']) || lhs.ends_with(['!', '<', '>', '-', '*', '/']) {
                return raw(self);
            }
            let event = lhs.ends_with('+');
            if event {
                lhs = lhs[..lhs.len() - 1].trim_end();
            }
            let path: Vec<&str> = strip_this(lhs).split('.').map(str::trim).collect();
            if !path.iter().all(|p| lex::is_identifier(p)) {
                return raw(self);
            }
            let (target, name) = match path.as_slice() {
                [name] if self.get(name).is_none() => (root.clone(), *name),
                [owner, name] if self.get(owner).is_some_and(|c| c.name != root) => ((*owner).to_owned(), *name),
                _ => return raw(self),
            };
            let member = if event {
                Member::Event { name: name.to_owned(), handler: handler_name(rhs), text: s.to_owned() }
            } else {
                Member::Prop { name: name.to_owned(), value: rhs.to_owned(), text: s.to_owned() }
            };
            return (target, member, None);
        }
        let Some(paren) = body.find('(') else { return raw(self) };
        let path: Vec<&str> = strip_this(&body[..paren]).split('.').map(str::trim).collect();
        if path.len() >= 2 && path[path.len() - 2..] == ["Controls", "Add"] {
            let (target, slot) = if path.len() == 2 {
                (root.clone(), String::new())
            } else if self.get(path[0]).is_some_and(|c| c.name != root) {
                (path[0].to_owned(), path[1..path.len() - 2].join("."))
            } else {
                return raw(self);
            };
            let Some(args) = lex::call_args(body) else { return raw(self) };
            let (first, rest) = match lex::find_top(args, b',') {
                Some(p) => (&args[..p], &args[p..]),
                None => (args, ""),
            };
            let name = strip_this(first);
            if self.get(name).is_some_and(|c| c.name != root) {
                let child = Child { name: name.to_owned(), slot, args: rest.trim_end().to_owned(), text: Some(s.to_owned()) };
                return (target, Member::Children, Some(child));
            }
        }
        raw(self)
    }

    /// The file with the model written into InitializeComponent.
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity(self.prefix.len() + self.suffix.len() + 4096);
        out.push_str(&self.prefix);
        let mut line = |text: &str| {
            out.push_str(&self.indent);
            out.push_str(text);
            out.push('\n');
        };
        for s in &self.setup {
            match s {
                Setup::Create(name) => {
                    if let Some(c) = self.get(name) {
                        line(&c.create.clone().unwrap_or_else(|| self.creation_text(c)));
                    }
                }
                Setup::Raw { text, .. } => line(text),
            }
        }
        for block in &self.blocks {
            // A component nothing is set on gets no block (as in Visual Studio).
            let Some(c) = self.get(block).filter(|c| !c.members.is_empty()) else { continue };
            if !self.bare.contains(block) {
                line(&self.blank_comment);
                line(&format!("// {}", c.name));
                line(&self.blank_comment);
            }
            for m in &c.members {
                match m {
                    Member::Prop { text, .. } | Member::Event { text, .. } | Member::Raw(text) => line(text),
                    Member::Children => {
                        for child in &c.children {
                            line(&child.text.clone().unwrap_or_else(|| self.child_text(&c.name, child)));
                        }
                    }
                }
            }
        }
        for (_, text) in &self.teardown {
            line(text);
        }
        out.push_str(&self.tail);
        out.push_str(&self.suffix);
        out
    }

    // ───────────── reading ─────────────

    pub fn root(&self) -> &Component {
        &self.comps[0]
    }

    pub fn is_root(&self, name: &str) -> bool {
        name == self.class_name
    }

    /// The form first, then every component.
    pub fn components(&self) -> &[Component] {
        &self.comps
    }

    pub fn get(&self, name: &str) -> Option<&Component> {
        self.comps.iter().find(|c| c.name == name)
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut Component> {
        self.comps.iter_mut().find(|c| c.name == name)
    }

    /// The code a property is set to.
    pub fn prop(&self, name: &str, prop: &str) -> Option<&str> {
        self.get(name)?.members.iter().find_map(|m| match m {
            Member::Prop { name, value, .. } if name == prop => Some(value.as_str()),
            _ => None,
        })
    }

    /// The handlers hooked to a component's events.
    pub fn events(&self, name: &str) -> Vec<(&str, &str)> {
        self.get(name)
            .map(|c| {
                c.members
                    .iter()
                    .filter_map(|m| match m {
                        Member::Event { name, handler, .. } => Some((name.as_str(), handler.as_str())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The container a control is in, and the part of it.
    pub fn parent(&self, name: &str) -> Option<(&str, &str)> {
        self.comps.iter().find_map(|c| c.children.iter().find(|ch| ch.name == name).map(|ch| (c.name.as_str(), ch.slot.as_str())))
    }

    /// Whether `name` is `ancestor` or inside it.
    pub fn is_within(&self, name: &str, ancestor: &str) -> bool {
        let mut at = name;
        loop {
            if at == ancestor {
                return true;
            }
            match self.parent(at) {
                Some((p, _)) => at = p,
                None => return false,
            }
        }
    }

    /// Components that are not controls on the form: timers, dialogs and the like (not the items of menus
    /// and lists, which belong to their owner).
    pub fn tray(&self) -> Vec<&Component> {
        self.comps[1..].iter().filter(|c| self.parent(&c.name).is_none() && !self.is_item(&c.name)).collect()
    }

    /// The items an `Items.AddRange(new ToolStripItem[] { … })` (or `DropDownItems`) gives a strip or menu.
    pub fn strip_items(&self, name: &str) -> Vec<String> {
        let Some(c) = self.get(name) else { return Vec::new() };
        c.members
            .iter()
            .filter_map(|m| match m {
                Member::Raw(text) if text.contains("Items.AddRange(") => Some(text),
                _ => None,
            })
            .flat_map(|text| {
                lex::identifiers(text)
                    .into_iter()
                    .map(|(r, _)| text[r].to_owned())
                    .filter(|n| n != name && self.get(n).is_some())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The strings of a list's `Items.AddRange(new object[] { … })`.
    pub fn list_items(&self, name: &str) -> Vec<String> {
        let Some(c) = self.get(name) else { return Vec::new() };
        for m in &c.members {
            if let Member::Raw(text) = m
                && strip_this(text).strip_prefix(&format!("{name}.")).is_some_and(|r| r.starts_with("Items.AddRange("))
                && let (Some(open), Some(close)) = (text.find('{'), text.rfind('}'))
            {
                return lex::split_args(&text[open + 1..close]).iter().map(|a| parse_string(a).unwrap_or_else(|| a.to_string())).collect();
            }
        }
        Vec::new()
    }

    pub fn text_of(&self, name: &str) -> Option<String> {
        self.prop(name, "Text").and_then(parse_string)
    }

    pub fn location(&self, name: &str) -> (i32, i32) {
        self.prop(name, "Location").and_then(parse_pair).map_or((0, 0), |(x, y)| (x as i32, y as i32))
    }

    /// Its size (the form's: its client area).
    pub fn size(&self, name: &str) -> (i32, i32) {
        let Some(c) = self.get(name) else { return (0, 0) };
        let key = if self.is_root(name) && c.short_type() != "UserControl" { "ClientSize" } else { "Size" };
        let fallback = || info(&c.type_name).map_or((100, 23), |i| i.size);
        self.prop(name, key).and_then(parse_pair).map_or_else(fallback, |(w, h)| (w as i32, h as i32))
    }

    pub fn dock(&self, name: &str) -> String {
        self.prop(name, "Dock").and_then(parse_enum).unwrap_or_else(|| self.get(name).and_then(|c| info(&c.type_name)).map_or("None", |i| i.dock).to_owned())
    }

    pub fn anchor(&self, name: &str) -> Vec<String> {
        match self.prop(name, "Anchor") {
            Some(v) => parse_flags(v, "AnchorStyles"),
            None => vec!["Top".into(), "Left".into()],
        }
    }

    // ───────────── writing code ─────────────

    fn this(&self) -> &'static str {
        if self.style.this { "this." } else { "" }
    }

    /// What statements about `owner` start with: `this.button1.`, `button1.`, `this.` or nothing.
    fn target(&self, owner: &str) -> String {
        if self.is_root(owner) { self.this().to_owned() } else { format!("{}{owner}.", self.this()) }
    }

    fn creation_text(&self, c: &Component) -> String {
        format!("{}{} = new {}({});", self.this(), c.name, c.type_name, c.ctor)
    }

    fn child_text(&self, owner: &str, child: &Child) -> String {
        let slot = if child.slot.is_empty() { String::new() } else { format!("{}.", child.slot) };
        format!("{}{slot}Controls.Add({}{}{});", self.target(owner), self.this(), child.name, child.args)
    }

    /// Code for a value the catalog writes as a template (`{text}`: the component's name as a string;
    /// `Type.Member`: qualified as the file does).
    fn template(&self, value: &str, name: &str) -> String {
        if value == "{text}" {
            return format_string(name);
        }
        match value.split_once('.') {
            Some((ty, member)) if ty.starts_with(|c: char| c.is_ascii_uppercase()) && lex::is_identifier(member) => format_enum(self.style, ty, member),
            _ => value.to_owned(),
        }
    }

    // ───────────── changing ─────────────

    /// Sets a property to code (`None`: takes it out, back to its default).
    pub fn set_prop(&mut self, owner: &str, prop: &str, value: Option<&str>) {
        let target = self.target(owner);
        let root = self.is_root(owner);
        let Some(c) = self.get_mut(owner) else { return };
        let Some(value) = value else {
            c.members.retain(|m| !matches!(m, Member::Prop { name, .. } if name == prop));
            return;
        };
        let text = format!("{target}{prop} = {value};");
        if let Some(Member::Prop { value: v, text: t, .. }) = c.members.iter_mut().find(|m| matches!(m, Member::Prop { name, .. } if name == prop)) {
            if v != value {
                *v = value.to_owned();
                *t = text;
            }
            return;
        }
        let member = Member::Prop { name: prop.to_owned(), value: value.to_owned(), text };
        let at = sorted_position(&c.members, &sort_key(prop, root), owner, root);
        c.members.insert(at, member);
    }

    /// Hooks a handler to an event (`None`: unhooks it).
    pub fn set_event(&mut self, owner: &str, event: &str, handler: Option<&str>) {
        let target = self.target(owner);
        let (qualified, this) = (self.style.qualified, self.this());
        let Some(c) = self.get_mut(owner) else { return };
        c.members.retain(|m| !matches!(m, Member::Event { name, .. } if name == event));
        if let Some(handler) = handler {
            let text = if qualified {
                format!("{target}{event} += new {}({this}{handler});", event_info(event).delegate)
            } else {
                format!("{target}{event} += {handler};")
            };
            c.members.push(Member::Event { name: event.to_owned(), handler: handler.to_owned(), text });
        }
    }

    /// Replaces the list's items (`Items.AddRange(new object[] { … })`).
    pub fn set_list_items(&mut self, owner: &str, items: &[String]) {
        let target = self.target(owner);
        let Some(c) = self.get_mut(owner) else { return };
        let prefix = format!("{owner}.Items.AddRange(");
        c.members.retain(|m| !matches!(m, Member::Raw(t) if strip_this(t).starts_with(&prefix)));
        if items.is_empty() {
            return;
        }
        let list: Vec<String> = items.iter().map(|i| format_string(i)).collect();
        let text = format!("{target}Items.AddRange(new object[] {{ {} }});", list.join(", "));
        let at = sorted_position(&c.members, "items", owner, false);
        c.members.insert(at, Member::Raw(text));
    }

    pub fn set_location(&mut self, name: &str, (x, y): (i32, i32)) {
        let value = format_pair(self.style, "Point", x as i64, y as i64);
        self.set_prop(name, "Location", Some(&value));
    }

    /// Sets the size (the form's client size); anchored controls inside follow, as they do at run time.
    pub fn set_size(&mut self, name: &str, size: (i32, i32)) {
        let old = self.size(name);
        let size = (size.0.max(1), size.1.max(1));
        let key = if self.is_root(name) && self.root().short_type() != "UserControl" { "ClientSize" } else { "Size" };
        let value = format_pair(self.style, "Size", size.0 as i64, size.1 as i64);
        self.set_prop(name, key, Some(&value));
        let (dw, dh) = (size.0 - old.0, size.1 - old.1);
        if dw == 0 && dh == 0 {
            return;
        }
        let children: Vec<String> = self.get(name).map(|c| c.children.iter().map(|ch| ch.name.clone()).collect()).unwrap_or_default();
        for child in children {
            if self.dock(&child) != "None" {
                continue;
            }
            let anchor = self.anchor(&child);
            let has = |a: &str| anchor.iter().any(|x| x == a);
            let (mut x, mut y) = self.location(&child);
            let (mut w, mut h) = self.size(&child);
            match (has("Left"), has("Right")) {
                (true, true) => w += dw,
                (false, true) => x += dw,
                (false, false) => x += dw / 2,
                _ => {}
            }
            match (has("Top"), has("Bottom")) {
                (true, true) => h += dh,
                (false, true) => y += dh,
                (false, false) => y += dh / 2,
                _ => {}
            }
            if (x, y) != self.location(&child) {
                self.set_location(&child, (x, y));
            }
            if (w, h) != self.size(&child) {
                self.set_size(&child, (w, h));
            }
        }
    }

    /// A name no component (or the form) has yet: `button1`, `button2`, …
    pub fn unique_name(&self, base: &str) -> String {
        let base = lower_first(base);
        (1..).map(|n| format!("{base}{n}")).find(|n| self.get(n).is_none() && n != "components" && !self.suffix.contains(&format!(" {n};"))).unwrap_or_default()
    }

    /// Adds a new control (to `parent`, the form when `None`) or component, and returns its name.
    pub fn add_component(&mut self, type_name: &str, parent: Option<&str>, at: (i32, i32), size: Option<(i32, i32)>) -> String {
        let short = short_type(type_name).to_owned();
        let info = info(&short);
        let name = self.unique_name(&short);
        let written = if self.style.qualified || short == "Timer" { format!("System.Windows.Forms.{short}") } else { short.clone() };
        self.create(&name, &written, info.is_some_and(|i| i.needs_container), info.is_some_and(|i| i.init));

        let component = is_component(&short);
        if !component {
            let parent = parent.map(str::to_owned).unwrap_or_else(|| self.class_name.clone());
            let index = self.get(&parent).map_or(0, |p| p.children.len());
            let mut size = size.or(info.map(|i| i.size)).unwrap_or((100, 23));
            if short == "Label" || short == "LinkLabel" {
                size = (name.chars().count() as i32 * 6 + 2, 15);
            }
            let dock = info.map_or("None", |i| i.dock);
            if dock != "None" {
                // Docked strips span their container.
                size.0 = self.size(&parent).0;
            }
            self.set_location(&name, if dock == "None" { at } else { (0, 0) });
            self.set_prop(&name, "Name", Some(&format_string(&name)));
            let value = format_pair(self.style, "Size", size.0 as i64, size.1 as i64);
            self.set_prop(&name, "Size", Some(&value));
            self.set_prop(&name, "TabIndex", Some(&index.to_string()));
            let parent_type = self.get(&parent).map(|p| p.short_type().to_owned()).unwrap_or_default();
            let slot = if parent_type == "SplitContainer" { "Panel1" } else { "" };
            // Flow and table panels arrange in collection order: a new control goes last there.
            let front = !matches!(parent_type.as_str(), "FlowLayoutPanel" | "TableLayoutPanel");
            self.add_child(&parent, slot, &name, front);
            self.ensure_layout_calls(&parent);
        }
        for (prop, value) in info.map(|i| i.initial).unwrap_or_default() {
            let code = self.template(value, &name);
            self.set_prop(&name, prop, Some(&code));
        }
        match short.as_str() {
            "TabControl" => {
                let (w, h) = self.size(&name);
                for _ in 0..2 {
                    let page = self.add_component("TabPage", Some(&name), (4, 24), Some((w - 8, h - 28)));
                    let padding = format!("new {}(3)", type_ref(self.style, "Padding"));
                    self.set_prop(&page, "Padding", Some(&padding));
                    // Pages are listed first to last, not front to back.
                    let c = self.get_mut(&name).expect("just added");
                    if let Some(p) = c.children.iter().position(|ch| ch.name == page) {
                        let ch = c.children.remove(p);
                        c.children.push(ch);
                    }
                }
            }
            "SplitContainer" => {
                let w = self.size(&name).0;
                self.set_prop(&name, "SplitterDistance", Some(&(w / 3).to_string()));
            }
            _ => {}
        }
        name
    }

    /// Creates a component with nothing set on it yet: its creation statement, its (empty) block before the
    /// form's, its field and, for ISupportInitialize types, BeginInit/EndInit.
    fn create(&mut self, name: &str, written_type: &str, needs_container: bool, init: bool) {
        let ctor = if needs_container {
            self.ensure_components_container();
            format!("{}components", self.this())
        } else {
            String::new()
        };
        self.comps.push(Component { name: name.to_owned(), type_name: written_type.to_owned(), ctor, create: None, members: Vec::new(), children: Vec::new() });
        let last_create = self.setup.iter().rposition(|s| matches!(s, Setup::Create(_)));
        let first_layout = self.setup.iter().position(|s| matches!(s, Setup::Raw { text, .. } if is_layout_call(text)));
        let at_setup = last_create.map(|i| i + 1).or(first_layout).unwrap_or(self.setup.len());
        self.setup.insert(at_setup, Setup::Create(name.to_owned()));
        let root_block = self.blocks.iter().position(|b| *b == self.class_name).unwrap_or(self.blocks.len());
        self.blocks.insert(root_block, name.to_owned());
        self.add_field(written_type, name);
        if init {
            self.add_init_calls(name);
        }
    }

    fn add_child(&mut self, parent: &str, slot: &str, name: &str, front: bool) {
        let root = self.is_root(parent);
        let Some(c) = self.get_mut(parent) else { return };
        let child = Child { name: name.to_owned(), slot: slot.to_owned(), args: String::new(), text: None };
        if front { c.children.insert(0, child) } else { c.children.push(child) }
        if !c.members.contains(&Member::Children) {
            let at = sorted_position(&c.members, &sort_key("Controls", root), parent, root);
            c.members.insert(at, Member::Children);
        }
    }

    /// Takes a control out of its container; returns where it was.
    fn detach(&mut self, name: &str) -> Option<(String, Child)> {
        for c in &mut self.comps {
            if let Some(p) = c.children.iter().position(|ch| ch.name == name) {
                let child = c.children.remove(p);
                if c.children.is_empty() {
                    c.members.retain(|m| *m != Member::Children);
                }
                return Some((c.name.clone(), child));
            }
        }
        None
    }

    /// Moves a control into another container (or another part of it), at `location` there.
    pub fn reparent(&mut self, name: &str, parent: &str, slot: &str, location: (i32, i32)) {
        if self.is_within(parent, name) {
            return;
        }
        if let Some((_, mut child)) = self.detach(name) {
            child.slot = slot.to_owned();
            child.text = None;
            // Table cells don't carry over.
            child.args.clear();
            let root = self.is_root(parent);
            let c = self.get_mut(parent).expect("parent exists");
            c.children.insert(0, child);
            if !c.members.contains(&Member::Children) {
                let at = sorted_position(&c.members, &sort_key("Controls", root), parent, root);
                c.members.insert(at, Member::Children);
            }
        } else {
            self.add_child(parent, slot, name, true);
        }
        self.set_location(name, location);
        self.ensure_layout_calls(parent);
    }

    /// Brings a control to the front (`true`) or sends it to the back of its container.
    pub fn reorder(&mut self, name: &str, front: bool) {
        let Some((parent, _)) = self.parent(name).map(|(p, s)| (p.to_owned(), s.to_owned())) else { return };
        let c = self.get_mut(&parent).expect("parent exists");
        let Some(p) = c.children.iter().position(|ch| ch.name == name) else { return };
        let child = c.children.remove(p);
        if front { c.children.insert(0, child) } else { c.children.push(child) }
    }

    /// Deletes a component with what is inside it (controls, menu items), and every statement that uses it.
    pub fn remove(&mut self, name: &str) {
        if self.is_root(name) || self.get(name).is_none() {
            return;
        }
        let mut doomed = vec![name.to_owned()];
        let mut i = 0;
        while i < doomed.len() {
            let n = doomed[i].clone();
            if let Some(c) = self.get(&n) {
                let mut inside: Vec<String> = c.children.iter().map(|ch| ch.name.clone()).collect();
                inside.extend(self.owned_items(&n));
                for x in inside {
                    if !doomed.contains(&x) {
                        doomed.push(x);
                    }
                }
            }
            i += 1;
        }
        for n in &doomed {
            self.detach(n);
        }
        self.comps.retain(|c| !doomed.contains(&c.name));
        self.blocks.retain(|b| !doomed.contains(b));
        let uses = |text: &str| doomed.iter().any(|n| lex::mentions(text, n));
        self.setup.retain(|s| match s {
            Setup::Create(n) => !doomed.contains(n),
            Setup::Raw { text, .. } => !uses(text),
        });
        self.teardown.retain(|(_, text)| !uses(text));
        for c in &mut self.comps {
            let members = std::mem::take(&mut c.members);
            c.members = members
                .into_iter()
                .filter_map(|m| match m {
                    Member::Prop { ref value, .. } if uses(value) => None,
                    // A list of items loses the doomed ones; the others stay.
                    Member::Raw(text) if uses(&text) => items::without_items(&text, &doomed).filter(|t| text.contains("AddRange(") && !uses(t)).map(Member::Raw),
                    m => Some(m),
                })
                .collect();
        }
        for n in &doomed {
            self.remove_field(n);
        }
    }

    /// Renames a component: its field, every statement that uses it and its Name.
    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), String> {
        if old == new {
            return Ok(());
        }
        if !lex::is_identifier(new) {
            return Err(format!("'{new}' is not a valid name."));
        }
        if self.get(new).is_some() || new == "components" {
            return Err(format!("There is already a component named '{new}'."));
        }
        if self.is_root(old) {
            return Err("The form's name is its class name; rename the class instead.".into());
        }
        let r = |s: &str| lex::replace_ident(s, old, new);
        for c in &mut self.comps {
            if c.name == old {
                c.name = new.to_owned();
            }
            c.create = c.create.as_deref().map(r);
            for m in &mut c.members {
                match m {
                    Member::Prop { value, text, .. } => {
                        *value = r(value);
                        *text = r(text);
                    }
                    Member::Event { text, .. } | Member::Raw(text) => *text = r(text),
                    Member::Children => {}
                }
            }
            for ch in &mut c.children {
                if ch.name == old {
                    ch.name = new.to_owned();
                }
                ch.text = ch.text.as_deref().map(r);
            }
        }
        for s in &mut self.setup {
            match s {
                Setup::Create(n) if n == old => *n = new.to_owned(),
                Setup::Raw { owner, text } => {
                    if owner.as_deref() == Some(old) {
                        *owner = Some(new.to_owned());
                    }
                    *text = r(text);
                }
                _ => {}
            }
        }
        for (owner, text) in &mut self.teardown {
            if owner.as_deref() == Some(old) {
                *owner = Some(new.to_owned());
            }
            *text = r(text);
        }
        for b in self.blocks.iter_mut().chain(self.bare.iter_mut()) {
            if b == old {
                *b = new.to_owned();
            }
        }
        self.suffix = self.suffix.split_inclusive('\n').map(|l| if field_decl(l) == Some(old) { r(l) } else { l.to_owned() }).collect();
        if self.prop(new, "Name").is_some() {
            self.set_prop(new, "Name", Some(&format_string(new)));
        }
        Ok(())
    }

    fn ensure_components_container(&mut self) {
        let has = self.setup.iter().any(|s| matches!(s, Setup::Raw { text, .. } if strip_this(text).starts_with("components = ")));
        if !has {
            let text = format!("{}components = new System.ComponentModel.Container();", self.this());
            self.setup.insert(0, Setup::Raw { owner: None, text });
        }
        if !self.prefix.contains(" components") && !self.suffix.contains(" components") {
            self.add_field("System.ComponentModel.IContainer", "components");
        }
    }

    fn is_root_call(&self, text: &str, call: &str) -> bool {
        strip_this(text) == call
    }

    /// `SuspendLayout()` and `ResumeLayout(false)` / `PerformLayout()` around a container that holds controls.
    fn ensure_layout_calls(&mut self, owner: &str) {
        let target = self.target(owner);
        let root = self.is_root(owner);
        let suspend = format!("{target}SuspendLayout();");
        if !self.setup.iter().any(|s| matches!(s, Setup::Raw { text, .. } if *text == suspend || strip_this(text) == strip_this(&suspend))) {
            let at = if root { self.setup.len() } else { self.setup.iter().position(|s| matches!(s, Setup::Raw { text, .. } if self.is_root_call(text, "SuspendLayout();"))).unwrap_or(self.setup.len()) };
            self.setup.insert(at, Setup::Raw { owner: (!root).then(|| owner.to_owned()), text: suspend });
        }
        let resume = format!("{target}ResumeLayout(false);");
        if !self.teardown.iter().any(|(_, t)| strip_this(t) == strip_this(&resume)) {
            let at = if root { self.teardown.len() } else { self.teardown.iter().position(|(_, t)| self.is_root_call(t, "ResumeLayout(false);")).unwrap_or(self.teardown.len()) };
            let owner = (!root).then(|| owner.to_owned());
            self.teardown.insert(at, (owner.clone(), resume));
            self.teardown.insert(at + 1, (owner, format!("{target}PerformLayout();")));
        }
    }

    fn add_init_calls(&mut self, name: &str) {
        let r = if self.style.this { format!("(this.{name})") } else { name.to_owned() };
        let begin = format!("((System.ComponentModel.ISupportInitialize){r}).BeginInit();");
        let end = format!("((System.ComponentModel.ISupportInitialize){r}).EndInit();");
        let at = self.setup.iter().position(|s| matches!(s, Setup::Raw { text, .. } if self.is_root_call(text, "SuspendLayout();"))).unwrap_or(self.setup.len());
        self.setup.insert(at, Setup::Raw { owner: Some(name.to_owned()), text: begin });
        let at = self.teardown.iter().position(|(_, t)| self.is_root_call(t, "ResumeLayout(false);")).unwrap_or(self.teardown.len());
        self.teardown.insert(at, (Some(name.to_owned()), end));
    }

    /// Declares a field for a new component next to the others (or before the class's closing brace).
    fn add_field(&mut self, type_name: &str, name: &str) {
        let lines: Vec<&str> = self.suffix.split_inclusive('\n').collect();
        let last = lines.iter().rposition(|l| field_decl(l).is_some());
        let (at, indent) = match last {
            Some(i) => (i + 1, lines[i].chars().take_while(|c| c.is_whitespace() && *c != '\n').collect::<String>()),
            None => {
                let method_indent: String = lines.first().map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').collect()).unwrap_or_default();
                let close = lines.iter().skip(1).position(|l| l.trim() == "}").map_or(lines.len(), |p| p + 1);
                (close, method_indent)
            }
        };
        let mut line = format!("{indent}private {type_name} {name};\n");
        if last.is_none() {
            line.insert(0, '\n');
        }
        let mut out: String = lines[..at].concat();
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
        out.push_str(&lines[at..].concat());
        self.suffix = out;
    }

    fn remove_field(&mut self, name: &str) {
        self.suffix = self.suffix.split_inclusive('\n').filter(|l| field_decl(l) != Some(name)).collect();
    }
}

/// A creation statement in setup: `button1 = new Button();` → (name, type, constructor arguments).
fn creation(s: &str, fields: &[&str]) -> Option<(String, String, String)> {
    let body = s.trim_end().strip_suffix(';')?.trim_end();
    let eq = lex::find_top(body, b'=')?;
    let lhs = strip_this(&body[..eq]);
    let rhs = body[eq + 1..].trim();
    if !lex::is_identifier(lhs) || lhs == "components" || !rhs.starts_with("new ") {
        return None;
    }
    if !fields.contains(&lhs) && !lhs.starts_with(|c: char| c.is_lowercase()) {
        return None;
    }
    let rest = rhs[4..].trim_start();
    let paren = rest.find('(')?;
    let type_name = rest[..paren].trim().to_owned();
    let ctor = lex::call_args(rest)?.trim().to_owned();
    Some((lhs.to_owned(), type_name, ctor))
}

/// A local declaration (`ComponentResourceManager resources = new …;`) or the components container.
fn is_declaration(s: &str) -> bool {
    let body = s.trim_end().trim_end_matches(';');
    match lex::find_top(body, b'=') {
        Some(eq) => {
            let lhs = body[..eq].trim();
            strip_this(lhs) == "components" || (lhs.split_whitespace().count() == 2 && !lhs.ends_with(['+', '-']))
        }
        None => false,
    }
}

/// The method an event hookup names: `new System.EventHandler(this.button1_Click)` → `button1_Click`.
fn handler_name(rhs: &str) -> String {
    let inner = if rhs.starts_with("new ") { lex::call_args(rhs).unwrap_or(rhs) } else { rhs };
    strip_this(inner).rsplit('.').next().unwrap_or("").trim().to_owned()
}

/// Where Visual Studio sorts a statement in a block (alphabetically by property; the form's AutoScale
/// settings first).
fn sort_key(name: &str, root: bool) -> String {
    match name {
        "AutoScaleDimensions" if root => "\0a".into(),
        "AutoScaleMode" if root => "\0b".into(),
        n => n.to_ascii_lowercase(),
    }
}

fn member_key(m: &Member, owner: &str, root: bool) -> Option<String> {
    match m {
        Member::Prop { name, .. } => Some(sort_key(name, root)),
        Member::Children => Some("controls".into()),
        Member::Raw(text) => {
            let rest = strip_this(text);
            let rest = rest.strip_prefix(&format!("{owner}.")).unwrap_or(rest);
            let first: String = rest.chars().take_while(|c| lex::is_ident_char(*c)).collect();
            (!first.is_empty()).then(|| first.to_ascii_lowercase())
        }
        Member::Event { .. } => None,
    }
}

/// Index to insert a statement sorting as `key` (before events, which come last).
fn sorted_position(members: &[Member], key: &str, owner: &str, root: bool) -> usize {
    let key = key.to_ascii_lowercase();
    let mut last_keyed = None;
    for (i, m) in members.iter().enumerate() {
        if let Some(k) = member_key(m, owner, root) {
            if k > key {
                return i;
            }
            last_keyed = Some(i);
        }
    }
    last_keyed.map_or_else(|| members.iter().position(|m| matches!(m, Member::Event { .. })).unwrap_or(members.len()), |i| i + 1)
}

#[cfg(test)]
mod tests;
