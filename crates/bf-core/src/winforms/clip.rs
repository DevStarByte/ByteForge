//! Copy and paste of controls: a [`Fragment`] holds components with what is set on them and what is inside
//! them (controls, menu items, grid columns), taken out of one form and pasted into the same or another one,
//! where each gets a free name and the code is written in that file's style.

use super::*;

/// What a component's statement was.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    Prop(String, String),
    Event(String, String),
    /// A statement about the component, without its `this.button1.` (`Items.AddRange(…);`).
    Own(String),
    /// Any other statement in its block, as written.
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
struct Piece {
    name: String,
    type_name: String,
    needs_container: bool,
    init: bool,
    /// The setup suspends its layout (a container, a menu strip).
    layout: bool,
    parts: Vec<Part>,
    /// (name, slot, arguments after the control), front to back.
    children: Vec<(String, String, String)>,
}

/// Copied components.
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    /// The class they were copied from: their event handlers only exist there.
    pub class_name: String,
    /// The copied ones that aren't inside another copied one, in the order the form created them.
    pub top: Vec<String>,
    /// Every copied component (what is inside the top ones too), in the order the form created them.
    pieces: Vec<Piece>,
}

impl Fragment {
    /// The fragment as code (creations and statements in .NET 6 style), for the system clipboard.
    pub fn to_code(&self) -> String {
        let mut out = String::new();
        for p in &self.pieces {
            out.push_str(&format!("{} = new {}({});\n", p.name, p.type_name, if p.needs_container { "components" } else { "" }));
        }
        for p in &self.pieces {
            for part in &p.parts {
                let line = match part {
                    Part::Prop(n, v) => format!("{}.{n} = {v};", p.name),
                    Part::Event(n, h) => format!("{}.{n} += {h};", p.name),
                    Part::Own(s) => format!("{}.{s}", p.name),
                    Part::Other(s) => s.clone(),
                };
                out.push_str(&line);
                out.push('\n');
            }
            for (child, slot, args) in &p.children {
                let slot = if slot.is_empty() { String::new() } else { format!("{slot}.") };
                out.push_str(&format!("{}.{slot}Controls.Add({child}{args});\n", p.name));
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.top.len()
    }

    pub fn is_empty(&self) -> bool {
        self.top.is_empty()
    }
}

impl Designer {
    /// The components an `AddRange` of `name` adds (menu items, grid columns).
    fn owned(&self, name: &str) -> Vec<String> {
        let Some(c) = self.get(name) else { return Vec::new() };
        let mut out: Vec<String> = Vec::new();
        for m in &c.members {
            if let Member::Raw(text) = m
                && text.contains("AddRange(")
            {
                for (r, member) in lex::identifiers(text) {
                    let n = &text[r];
                    if !member && n != name && !self.is_root(n) && self.get(n).is_some() && !out.iter().any(|o| o == n) {
                        out.push(n.to_owned());
                    }
                }
            }
        }
        out
    }

    /// Copies components (the form itself is left out) with everything inside them.
    pub fn copy(&self, names: &[String]) -> Fragment {
        let names: Vec<&String> = names.iter().filter(|n| !self.is_root(n) && self.get(n).is_some()).collect();
        let top: Vec<String> = self.comps[1..]
            .iter()
            .map(|c| &c.name)
            .filter(|n| names.contains(n) && !names.iter().any(|o| o != n && self.is_within(n, o)))
            .cloned()
            .collect();
        let mut all: Vec<String> = top.clone();
        let mut i = 0;
        while i < all.len() {
            let n = all[i].clone();
            if let Some(c) = self.get(&n) {
                let mut inside: Vec<String> = c.children.iter().map(|ch| ch.name.clone()).collect();
                inside.extend(self.owned(&n));
                for x in inside {
                    if !all.contains(&x) {
                        all.push(x);
                    }
                }
            }
            i += 1;
        }
        let pieces = self.comps[1..]
            .iter()
            .filter(|c| all.contains(&c.name))
            .map(|c| {
                let own = format!("{}.", c.name);
                let parts = c
                    .members
                    .iter()
                    .filter_map(|m| match m {
                        Member::Prop { name, value, .. } => Some(Part::Prop(name.clone(), value.clone())),
                        Member::Event { name, handler, .. } => Some(Part::Event(name.clone(), handler.clone())),
                        Member::Raw(text) => Some(match strip_this(text).strip_prefix(&own) {
                            Some(rest) => Part::Own(rest.to_owned()),
                            None => Part::Other(text.clone()),
                        }),
                        Member::Children => None,
                    })
                    .collect();
                let layout = self.setup.iter().any(|s| matches!(s, Setup::Raw { owner: Some(o), text } if *o == c.name && text.contains("SuspendLayout")));
                let init = self.setup.iter().any(|s| matches!(s, Setup::Raw { owner: Some(o), text } if *o == c.name && text.contains("BeginInit")));
                Piece {
                    name: c.name.clone(),
                    type_name: c.type_name.clone(),
                    needs_container: strip_this(&c.ctor) == "components",
                    init,
                    layout,
                    parts,
                    children: c.children.iter().map(|ch| (ch.name.clone(), ch.slot.clone(), ch.args.clone())).collect(),
                }
            })
            .collect();
        Fragment { class_name: self.class_name.clone(), top, pieces }
    }

    /// Pastes copied components into `parent` (the form when it is the root) and returns the new names of
    /// the top ones. Each keeps its name when it is free here, else gets the next free one (`button2`).
    /// Controls keep their location unless one of the container's controls is already there; then they
    /// move down and right until none is.
    pub fn paste(&mut self, f: &Fragment, parent: &str, slot: &str) -> Vec<String> {
        if self.get(parent).is_none() || f.pieces.is_empty() {
            return Vec::new();
        }
        let style = self.style;
        // New names, one after the other, so two pasted components don't get the same one.
        let mut taken: Vec<String> = Vec::new();
        let mut renames: Vec<(String, String)> = Vec::new();
        for p in &f.pieces {
            let free = |n: &str, d: &Designer, taken: &[String]| d.get(n).is_none() && n != "components" && !d.suffix.contains(&format!(" {n};")) && !taken.iter().any(|t| t == n);
            let new = if free(&p.name, self, &taken) {
                p.name.clone()
            } else {
                let base = lower_first(short_type(&p.type_name));
                (1..).map(|n| format!("{base}{n}")).find(|n| free(n, self, &taken)).unwrap_or_default()
            };
            taken.push(new.clone());
            renames.push((p.name.clone(), new));
        }
        let rename = |s: &str| -> String {
            // Through placeholders, so `a → b` and `b → c` don't chain.
            let mut out = s.to_owned();
            for (i, (old, new)) in renames.iter().enumerate() {
                if old != new {
                    out = lex::replace_ident(&out, old, &format!("__bf_paste_{i}__"));
                }
            }
            for (i, (old, new)) in renames.iter().enumerate() {
                if old != new {
                    out = out.replace(&format!("__bf_paste_{i}__"), new);
                }
            }
            out
        };
        let new_name = |old: &str| renames.iter().find(|(o, _)| o == old).map_or_else(|| old.to_owned(), |(_, n)| n.clone());
        let same_form = f.class_name == self.class_name;

        // Where the top controls go: moved on while one of the container's controls is where the first is.
        let siblings: Vec<(i32, i32)> = self.get(parent).map(|c| c.children.iter().filter(|ch| ch.slot == slot).map(|ch| self.location(&ch.name)).collect()).unwrap_or_default();
        let first_at = f.top.first().and_then(|t| f.pieces.iter().find(|p| p.name == *t)).and_then(|p| {
            p.parts.iter().find_map(|part| match part {
                Part::Prop(n, v) if n == "Location" => parse_pair(v).map(|(x, y)| (x as i32, y as i32)),
                _ => None,
            })
        });
        let mut offset = (0, 0);
        if let Some((x, y)) = first_at {
            while siblings.contains(&(x + offset.0, y + offset.1)) {
                offset = (offset.0 + 10, offset.1 + 10);
            }
        }

        for p in &f.pieces {
            let name = new_name(&p.name);
            let short = short_type(&p.type_name);
            let written = match (style.qualified, p.type_name.contains('.'), info(short).is_some()) {
                (true, false, true) => format!("System.Windows.Forms.{short}"),
                (false, true, true) if p.type_name == format!("System.Windows.Forms.{short}") && short != "Timer" => short.to_owned(),
                _ => p.type_name.clone(),
            };
            self.create(&name, &written, p.needs_container, p.init);
            let top = f.top.contains(&p.name);
            for part in &p.parts {
                match part {
                    Part::Prop(prop, value) => {
                        let value = match prop.as_str() {
                            "Name" => format_string(&name),
                            "Location" | "Size" | "ClientSize" => match parse_pair(value) {
                                Some((a, b)) => {
                                    let (dx, dy) = if top && prop == "Location" { offset } else { (0, 0) };
                                    format_pair(style, if prop == "Location" { "Point" } else { "Size" }, a as i64 + dx as i64, b as i64 + dy as i64)
                                }
                                None => rename(value),
                            },
                            _ => rename(value),
                        };
                        self.set_prop(&name, prop, Some(&value));
                    }
                    Part::Event(event, handler) if same_form => self.set_event(&name, event, Some(handler)),
                    Part::Event(..) => {}
                    Part::Own(rest) => {
                        let text = format!("{}{}", self.target(&name), rename(rest));
                        let c = self.get_mut(&name).expect("just created");
                        let key = member_key(&Member::Raw(text.clone()), &name, false).unwrap_or_default();
                        let at = sorted_position(&c.members, &key, &name, false);
                        c.members.insert(at, Member::Raw(text));
                    }
                    Part::Other(text) => {
                        let text = rename(text);
                        self.get_mut(&name).expect("just created").members.push(Member::Raw(text));
                    }
                }
            }
        }
        // What is inside the pasted containers, in their order.
        for p in &f.pieces {
            let name = new_name(&p.name);
            for (child, child_slot, args) in &p.children {
                self.add_child(&name, child_slot, &new_name(child), false);
                if let Some(ch) = self.get_mut(&name).and_then(|c| c.children.last_mut()) {
                    ch.args = args.clone();
                }
            }
            if !p.children.is_empty() || p.layout {
                self.ensure_layout_calls(&name);
            }
        }
        // The top controls go into the container, in front, in the order they had.
        let parent_type = self.get(parent).map(|c| c.short_type().to_owned()).unwrap_or_default();
        let arranged = matches!(parent_type.as_str(), "FlowLayoutPanel" | "TableLayoutPanel");
        let controls: Vec<String> = f.top.iter().map(|t| new_name(t)).filter(|n| self.get(n).is_some_and(|c| !is_component(&c.type_name) && !self.is_item(n))).collect();
        let order: Vec<&String> = if arranged { controls.iter().collect() } else { controls.iter().rev().collect() };
        for n in order {
            self.add_child(parent, slot, n, !arranged);
        }
        if !controls.is_empty() {
            self.ensure_layout_calls(parent);
        }
        f.top.iter().map(|t| new_name(t)).collect()
    }
}
