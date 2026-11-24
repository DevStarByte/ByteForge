//! The property grid's side of the model: a property's value as the grid shows it, and what the user
//! types turned back into code.

use super::*;

/// A property as the grid shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct GridValue {
    pub text: String,
    /// The code sets it (else it shows its default).
    pub set: bool,
}

const ANCHORS: [&str; 4] = ["Top", "Bottom", "Left", "Right"];

fn pair_input(input: &str) -> Option<(i64, i64)> {
    let nums: Vec<i64> = input.split([',', ' ', 'x', 'X', ';']).filter(|p| !p.trim().is_empty()).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match nums.as_slice() {
        [a, b] => Some((*a, *b)),
        _ => None,
    }
}

impl Designer {
    /// The value the grid shows for a property.
    pub fn grid_value(&self, owner: &str, p: &PropInfo) -> GridValue {
        let raw = self.prop(owner, p.name).map(str::trim);
        let default = p.default.unwrap_or("");
        let or_default = |s: Option<String>| s.unwrap_or_else(|| default.to_owned());
        let text = match p.kind {
            PropKind::Items => {
                let items = self.list_items(owner);
                return GridValue { text: items.join(", "), set: !items.is_empty() };
            }
            PropKind::Text => or_default(raw.map(|r| parse_string(r).unwrap_or_else(|| r.to_owned()))),
            PropKind::Int | PropKind::Code => or_default(raw.map(str::to_owned)),
            PropKind::Bool => raw.map(|r| parse_bool(r).map_or_else(|| r.to_owned(), |b| b.to_string())).unwrap_or_else(|| p.default.unwrap_or("false").to_owned()),
            PropKind::Decimal => or_default(raw.map(describe_decimal)),
            PropKind::Char => or_default(raw.map(|r| parse_char(r).map_or_else(|| r.to_owned(), |c| c.to_string()))),
            PropKind::Enum(..) => or_default(raw.map(|r| parse_enum(r).unwrap_or_else(|| r.to_owned()))),
            PropKind::Point | PropKind::Size => match raw.and_then(parse_pair) {
                Some((a, b)) => format!("{a}, {b}"),
                None if matches!(p.name, "Size" | "ClientSize") => {
                    let (w, h) = self.size(owner);
                    format!("{w}, {h}")
                }
                None => or_default(raw.map(str::to_owned)),
            },
            PropKind::Color => or_default(raw.map(describe_color)),
            PropKind::Font => or_default(raw.map(|r| parse_font(r).map_or_else(|| r.to_owned(), |f| describe_font(&f)))),
            PropKind::Anchor => match raw {
                Some(r) => {
                    let flags = parse_flags(r, "AnchorStyles");
                    if flags.is_empty() { r.to_owned() } else { flags.join(", ") }
                }
                None => default.to_owned(),
            },
            PropKind::Reference => or_default(raw.map(|r| strip_this(r).to_owned())),
        };
        GridValue { text, set: raw.is_some() }
    }

    /// What the edit box starts with: the shown value (a list's items one per line).
    pub fn grid_edit_text(&self, owner: &str, p: &PropInfo) -> String {
        match p.kind {
            PropKind::Items => self.list_items(owner).join("\n"),
            _ => self.grid_value(owner, p).text,
        }
    }

