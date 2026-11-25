//! Localized forms: what `resources.ApplyResources(button1, "button1")` sets on a control comes from the
//! form's `.resx` files (`button1.Text`, `button1.Location`, …; the form's own as `$this.Text`). The neutral
//! file has every value; a language's file (`Form1.de.resx`) overrides some of them. The designer shows the
//! values of one language and writes changes of those properties back to its file, not into the code.

use std::collections::{BTreeMap, HashMap};

use crate::resx::Resource;

use super::*;

/// A form's resources in the language the designer shows.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FormResources {
    /// The neutral file's values.
    pub neutral: Vec<Resource>,
    /// The language shown and its file's values (`None`: the neutral language).
    pub culture: Option<(String, Vec<Resource>)>,
}

/// A change to a form's `.resx` files, for the designer to write.
#[derive(Clone, Debug, PartialEq)]
pub enum ResEdit {
    /// Sets a value in the language's file (`None`: the neutral file).
    Set { culture: Option<String>, name: String, value: String, type_name: Option<String> },
    /// Takes a value out of a language's file (the neutral value shows again).
    Remove { culture: String, name: String },
    /// A deleted component: every value of its key (`button1.*`, `>>button1.*`) goes from every file.
    RemoveKey(String),
}

/// The type of a resource without its assembly: `System.Drawing.Point, System.Drawing` → `System.Drawing.Point`.
fn base_type(type_name: Option<&str>) -> &str {
    type_name.and_then(|t| t.split(',').next()).map_or("System.String", str::trim)
}

/// A resource's value as code (`12, 12` of a Point → `new Point(12, 12)`); `None` for kinds the designer
/// doesn't convert (fonts, images).
pub fn resx_to_code(style: Style, value: &str, type_name: Option<&str>) -> Option<String> {
    let ty = base_type(type_name);
    let short = ty.rsplit('.').next().unwrap_or(ty);
    let pair = || {
        let (a, b) = value.split_once(',')?;
        Some((a.trim().parse::<i64>().ok()?, b.trim().parse::<i64>().ok()?))
    };
    match ty {
        "System.String" => Some(format_string(value)),
        "System.Drawing.Point" | "System.Drawing.Size" => pair().map(|(a, b)| format_pair(style, short, a, b)),
        "System.Int32" => value.trim().parse::<i64>().ok().map(|n| n.to_string()),
        "System.Boolean" => match value.trim().to_ascii_lowercase().as_str() {
            "true" => Some("true".into()),
            "false" => Some("false".into()),
            _ => None,
        },
        _ if ty.starts_with("System.Windows.Forms.") || ty == "System.Drawing.ContentAlignment" => {
            let members: Vec<&str> = value.split(',').map(str::trim).collect();
            members.iter().all(|m| lex::is_identifier(m)).then(|| format_flags(style, short, &members))
        }
        _ => None,
    }
}

/// Code as a resource's value (of the resource's type); `None` when it isn't a plain value of that type.
pub fn code_to_resx(code: &str, type_name: Option<&str>) -> Option<String> {
    let ty = base_type(type_name);
    let short = ty.rsplit('.').next().unwrap_or(ty);
    match ty {
        "System.String" => parse_string(code),
        "System.Drawing.Point" | "System.Drawing.Size" => parse_pair(code).map(|(a, b)| format!("{}, {}", a as i64, b as i64)),
        "System.Int32" => parse_int(code).map(|n| n.to_string()),
        "System.Boolean" => parse_bool(code).map(|b| if b { "True".into() } else { "False".into() }),
        _ if ty.starts_with("System.Windows.Forms.") || ty == "System.Drawing.ContentAlignment" => {
            let members = parse_flags(code, short);
            (!members.is_empty()).then(|| members.join(", "))
        }
        _ => None,
    }
}

impl Designer {
    /// The key of a component's resources (`button1`, the form's `$this`), when the code applies them.
    pub fn resource_key(&self, name: &str) -> Option<String> {
        let c = self.get(name)?;
        c.members.iter().find_map(|m| {
            let Member::Raw(text) = m else { return None };
            if !text.contains(".ApplyResources(") {
                return None;
            }
            let args = lex::split_args(lex::call_args(text)?);
            let [target, key] = args.as_slice() else { return None };
            let target = strip_this(target);
            let target = if target == "this" { self.class_name.as_str() } else { target };
            (target == name).then(|| parse_string(key)).flatten()
        })
    }

    /// Whether the form is localized (its code applies resources).
    pub fn is_localized(&self) -> bool {
        self.comps.iter().any(|c| self.resource_key(&c.name).is_some())
    }

    pub fn resources(&self) -> Option<&FormResources> {
        self.res.as_ref()
    }

