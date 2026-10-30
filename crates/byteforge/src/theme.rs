//! Resolved theme as a GPUI global: every color, size, font and editor option is looked up here
//! by key, so the whole UI follows the theme files.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bf_core::ThemeData;
use gpui::{
    App, Background, BoxShadow, Edges, FontWeight, Global, Hsla, Pixels, Rgba, SharedString, linear_color_stop,
    linear_gradient, point, px,
};

#[derive(Clone, Debug)]
pub struct TokenStyle {
    pub color: Hsla,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretStyle {
    Line,
    Block,
    BlockOutline,
    Underline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretBlink {
    Blink,
    Smooth,
    Phase,
    Solid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineHighlight {
    Fill,
    Border,
    Gutter,
    All,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whitespace {
    None,
    Leading,
    All,
}

/// Editor look & behavior from the theme's "editor" section.
#[derive(Clone, Debug)]
pub struct EditorStyle {
    pub caret_style: CaretStyle,
    pub caret_blink: CaretBlink,
    pub blink_interval_ms: u64,
    pub caret_width: f32,
    pub smooth_caret: bool,
    pub animation_speed: f32,
    pub line_highlight: LineHighlight,
    pub selection_radius: f32,
    pub whitespace: Whitespace,
    pub squiggle_wavy: bool,
    pub bracket_colors: bool,
    pub match_brackets: bool,
    pub indent_guides: bool,
    pub rulers: Vec<usize>,
    pub text_padding: f32,
    pub gutter_left: f32,
    pub gutter_right: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub line_number_size: f32,
    pub font_weight: FontWeight,
}

pub struct Theme {
    pub data: Arc<ThemeData>,
    pub version: usize,
    backgrounds: HashMap<String, Background>,
    colors: HashMap<String, Hsla>,
    tokens: HashMap<String, TokenStyle>,
    pub ui_font: SharedString,
    pub editor_font: SharedString,
    pub editor: EditorStyle,
    pub layout: crate::layout::LayoutConfig,
}

/// The active theme as a GPUI global (an Arc, so reading it never keeps the app context borrowed).
pub struct ThemeGlobal(pub Arc<Theme>);

impl Global for ThemeGlobal {}

static FONTS: std::sync::OnceLock<HashSet<String>> = std::sync::OnceLock::new();

impl Theme {
    pub fn global(cx: &App) -> Arc<Theme> {
        cx.global::<ThemeGlobal>().0.clone()
    }

    pub fn install(self, cx: &mut App) {
        cx.set_global(ThemeGlobal(Arc::new(self)));
    }

    pub fn build(data: Arc<ThemeData>, version: usize, cx: &App) -> Self {
        let fonts = FONTS.get_or_init(|| cx.text_system().all_font_names().into_iter().map(|f| f.to_lowercase()).collect());
        let mut backgrounds = HashMap::new();
        let mut colors = HashMap::new();
        for (key, value) in &data.colors {
            if let Some((bg, color)) = parse_background(value) {
                backgrounds.insert(key.clone(), bg);
                colors.insert(key.clone(), color);
            }
        }
        let foreground = colors.get("editor.foreground").copied().unwrap_or(gpui::white());
        let tokens = data
            .token_colors
            .iter()
            .map(|(k, s)| {
                let color = s.foreground.as_deref().and_then(parse_color).unwrap_or(foreground);
                (k.clone(), TokenStyle { color, bold: s.bold, italic: s.italic, underline: s.underline })
            })
            .collect();

        let pick_font = |key: &str, defaults: &[&str]| -> SharedString {
            let wanted = data.fonts.get(key).map(String::as_str).unwrap_or("default");
            let candidates: Vec<&str> = if wanted.eq_ignore_ascii_case("default") {
                defaults.to_vec()
            } else {
                wanted.split(',').map(str::trim).chain(defaults.iter().copied()).collect()
            };
            candidates
                .iter()
                .find(|c| fonts.contains(&c.to_lowercase()))
                .map(|c| SharedString::from(c.to_string()))
                .unwrap_or_else(|| SharedString::from(defaults[0].to_string()))
        };
        let ui_font = pick_font("ui.family", &["Inter", "Segoe UI", "Noto Sans", "Cantarell", "DejaVu Sans", "Ubuntu", "Arial"]);
        let editor_font = pick_font(
            "editor.family",
            &["JetBrains Mono", "Cascadia Code", "Fira Code", "Hack", "DejaVu Sans Mono", "Noto Sans Mono", "Liberation Mono", "Consolas", "Courier New"],
        );

        let e = &data.editor;
        let num = |section: &bf_core::theme::Section, key: &str, d: f32| data.number(section, key, d);
        let opt = |key: &str, d: &str| e.get(key).map(|v| v.to_lowercase().replace(['-', '_'], "")).unwrap_or_else(|| d.to_owned());
        let editor = EditorStyle {
            caret_style: match opt("cursorStyle", "line").as_str() {
                "block" => CaretStyle::Block,
                "blockoutline" => CaretStyle::BlockOutline,
                "underline" => CaretStyle::Underline,
                _ => CaretStyle::Line,
            },
            caret_blink: match opt("cursorBlinking", "smooth").as_str() {
                "blink" => CaretBlink::Blink,
                "phase" => CaretBlink::Phase,
                "solid" => CaretBlink::Solid,
                _ => CaretBlink::Smooth,
            },
            blink_interval_ms: num(e, "cursorBlinkInterval", 530.0) as u64,
            caret_width: num(e, "cursorWidth", num(&data.metrics, "editor.caretWidth", 2.0)),
            smooth_caret: data.flag(e, "smoothCaretAnimation", true),
            animation_speed: num(e, "animationSpeed", 0.05),
            line_highlight: match opt("lineHighlight", "fill").as_str() {
                "border" => LineHighlight::Border,
                "gutter" => LineHighlight::Gutter,
                "all" => LineHighlight::All,
                "none" => LineHighlight::None,
                _ => LineHighlight::Fill,
            },
            selection_radius: num(e, "selectionRadius", 3.0),
            whitespace: match opt("renderWhitespace", "none").as_str() {
                "all" => Whitespace::All,
                "leading" => Whitespace::Leading,
                _ => Whitespace::None,
            },
            squiggle_wavy: opt("squiggleStyle", "wavy") == "wavy",
            bracket_colors: data.flag(e, "bracketPairColorization", true),
            match_brackets: data.flag(e, "matchBrackets", true),
            indent_guides: data.flag(e, "indentGuides", true),
            rulers: e
                .get("rulers")
                .map(|r| r.split(|c: char| !c.is_ascii_digit()).filter_map(|s| s.parse().ok()).collect())
                .unwrap_or_default(),
            text_padding: num(e, "textPadding", 8.0),
            gutter_left: num(e, "gutterPaddingLeft", 16.0),
            gutter_right: num(e, "gutterPaddingRight", 16.0),
            font_size: num(&data.fonts, "editor.size", 14.0),
            line_height: num(&data.fonts, "editor.lineHeight", 1.5),
            line_number_size: num(&data.fonts, "editor.lineNumberSize", 0.0),
            font_weight: data.fonts.get("editor.weight").and_then(|w| parse_weight(w)).unwrap_or(FontWeight::NORMAL),
        };

        let layout = crate::layout::LayoutConfig::parse(&data.layout);
        Self { data, version, backgrounds, colors, tokens, ui_font, editor_font, editor, layout }
    }

    /// Solid color for a key (first stop for gradients).
    pub fn c(&self, key: &str) -> Hsla {
        self.colors.get(key).copied().unwrap_or(gpui::transparent_black())
    }

    pub fn has(&self, key: &str) -> bool {
        self.colors.contains_key(key)
    }

    /// Background (solid or gradient) for a key.
    pub fn bg(&self, key: &str) -> Background {
        self.backgrounds.get(key).copied().unwrap_or_else(|| gpui::transparent_black().into())
    }

    /// Number from metrics (or fonts), as pixels.
    pub fn m(&self, key: &str, default: f32) -> Pixels {
        px(self.n(key, default))
    }

    pub fn n(&self, key: &str, default: f32) -> f32 {
        let d = &self.data;
        d.metrics.get(key).or_else(|| d.fonts.get(key)).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
    }

    /// Thickness ("all", "h,v" or "l,t,r,b").
    pub fn edges(&self, key: &str, default: f32) -> Edges<Pixels> {
        let raw = self.data.metrics.get(key).cloned().unwrap_or_else(|| default.to_string());
        let v: Vec<f32> = raw.split([',', ' ']).filter_map(|s| s.trim().parse().ok()).collect();
        let (l, t, r, b) = match v.as_slice() {
            [a] => (*a, *a, *a, *a),
            [h, vv] => (*h, *vv, *h, *vv),
            [l, t, r, b, ..] => (*l, *t, *r, *b),
            _ => (default, default, default, default),
        };
        Edges { top: px(t), right: px(r), bottom: px(b), left: px(l) }
    }

    /// Corner radius (first value of the key).
    pub fn radius(&self, key: &str, default: f32) -> Pixels {
        px(self
            .data
            .metrics
            .get(key)
            .and_then(|v| v.split([',', ' ']).find_map(|s| s.trim().parse().ok()))
            .unwrap_or(default))
    }

    pub fn weight(&self, key: &str, default: FontWeight) -> FontWeight {
        self.data.fonts.get(key).or_else(|| self.data.metrics.get(key)).and_then(|v| parse_weight(v)).unwrap_or(default)
    }

    pub fn italic(&self, key: &str) -> bool {
        self.data.fonts.get(key).is_some_and(|v| v.eq_ignore_ascii_case("italic"))
    }

    /// Box shadows from the "effects" section: "x y blur spread color, …" or "none".
    pub fn shadow(&self, key: &str) -> Vec<BoxShadow> {
        let Some(raw) = self.data.effects.get(key) else { return Vec::new() };
        raw.split(',')
            .filter_map(|part| {
                let tokens: Vec<&str> = part.split_whitespace().collect();
                let color = tokens.iter().find_map(|t| parse_color(t))?;
                let nums: Vec<f32> = tokens.iter().filter_map(|t| t.trim_end_matches("px").parse().ok()).collect();
                Some(BoxShadow {
                    color,
                    offset: point(px(*nums.first().unwrap_or(&0.0)), px(*nums.get(1).unwrap_or(&0.0))),
                    blur_radius: px(*nums.get(2).unwrap_or(&0.0)),
                    spread_radius: px(*nums.get(3).unwrap_or(&0.0)),
                })
            })
            .collect()
    }

    pub fn layout(&self, key: &str) -> Option<&str> {
        self.data.layout.get(key).map(String::as_str)
    }

    pub fn effect(&self, key: &str) -> Option<&str> {
        self.data.effects.get(key).map(String::as_str)
    }

    /// Token style for a classification ("a - b" falls back to "a").
    pub fn token(&self, kind: &str) -> Option<&TokenStyle> {
        let mut key = kind;
        loop {
            if let Some(style) = self.tokens.get(key) {
                return Some(style);
            }
            key = &key[..key.rfind(" - ")?];
        }
    }

    pub fn bracket_colors(&self) -> Vec<Hsla> {
        (1..=6).filter_map(|i| self.colors.get(&format!("editorBracketHighlight.foreground{i}")).copied()).collect()
    }
}

fn parse_weight(value: &str) -> Option<FontWeight> {
    if let Ok(n) = value.trim().parse::<f32>() {
        return Some(FontWeight(n));
    }
    Some(match value.trim().to_lowercase().replace(['-', ' '], "").as_str() {
        "thin" => FontWeight::THIN,
        "extralight" | "ultralight" => FontWeight::EXTRA_LIGHT,
        "light" => FontWeight::LIGHT,
        "normal" | "regular" => FontWeight::NORMAL,
        "medium" => FontWeight::MEDIUM,
        "semibold" | "demibold" => FontWeight::SEMIBOLD,
        "bold" => FontWeight::BOLD,
        "extrabold" | "ultrabold" => FontWeight::EXTRA_BOLD,
        "black" | "heavy" => FontWeight::BLACK,
        _ => return None,
    })
}

/// CSS-style colors: #RGB #RGBA #RRGGBB #RRGGBBAA, `transparent`, and a few names.
pub fn parse_color(value: &str) -> Option<Hsla> {
    let v = value.trim();
    if v.eq_ignore_ascii_case("transparent") {
        return Some(gpui::transparent_black());
    }
    let named = match v.to_ascii_lowercase().as_str() {
        "white" => Some("#ffffff"),
        "black" => Some("#000000"),
        "red" => Some("#ff0000"),
        "green" => Some("#00ff00"),
        "blue" => Some("#0000ff"),
        "gray" | "grey" => Some("#808080"),
        _ => None,
    };
    let hex = named.unwrap_or(v).strip_prefix('#')?;
    let expanded: String = if hex.len() <= 4 { hex.chars().flat_map(|c| [c, c]).collect() } else { hex.to_owned() };
    let n = u32::from_str_radix(&expanded, 16).ok()?;
    let (r, g, b, a) = match expanded.len() {
        6 => (n >> 16, n >> 8, n, 0xff),
        8 => (n >> 24, n >> 16, n >> 8, n),
        _ => return None,
    };
    let f = |x: u32| (x & 0xff) as f32 / 255.0;
    Some(Rgba { r: f(r), g: f(g), b: f(b), a: f(a) }.into())
}

/// Solid color or `linear-gradient(<angle>deg, c1, …, cN)` / `radial-gradient(…)`
/// (GPUI gradients have two stops: the first and last stop are used).
pub fn parse_background(value: &str) -> Option<(Background, Hsla)> {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    if !(lower.starts_with("linear-gradient(") || lower.starts_with("radial-gradient(")) {
        let c = parse_color(v)?;
        return Some((c.into(), c));
    }
    let inner = &v[v.find('(')? + 1..v.rfind(')')?];
    let mut parts: Vec<&str> = inner.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    let mut angle = 180.0;
    if let Some(first) = parts.first()
        && let Some(deg) = first.strip_suffix("deg")
        && let Ok(a) = deg.trim().parse::<f32>()
    {
        angle = a;
        parts.remove(0);
    }
    let stops: Vec<(Hsla, Option<f32>)> = parts
        .iter()
        .filter_map(|p| {
            let mut it = p.split_whitespace();
            let color = parse_color(it.next()?)?;
            let pct = it.next().and_then(|s| s.strip_suffix('%')).and_then(|s| s.parse::<f32>().ok()).map(|x| x / 100.0);
            Some((color, pct))
        })
        .collect();
    let first = *stops.first()?;
    let last = *stops.last()?;
    let bg = linear_gradient(angle, linear_color_stop(first.0, first.1.unwrap_or(0.0)), linear_color_stop(last.0, last.1.unwrap_or(1.0)));
    Some((bg, first.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_css_colors() {
        let c = parse_color("#ff000080").unwrap();
        assert!((c.a - 0.5).abs() < 0.01);
        assert!(parse_color("#abc").is_some());
        assert!(parse_color("nope").is_none());
        assert!(parse_background("linear-gradient(90deg, #ff2a6d, #b967ff 55%, #05d9e8)").is_some());
    }
}