    /// Sets a property from what was typed in the grid.
    pub fn set_from_grid(&mut self, owner: &str, p: &PropInfo, input: &str) -> Result<(), String> {
        let t = input.trim();
        if p.kind == PropKind::Items {
            let items: Vec<String> = input.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
            self.set_list_items(owner, &items);
            return Ok(());
        }
        if let Some(default) = p.default
            && p.kind != PropKind::Text
            && t.eq_ignore_ascii_case(default)
        {
            self.set_prop(owner, p.name, None);
            return Ok(());
        }
        let style = self.style;
        let code = match p.kind {
            PropKind::Text => {
                if p.default == Some(input) {
                    None
                } else {
                    Some(format_string(input))
                }
            }
            PropKind::Int => Some(t.parse::<i64>().map_err(|_| format!("{} takes a whole number.", p.name))?.to_string()),
            PropKind::Bool => match t.to_ascii_lowercase().as_str() {
                "true" => Some("true".into()),
                "false" => Some("false".into()),
                _ => return Err(format!("{} is true or false.", p.name)),
            },
            PropKind::Decimal => Some(format_decimal(t.parse::<f64>().map_err(|_| format!("{} takes a number.", p.name))?)),
            PropKind::Char => {
                let mut chars = input.chars();
                match (chars.next(), chars.next()) {
                    (None, _) => None,
                    (Some(c), None) => Some(format_char(c)),
                    _ => return Err(format!("{} takes one character.", p.name)),
                }
            }
            PropKind::Enum(ty, members) => {
                let m = members.iter().find(|m| m.eq_ignore_ascii_case(t)).ok_or_else(|| format!("{} is one of: {}.", p.name, members.join(", ")))?;
                Some(format_enum(style, ty, m))
            }
            PropKind::Point | PropKind::Size => {
                let (a, b) = pair_input(t).ok_or_else(|| format!("{} takes two numbers, like 12, 34.", p.name))?;
                match p.name {
                    "Location" => {
                        self.set_location(owner, (a as i32, b as i32));
                        return Ok(());
                    }
                    "Size" | "ClientSize" => {
                        if a < 1 || b < 1 {
                            return Err(format!("{} must be at least 1, 1.", p.name));
                        }
                        self.set_size(owner, (a as i32, b as i32));
                        return Ok(());
                    }
                    _ => Some(format_pair(style, "Size", a, b)),
                }
            }
            PropKind::Color => {
                if t.is_empty() {
                    None
                } else {
                    Some(format_color(style, t).ok_or("Type a color name (Red, SystemColors.Control), #RRGGBB or r, g, b.")?)
                }
            }
            PropKind::Font => {
                if t.is_empty() {
                    None
                } else {
                    Some(format_font(style, &read_font(t).ok_or("Type a font like: Segoe UI, 12pt, Bold")?))
                }
            }
            PropKind::Anchor => {
                let parts: Vec<String> = t.split([',', '|', ' ']).map(str::trim).filter(|p| !p.is_empty()).map(str::to_ascii_lowercase).collect();
                if let Some(bad) = parts.iter().find(|p| *p != "none" && !ANCHORS.iter().any(|a| a.eq_ignore_ascii_case(p))) {
                    return Err(format!("'{bad}' isn't an anchor; use Top, Bottom, Left and Right."));
                }
                let chosen: Vec<&str> = ANCHORS.iter().copied().filter(|a| parts.iter().any(|p| a.eq_ignore_ascii_case(p))).collect();
                if chosen == ["Top", "Left"] {
                    None
                } else if chosen.is_empty() {
                    Some(format_enum(style, "AnchorStyles", "None"))
                } else {
                    Some(format_flags(style, "AnchorStyles", &chosen))
                }
            }
            PropKind::Reference => {
                if t.is_empty() {
                    None
                } else if self.get(t).is_some() && !self.is_root(t) {
                    Some(format!("{}{t}", self.this()))
                } else {
                    return Err(format!("The form has no component named '{t}'."));
                }
            }
            PropKind::Code => (!t.is_empty()).then(|| t.to_owned()),
            PropKind::Items => unreachable!(),
        };
        self.set_prop(owner, p.name, code.as_deref());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::NET8;
    use super::*;

    fn p(type_name: &str, root: bool, name: &str) -> &'static PropInfo {
        property(type_name, root, name).unwrap_or_else(|| panic!("{type_name} has {name}"))
    }

    #[test]
    fn shows_and_sets_values() {
        let mut d = Designer::parse(NET8, "Form").unwrap();
        let show = |d: &Designer, owner: &str, t: &str, name: &str| d.grid_value(owner, p(t, d.is_root(owner), name));
        assert_eq!(show(&d, "button1", "Button", "Text"), GridValue { text: "OK".into(), set: true });
        assert_eq!(show(&d, "button1", "Button", "Dock"), GridValue { text: "None".into(), set: false });
        assert_eq!(show(&d, "button1", "Button", "Anchor").text, "Bottom, Right");
        assert_eq!(show(&d, "button1", "Button", "Enabled").text, "true");
        assert_eq!(show(&d, "Form1", "Form", "ClientSize").text, "800, 450");
        assert_eq!(show(&d, "comboBox1", "ComboBox", "Items").text, "One, Two");

        let set = |d: &mut Designer, owner: &str, t: &str, name: &str, v: &str| {
            let root = d.is_root(owner);
            d.set_from_grid(owner, p(t, root, name), v)
        };
        set(&mut d, "button1", "Button", "Dock", "fill").unwrap();
        assert_eq!(d.prop("button1", "Dock"), Some("DockStyle.Fill"));
        set(&mut d, "button1", "Button", "Dock", "None").unwrap();
        assert_eq!(d.prop("button1", "Dock"), None, "back to the default: out of the code");
        set(&mut d, "button1", "Button", "Anchor", "Top, Left").unwrap();
        assert_eq!(d.prop("button1", "Anchor"), None);
        set(&mut d, "button1", "Button", "Anchor", "left right top").unwrap();
        assert_eq!(d.prop("button1", "Anchor"), Some("AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right"));
        set(&mut d, "button1", "Button", "BackColor", "#336699").unwrap();
        assert_eq!(show(&d, "button1", "Button", "BackColor").text, "51, 102, 153");
        set(&mut d, "button1", "Button", "Font", "Segoe UI, 11pt, Italic").unwrap();
        assert_eq!(show(&d, "button1", "Button", "Font").text, "Segoe UI, 11pt, Italic");
        set(&mut d, "button1", "Button", "Location", "1, 2").unwrap();
        assert_eq!(d.location("button1"), (1, 2));
        set(&mut d, "Form1", "Form", "AcceptButton", "button1").unwrap();
        assert_eq!(d.prop("Form1", "AcceptButton"), Some("button1"));
        set(&mut d, "comboBox1", "ComboBox", "Items", "a\n\nb \n").unwrap();
        assert_eq!(d.list_items("comboBox1"), ["a", "b"]);
        assert!(set(&mut d, "button1", "Button", "TabIndex", "x").is_err());
        assert!(set(&mut d, "button1", "Button", "Dock", "Middle").is_err());
        assert!(set(&mut d, "button1", "Button", "BackColor", "nope").is_err());
        assert!(set(&mut d, "button1", "Button", "Size", "0, 5").is_err());
        assert!(set(&mut d, "Form1", "Form", "AcceptButton", "nothing").is_err());
        // Whatever was typed, the file still reads back the same.
        let text = d.to_text();
        assert_eq!(Designer::parse(&text, "Form").unwrap().to_text(), text);
    }
}