    /// The form's resources in the language to show (`None`: as if it had none).
    pub fn set_resources(&mut self, res: Option<FormResources>) {
        self.res = res;
        self.rebuild_res_props();
    }

    /// The changes to the `.resx` files since the last call.
    pub fn take_resource_edits(&mut self) -> Vec<ResEdit> {
        std::mem::take(&mut self.res_edits)
    }

    /// The properties the resources give components, as code.
    pub(super) fn rebuild_res_props(&mut self) {
        self.res_props.clear();
        let Some(res) = &self.res else { return };
        let style = self.style;
        let keys: Vec<(String, String)> = self.comps.iter().filter_map(|c| self.resource_key(&c.name).map(|k| (c.name.clone(), k))).collect();
        let mut props: HashMap<(String, String), String> = HashMap::new();
        let culture = res.culture.as_ref().map(|(_, r)| r.as_slice()).unwrap_or_default();
        // Neutral first, the language's values over them.
        for r in res.neutral.iter().chain(culture) {
            let Some((key, prop)) = r.name.rsplit_once('.') else { continue };
            if !lex::is_identifier(prop) {
                continue;
            }
            for (comp, k) in &keys {
                if k == key
                    && let Some(code) = resx_to_code(style, &r.value, r.type_name.as_deref())
                {
                    props.insert((comp.clone(), prop.to_owned()), code);
                }
            }
        }
        self.res_props = props;
    }

    /// Sets a property in the resources when that is where it lives (the neutral file has it and the code
    /// doesn't set it). False: it goes into the code.
    pub(super) fn set_resource_prop(&mut self, owner: &str, prop: &str, value: Option<&str>) -> bool {
        let Some(res) = &self.res else { return false };
        let Some(key) = self.resource_key(owner) else { return false };
        let entry = format!("{key}.{prop}");
        let Some(neutral) = res.neutral.iter().find(|r| r.name == entry) else { return false };
        let in_code = self.get(owner).is_some_and(|c| c.members.iter().any(|m| matches!(m, Member::Prop { name, .. } if name == prop)));
        if in_code {
            return false;
        }
        let type_name = neutral.type_name.clone();
        let culture = res.culture.as_ref().map(|(c, _)| c.clone());
        let edit = match value {
            Some(code) => {
                let Some(value) = code_to_resx(code, type_name.as_deref()) else { return false };
                ResEdit::Set { culture, name: entry, value, type_name }
            }
            // Back to the default: a language falls back to the neutral value; the neutral one stays.
            None => match culture {
                Some(culture) => ResEdit::Remove { culture, name: entry },
                None => return true,
            },
        };
        self.apply_res_edit(&edit);
        self.res_edits.push(edit);
        self.rebuild_res_props();
        true
    }

    /// Applies an edit to the resources held (so the form shows it before the files are written).
    fn apply_res_edit(&mut self, edit: &ResEdit) {
        let Some(res) = &mut self.res else { return };
        match edit {
            ResEdit::Set { culture, name, value, type_name } => {
                let list = match (culture, &mut res.culture) {
                    (None, _) => &mut res.neutral,
                    (Some(c), Some((shown, list))) if c == shown => list,
                    _ => return,
                };
                match list.iter_mut().find(|r| r.name == *name) {
                    Some(r) => r.value = value.clone(),
                    None => list.push(Resource { name: name.clone(), value: value.clone(), type_name: type_name.clone() }),
                }
            }
            ResEdit::Remove { culture, name } => {
                if let Some((shown, list)) = &mut res.culture
                    && shown == culture
                {
                    list.retain(|r| r.name != *name);
                }
            }
            ResEdit::RemoveKey(key) => {
                let of_key = |r: &Resource| r.name.strip_prefix(">>").unwrap_or(&r.name).rsplit_once('.').is_some_and(|(k, _)| k == key);
                res.neutral.retain(|r| !of_key(r));
                if let Some((_, list)) = &mut res.culture {
                    list.retain(|r| !of_key(r));
                }
            }
        }
    }

    /// Deleting components: their resources go too.
    pub(super) fn forget_resources(&mut self, names: &[String]) {
        for n in names {
            if let Some(key) = self.resource_key(n) {
                let edit = ResEdit::RemoveKey(key);
                self.apply_res_edit(&edit);
                self.res_edits.push(edit);
            }
        }
    }
}

