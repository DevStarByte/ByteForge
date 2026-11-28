//! The values of AXAML attributes: colors and brushes, thicknesses, lengths, grid definitions.

/// A value written as a markup extension (`{Binding Name}`, `{DynamicResource Accent}`); `{}` escapes one.
pub fn is_markup(v: &str) -> bool {
    let v = v.trim_start();
    v.starts_with('{') && !v.starts_with("{}")
}

/// The text a value shows: an escaped `{}` dropped, a binding shown as written.
pub fn display_text(v: &str) -> String {
    v.strip_prefix("{}").unwrap_or(v).to_owned()
}

pub fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub fn parse_num(v: &str) -> Option<f32> {
    let v = v.trim();
    if is_markup(v) {
        return None;
    }
    v.parse::<f32>().ok().filter(|n| n.is_finite())
}

/// A length: a number, `Auto` (or `NaN`) is none.
pub fn parse_len(v: &str) -> Option<f32> {
    parse_num(v)
}

/// A thickness (`4`, `4,2` or `left,top,right,bottom`, commas or spaces) as [left, top, right, bottom].
pub fn parse_thickness(v: &str) -> Option<[f32; 4]> {
    if is_markup(v) {
        return None;
    }
    let parts: Vec<f32> = v.split([',', ' ']).filter(|p| !p.trim().is_empty()).map(|p| p.trim().parse::<f32>().ok()).collect::<Option<_>>()?;
    match parts.as_slice() {
        [a] => Some([*a; 4]),
        [h, v] => Some([*h, *v, *h, *v]),
        [l, t, r, b] => Some([*l, *t, *r, *b]),
        _ => None,
    }
}

/// Formats a number as AXAML writes it: `12`, `12.5`.
pub fn format_num(n: f32) -> String {
    let r = (n * 100.0).round() / 100.0;
    if r.fract() == 0.0 { format!("{}", r as i64) } else { format!("{r}") }
}

/// A row or column of a Grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridLen {
    Auto,
    Star(f32),
    Px(f32),
}

/// `Auto,*,2*,100` (the short form of RowDefinitions / ColumnDefinitions, or one definition's Width/Height).
pub fn parse_grid_lengths(v: &str) -> Vec<GridLen> {
    v.split([',', ' ']).filter(|p| !p.trim().is_empty()).map(parse_grid_len).collect()
}

pub fn parse_grid_len(p: &str) -> GridLen {
    let p = p.trim();
    if p.eq_ignore_ascii_case("auto") {
        GridLen::Auto
    } else if let Some(n) = p.strip_suffix('*') {
        GridLen::Star(if n.trim().is_empty() { 1.0 } else { n.trim().parse().unwrap_or(1.0) })
    } else {
        p.parse().map(GridLen::Px).unwrap_or(GridLen::Star(1.0))
    }
}

/// A color or solid brush as RGBA: `#RGB`, `#ARGB`, `#RRGGBB`, `#AARRGGBB` or a known color's name.
pub fn parse_color(v: &str) -> Option<u32> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix('#') {
        let n = u32::from_str_radix(hex, 16).ok()?;
        let nib = |i: u32| ((n >> (i * 4)) & 0xf) * 0x11;
        return match hex.len() {
            3 => Some((nib(2) << 24) | (nib(1) << 16) | (nib(0) << 8) | 0xff),
            4 => Some((nib(2) << 24) | (nib(1) << 16) | (nib(0) << 8) | nib(3)),
            6 => Some((n << 8) | 0xff),
            8 => Some(n.rotate_left(8)),
            _ => None,
        };
    }
    let lower = v.to_ascii_lowercase();
    KNOWN_COLORS.iter().find(|(name, _)| name.eq_ignore_ascii_case(&lower)).map(|(_, c)| *c)
}

