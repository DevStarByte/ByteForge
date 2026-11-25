//! A localized form's `.resx` files in the designer: read next to the form (`Form1.resx`,
//! `Form1.de.resx`, …), the language shown, changes kept until the designer is saved, and undo and redo
//! of steps that changed the code, the resources or both.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bf_core::winforms::{self as wf, Designer, FormResources};

/// The texts of a form's `.resx` files.
#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct ResTexts {
    pub neutral: String,
    /// culture → text.
    pub cultures: BTreeMap<String, String>,
    /// The neutral file starts with a byte order mark (written back the same; new files get one too).
    pub bom: bool,
}

/// The resources of a form: its files, as loaded or last saved, and as changed since.
#[derive(Clone, Debug)]
pub(super) struct Resources {
    /// `Form1.resx`.
    pub neutral_path: PathBuf,
    pub saved: ResTexts,
    pub texts: ResTexts,
}

fn read(path: &Path) -> Option<(String, bool)> {
    let bytes = std::fs::read(path).ok()?;
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    Some((String::from_utf8_lossy(if bom { &bytes[3..] } else { &bytes[..] }).into_owned(), bom))
}

impl Resources {
    /// The `.resx` files of the form whose code file is `code` (`Form1.cs` → `Form1.resx` and its
    /// languages), when it has them.
    pub fn load(code: &Path) -> Option<Self> {
        let neutral_path = code.with_extension("resx");
        let (neutral, bom) = read(&neutral_path)?;
        let family = bf_core::resx::family(&neutral_path)?;
        let cultures = family.cultures.iter().filter_map(|(c, p)| read(p).map(|(t, _)| (c.clone(), t))).collect();
        let texts = ResTexts { neutral, cultures, bom };
        Some(Self { neutral_path, saved: texts.clone(), texts })
    }

    pub fn languages(&self) -> Vec<String> {
        self.texts.cultures.keys().cloned().collect()
    }

    /// The path of a language's file (`Form1.de.resx`).
    fn path_of(&self, culture: &str) -> PathBuf {
        let stem = self.neutral_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.neutral_path.with_file_name(format!("{stem}.{culture}.resx"))
    }

    /// What the designer shows for `language` (`None`: the neutral one).
    pub fn for_language(&self, language: Option<&str>) -> FormResources {
        let parse = |t: &str| bf_core::resx::parse_resources(t).unwrap_or_default();
        FormResources {
            neutral: parse(&self.texts.neutral),
            culture: language.map(|l| (l.to_owned(), self.texts.cultures.get(l).map(|t| parse(t)).unwrap_or_default())),
        }
    }

    /// Writes the model's resource edits into the texts.
    pub fn apply(&mut self, edits: &[wf::ResEdit]) -> Result<(), String> {
        if edits.is_empty() {
            return Ok(());
        }
        let mut texts = self.texts.clone();
        wf::write_res_edits(&mut texts.neutral, &mut texts.cultures, edits).map_err(|e| format!("Could not change {}: {e}", self.neutral_path.display()))?;
        self.texts = texts;
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.texts != self.saved
    }

    /// Writes the files that changed.
    pub fn save(&mut self) -> Result<(), String> {
        let mut files: Vec<(PathBuf, &String, Option<&String>)> = vec![(self.neutral_path.clone(), &self.texts.neutral, Some(&self.saved.neutral))];
        for (c, t) in &self.texts.cultures {
            files.push((self.path_of(c), t, self.saved.cultures.get(c)));
        }
        for (path, text, saved) in files {
            if saved == Some(text) {
                continue;
            }
            let mut bytes = Vec::with_capacity(text.len() + 3);
            if self.texts.bom {
                bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            bytes.extend_from_slice(text.as_bytes());
            std::fs::write(&path, bytes).map_err(|e| format!("Could not save {}: {e}", path.display()))?;
        }
        self.saved = self.texts.clone();
        Ok(())
    }
}

/// Gives a parsed form its resources (when it is localized and has them) in `language`.
pub(super) fn attach(doc: &mut Designer, res: Option<&Resources>, language: Option<&str>) {
    let shown = res.filter(|_| doc.is_localized()).map(|r| r.for_language(language));
    doc.set_resources(shown);
}

/// A change the designer made: the code and the resources before and after it.
#[derive(Clone, Debug)]
pub(super) struct Step {
    pub code_before: String,
    pub code_after: String,
    pub res_before: Option<ResTexts>,
    pub res_after: Option<ResTexts>,
}