/// Writes resource edits into the texts of a form's `.resx` files: `neutral` and each language's
/// (`texts`: culture → text; a language without a file gets one like the neutral file, without values).
pub fn write_res_edits(neutral: &mut String, cultures: &mut BTreeMap<String, String>, edits: &[ResEdit]) -> anyhow::Result<()> {
    for edit in edits {
        match edit {
            ResEdit::Set { culture: None, name, value, type_name } => *neutral = crate::resx::set_typed(neutral, name, value, type_name.as_deref())?,
            ResEdit::Set { culture: Some(c), name, value, type_name } => {
                let text = match cultures.get(c) {
                    Some(t) => t.clone(),
                    None => crate::resx::empty_of_values(neutral)?,
                };
                cultures.insert(c.clone(), crate::resx::set_typed(&text, name, value, type_name.as_deref())?);
            }
            ResEdit::Remove { culture, name } => {
                if let Some(text) = cultures.get_mut(culture) {
                    *text = crate::resx::remove(text, name)?;
                }
            }
            ResEdit::RemoveKey(key) => {
                for text in std::iter::once(&mut *neutral).chain(cultures.values_mut()) {
                    let names: Vec<String> = crate::resx::parse_resources(text)?
                        .into_iter()
                        .map(|r| r.name)
                        .filter(|n| n.strip_prefix(">>").unwrap_or(n).rsplit_once('.').is_some_and(|(k, _)| k == key))
                        .collect();
                    for n in names {
                        *text = crate::resx::remove(text, &n)?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A localized form as Visual Studio writes it (.NET Framework).
    pub(crate) const LOCALIZED: &str = "namespace Loc\n{\n    partial class MainForm\n    {\n        private System.ComponentModel.IContainer components = null;\n\n        private void InitializeComponent()\n        {\n            System.ComponentModel.ComponentResourceManager resources = new System.ComponentModel.ComponentResourceManager(typeof(MainForm));\n            this.button1 = new System.Windows.Forms.Button();\n            this.SuspendLayout();\n            // \n            // button1\n            // \n            resources.ApplyResources(this.button1, \"button1\");\n            this.button1.Name = \"button1\";\n            this.button1.UseVisualStyleBackColor = true;\n            // \n            // MainForm\n            // \n            resources.ApplyResources(this, \"$this\");\n            this.AutoScaleMode = System.Windows.Forms.AutoScaleMode.Font;\n            this.Controls.Add(this.button1);\n            this.Name = \"MainForm\";\n            this.ResumeLayout(false);\n\n        }\n\n        private System.Windows.Forms.Button button1;\n    }\n}\n";

    pub(crate) const NEUTRAL: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root>\n  <data name=\"button1.Location\" type=\"System.Drawing.Point, System.Drawing\">\n    <value>12, 12</value>\n  </data>\n  <data name=\"button1.Size\" type=\"System.Drawing.Size, System.Drawing\">\n    <value>75, 23</value>\n  </data>\n  <data name=\"button1.TabIndex\" type=\"System.Int32, mscorlib\">\n    <value>0</value>\n  </data>\n  <data name=\"button1.Text\" xml:space=\"preserve\">\n    <value>OK</value>\n  </data>\n  <data name=\"&gt;&gt;button1.Name\" xml:space=\"preserve\">\n    <value>button1</value>\n  </data>\n  <data name=\"$this.ClientSize\" type=\"System.Drawing.Size, System.Drawing\">\n    <value>284, 261</value>\n  </data>\n  <data name=\"$this.Text\" xml:space=\"preserve\">\n    <value>Main</value>\n  </data>\n</root>\n";

    pub(crate) const GERMAN: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root>\n  <data name=\"button1.Text\" xml:space=\"preserve\">\n    <value>Gut</value>\n  </data>\n  <data name=\"$this.Text\" xml:space=\"preserve\">\n    <value>Haupt</value>\n  </data>\n</root>\n";

    fn resources(culture: Option<&str>) -> FormResources {
        FormResources {
            neutral: crate::resx::parse_resources(NEUTRAL).unwrap(),
            culture: culture.map(|c| (c.to_owned(), crate::resx::parse_resources(GERMAN).unwrap())),
        }
    }

    #[test]
    fn shows_the_resources_of_a_language() {
        let mut d = Designer::parse(LOCALIZED, "Form").unwrap();
        assert!(d.is_localized());
        assert_eq!((d.resource_key("button1").as_deref(), d.resource_key("MainForm").as_deref()), (Some("button1"), Some("$this")));
        // Without its resources the button has no place or text.
        assert_eq!(d.text_of("button1"), None);
        d.set_resources(Some(resources(None)));
        assert_eq!((d.location("button1"), d.size("button1"), d.text_of("button1").as_deref()), ((12, 12), (75, 23), Some("OK")));
        assert_eq!((d.size("MainForm"), d.text_of("MainForm").as_deref()), ((284, 261), Some("Main")));
        assert_eq!(d.prop("button1", "Location"), Some("new System.Drawing.Point(12, 12)"), "as code in the file's style");
        d.set_resources(Some(resources(Some("de"))));
        assert_eq!((d.text_of("button1").as_deref(), d.text_of("MainForm").as_deref(), d.location("button1")), (Some("Gut"), Some("Haupt"), (12, 12)));
    }

    #[test]
    fn changes_go_to_the_language_shown() {
        let mut d = Designer::parse(LOCALIZED, "Form").unwrap();
        d.set_resources(Some(resources(Some("de"))));
        d.set_location("button1", (20, 30));
        d.set_prop("button1", "Text", Some("\"Prima\""));
        d.set_prop("button1", "UseVisualStyleBackColor", Some("false"));
        assert_eq!(d.location("button1"), (20, 30));
        assert_eq!(d.text_of("button1").as_deref(), Some("Prima"));
        // The code changed only where the code sets it.
        let text = d.to_text();
        assert!(text.contains("this.button1.UseVisualStyleBackColor = false;"), "{text}");
        assert!(!text.contains("Location") && !text.contains("Prima"), "{text}");
        let edits = d.take_resource_edits();
        assert_eq!(edits[0], ResEdit::Set { culture: Some("de".into()), name: "button1.Location".into(), value: "20, 30".into(), type_name: Some("System.Drawing.Point, System.Drawing".into()) });
        assert!(d.take_resource_edits().is_empty(), "taken");
        let (mut neutral, mut cultures) = (NEUTRAL.to_owned(), BTreeMap::from([("de".to_owned(), GERMAN.to_owned())]));
        write_res_edits(&mut neutral, &mut cultures, &edits).unwrap();
        assert_eq!(neutral, NEUTRAL, "the neutral file is untouched");
        let de = &cultures["de"];
        assert!(de.contains("<data name=\"button1.Location\" type=\"System.Drawing.Point, System.Drawing\" xml:space=\"preserve\">\n    <value>20, 30</value>"), "{de}");
        assert!(de.contains("<value>Prima</value>"), "{de}");
        // Back to the default in German: the neutral text shows again.
        d.set_prop("button1", "Text", None);
        assert_eq!(d.text_of("button1").as_deref(), Some("OK"));
        assert_eq!(d.take_resource_edits(), [ResEdit::Remove { culture: "de".into(), name: "button1.Text".into() }]);
    }

    #[test]
    fn deleting_and_copying_localized_controls() {
        let mut d = Designer::parse(LOCALIZED, "Form").unwrap();
        d.set_resources(Some(resources(None)));
        // Copied into another form, its resources' values come along as code.
        let f = d.copy(&["button1".into()]);
        let mut other = super::super::tests::net8();
        let pasted = other.paste(&f, "Form1", "");
        assert_eq!(other.text_of(&pasted[0]).as_deref(), Some("OK"));
        assert!(!other.to_text().contains("ApplyResources"));
        // Deleted, its values go from every language's file.
        d.remove("button1");
        let edits = d.take_resource_edits();
        assert_eq!(edits, [ResEdit::RemoveKey("button1".into())]);
        let (mut neutral, mut cultures) = (NEUTRAL.to_owned(), BTreeMap::from([("de".to_owned(), GERMAN.to_owned())]));
        write_res_edits(&mut neutral, &mut cultures, &edits).unwrap();
        assert!(!neutral.contains("button1") && neutral.contains("$this.Text"), "{neutral}");
        assert!(!cultures["de"].contains("button1"));
        // A new language's file is made like the neutral one.
        let edit = ResEdit::Set { culture: Some("fr".into()), name: "$this.Text".into(), value: "Principal".into(), type_name: None };
        write_res_edits(&mut neutral, &mut cultures, &[edit]).unwrap();
        assert!(cultures["fr"].contains("<value>Principal</value>") && !cultures["fr"].contains("ClientSize"), "{}", cultures["fr"]);
    }

    #[test]
    fn values_both_ways() {
        let st = Style { this: false, qualified: false };
        assert_eq!(resx_to_code(st, "Top, Right", Some("System.Windows.Forms.AnchorStyles, System.Windows.Forms")).as_deref(), Some("AnchorStyles.Top | AnchorStyles.Right"));
        assert_eq!(code_to_resx("AnchorStyles.Top | AnchorStyles.Right", Some("System.Windows.Forms.AnchorStyles, System.Windows.Forms")).as_deref(), Some("Top, Right"));
        assert_eq!(resx_to_code(st, "True", Some("System.Boolean, mscorlib")).as_deref(), Some("true"));
        assert_eq!(code_to_resx("false", Some("System.Boolean, mscorlib")).as_deref(), Some("False"));
        assert_eq!(resx_to_code(st, "Segoe UI, 12pt", Some("System.Drawing.Font, System.Drawing")), None, "fonts stay as they are");
    }
}