/// Colors offered by the property grid (Avalonia knows all of CSS's; these are the common ones).
pub const BRUSHES: &[&str] = &[
    "Transparent", "White", "Black", "Gray", "LightGray", "DarkGray", "Red", "DarkRed", "Orange", "Gold", "Yellow", "Green", "LimeGreen", "Teal", "CornflowerBlue",
    "SteelBlue", "Blue", "Navy", "Purple", "HotPink", "Brown", "Beige",
];

const KNOWN_COLORS: &[(&str, u32)] = &[
    ("transparent", 0x00ffffff),
    ("white", 0xffffffff),
    ("black", 0x000000ff),
    ("gray", 0x808080ff),
    ("grey", 0x808080ff),
    ("lightgray", 0xd3d3d3ff),
    ("lightgrey", 0xd3d3d3ff),
    ("darkgray", 0xa9a9a9ff),
    ("dimgray", 0x696969ff),
    ("silver", 0xc0c0c0ff),
    ("gainsboro", 0xdcdcdcff),
    ("whitesmoke", 0xf5f5f5ff),
    ("red", 0xff0000ff),
    ("darkred", 0x8b0000ff),
    ("crimson", 0xdc143cff),
    ("firebrick", 0xb22222ff),
    ("tomato", 0xff6347ff),
    ("coral", 0xff7f50ff),
    ("salmon", 0xfa8072ff),
    ("orange", 0xffa500ff),
    ("darkorange", 0xff8c00ff),
    ("orangered", 0xff4500ff),
    ("gold", 0xffd700ff),
    ("yellow", 0xffff00ff),
    ("lightyellow", 0xffffe0ff),
    ("khaki", 0xf0e68cff),
    ("green", 0x008000ff),
    ("darkgreen", 0x006400ff),
    ("lime", 0x00ff00ff),
    ("limegreen", 0x32cd32ff),
    ("lightgreen", 0x90ee90ff),
    ("seagreen", 0x2e8b57ff),
    ("olive", 0x808000ff),
    ("teal", 0x008080ff),
    ("cyan", 0x00ffffff),
    ("aqua", 0x00ffffff),
    ("turquoise", 0x40e0d0ff),
    ("lightblue", 0xadd8e6ff),
    ("skyblue", 0x87ceebff),
    ("deepskyblue", 0x00bfffff),
    ("dodgerblue", 0x1e90ffff),
    ("cornflowerblue", 0x6495edff),
    ("steelblue", 0x4682b4ff),
    ("royalblue", 0x4169e1ff),
    ("blue", 0x0000ffff),
    ("mediumblue", 0x0000cdff),
    ("darkblue", 0x00008bff),
    ("navy", 0x000080ff),
    ("indigo", 0x4b0082ff),
    ("purple", 0x800080ff),
    ("violet", 0xee82eeff),
    ("magenta", 0xff00ffff),
    ("fuchsia", 0xff00ffff),
    ("orchid", 0xda70d6ff),
    ("plum", 0xdda0ddff),
    ("pink", 0xffc0cbff),
    ("hotpink", 0xff69b4ff),
    ("deeppink", 0xff1493ff),
    ("brown", 0xa52a2aff),
    ("chocolate", 0xd2691eff),
    ("sienna", 0xa0522dff),
    ("tan", 0xd2b48cff),
    ("beige", 0xf5f5dcff),
    ("ivory", 0xfffff0ff),
    ("linen", 0xfaf0e6ff),
    ("snow", 0xfffafaff),
    ("aliceblue", 0xf0f8ffff),
    ("ghostwhite", 0xf8f8ffff),
    ("mintcream", 0xf5fffaff),
    ("lavender", 0xe6e6faff),
    ("slategray", 0x708090ff),
    ("darkslategray", 0x2f4f4fff),
    ("lightslategray", 0x778899ff),
    ("maroon", 0x800000ff),
];

/// Escapes a value for an attribute in double quotes.
pub fn escape_attr(v: &str) -> String {
    v.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

/// Escapes text content.
pub fn escape_text(v: &str) -> String {
    v.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
