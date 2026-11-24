//! Property values as designer code writes them: reading `new Point(12, 12)`, `Color.FromArgb(…)`,
//! `AnchorStyles.Top | AnchorStyles.Left` and so on, and writing them back in the file's style.

use super::Style;
use super::lex::{call_args, identifiers, matching_paren, split_args};

/// A font as `new Font("Segoe UI", 12F, FontStyle.Bold, …)` sets it.
#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    pub family: String,
    /// In points.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
}

/// The namespace a type used in designer code lives in.
fn namespace(type_name: &str) -> &'static str {
    match type_name {
        "Point" | "Size" | "SizeF" | "Color" | "SystemColors" | "Font" | "FontStyle" | "GraphicsUnit" | "ContentAlignment" => "System.Drawing",
        _ => "System.Windows.Forms",
    }
}

/// `type_name`, qualified when the file qualifies its types.
pub fn type_ref(style: Style, type_name: &str) -> String {
    if style.qualified { format!("{}.{type_name}", namespace(type_name)) } else { type_name.to_owned() }
}

/// Strips casts, parentheses and number suffixes: `((int)(((byte)(192))))` → `192`, `9F` → `9`.
fn unwrap(expr: &str) -> &str {
    let mut s = expr.trim();
    loop {
        if s.starts_with('(')
            && let Some(close) = matching_paren(s, 0)
        {
            if close == s.len() - 1 {
                s = s[1..close].trim();
                continue;
            }
            // A cast: (type)rest
            let inner = &s[1..close];
            if !inner.is_empty() && inner.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_') {
                s = s[close + 1..].trim();
                continue;
            }
        }
        return s;
    }
}

pub fn parse_string(expr: &str) -> Option<String> {
    let s = expr.trim();
    if let Some(body) = s.strip_prefix("@\"").and_then(|r| r.strip_suffix('"')) {
        return Some(body.replace("\"\"", "\""));
    }
    let body = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '0' => out.push('\0'),
            'u' => {
                let hex: String = chars.by_ref().take(4).collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            other => out.push(other),
        }
    }
    Some(out)
}

pub fn format_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn parse_char(expr: &str) -> Option<char> {
    let body = expr.trim().strip_prefix('\'')?.strip_suffix('\'')?;
    match body {
        "\\'" => Some('\''),
        "\\\\" => Some('\\'),
        "\\0" => Some('\0'),
        _ => {
            let mut chars = body.chars();
            let c = chars.next()?;
            chars.next().is_none().then_some(c)
        }
    }
}

pub fn format_char(c: char) -> String {
    match c {
        '\'' => "'\\''".into(),
        '\\' => "'\\\\'".into(),
        '\0' => "'\\0'".into(),
        c => format!("'{c}'"),
    }
}

pub fn parse_bool(expr: &str) -> Option<bool> {
    match unwrap(expr) {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub fn parse_number(expr: &str) -> Option<f64> {
    let s = unwrap(expr);
    let s = s.trim_end_matches(['F', 'f', 'D', 'd', 'M', 'm', 'L', 'l', 'U', 'u']);
    s.parse().ok()
}

pub fn parse_int(expr: &str) -> Option<i64> {
    parse_number(expr).filter(|n| n.fract() == 0.0).map(|n| n as i64)
}

/// `new Point(12, 34)` (also Size, SizeF, qualified or not) → its two numbers.
pub fn parse_pair(expr: &str) -> Option<(f64, f64)> {
    let s = unwrap(expr);
    let rest = s.strip_prefix("new ")?;
    let args = call_args(rest)?;
    match split_args(args).as_slice() {
        [a, b] => Some((parse_number(a)?, parse_number(b)?)),
        _ => None,
    }
}

pub fn format_pair(style: Style, type_name: &str, a: i64, b: i64) -> String {
    format!("new {}({a}, {b})", type_ref(style, type_name))
}

/// The member an enum value names: `System.Windows.Forms.DockStyle.Fill` → `Fill`.
pub fn parse_enum(expr: &str) -> Option<String> {
    let s = unwrap(expr);
    if s.is_empty() || !s.split('.').all(super::lex::is_identifier) {
        return None;
    }
    s.rsplit('.').next().map(str::to_owned)
}

pub fn format_enum(style: Style, enum_type: &str, member: &str) -> String {
    format!("{}.{member}", type_ref(style, enum_type))
}

/// The members of a flags value: `AnchorStyles.Top | AnchorStyles.Left` (casts and parentheses allowed).
pub fn parse_flags(expr: &str, enum_type: &str) -> Vec<String> {
    let mut out = Vec::new();
    let ids = identifiers(expr);
    for (i, (r, _)) in ids.iter().enumerate() {
        if &expr[r.clone()] == enum_type
            && let Some((next, _)) = ids.get(i + 1)
            && expr[r.end..next.start].trim() == "."
        {
            out.push(expr[next.clone()].to_owned());
        }
    }
    out
}

pub fn format_flags(style: Style, enum_type: &str, members: &[&str]) -> String {
    let parts: Vec<String> = members.iter().map(|m| format_enum(style, enum_type, m)).collect();
    if style.qualified && parts.len() > 1 {
        format!("(({})(({})))", type_ref(style, enum_type), parts.join(" | "))
    } else {
        parts.join(" | ")
    }
}

/// A color as 0xRRGGBBAA: named colors, system colors and `Color.FromArgb(…)`.
pub fn parse_color(expr: &str) -> Option<u32> {
    let s = unwrap(expr);
    if let Some(rest) = s.strip_prefix("System.Drawing.").or(Some(s))
        && let Some(args) = rest.strip_prefix("Color.FromArgb").and_then(call_args)
    {
        let nums: Vec<u32> = split_args(args).iter().map(|a| parse_int(a).map(|n| n as u32)).collect::<Option<_>>()?;
        return match nums.as_slice() {
            [r, g, b] => Some((r << 24) | (g << 16) | (b << 8) | 0xff),
            [a, r, g, b] => Some((r << 24) | (g << 16) | (b << 8) | a),
            // A packed ARGB int.
            [argb] => Some(argb.rotate_left(8)),
            _ => None,
        };
    }
    let name = parse_enum(s)?;
    let owner = s.rsplit('.').nth(1).unwrap_or("");
    if owner == "SystemColors" { system_color(&name) } else { named_color(&name) }
}

/// The name a color has (`Red`, `SystemColors.Control`), if it has one.
pub fn color_name(expr: &str) -> Option<String> {
    let s = unwrap(expr);
    if s.contains("FromArgb") {
        return None;
    }
    let name = parse_enum(s)?;
    let owner = s.rsplit('.').nth(1).unwrap_or("");
    Some(if owner == "SystemColors" { format!("SystemColors.{name}") } else { name })
}

/// Writes a color typed as a name (`Red`, `SystemColors.Control`), `#RRGGBB` or `r, g, b`.
pub fn format_color(style: Style, input: &str) -> Option<String> {
    let s = input.trim();
    if let Some(name) = s.strip_prefix("SystemColors.") {
        let name = SYSTEM_COLORS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))?.0;
        return Some(format!("{}.{name}", type_ref(style, "SystemColors")));
    }
    if let Some(&(name, _)) = NAMED_COLORS.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)) {
        return Some(format!("{}.{name}", type_ref(style, "Color")));
    }
    let rgb: Vec<u32> = if let Some(hex) = s.strip_prefix('#') {
        let v = u32::from_str_radix(hex, 16).ok()?;
        match hex.len() {
            6 => vec![v >> 16, (v >> 8) & 0xff, v & 0xff],
            8 => vec![v >> 24, (v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff],
            _ => return None,
        }
    } else {
        s.split(',').map(|p| p.trim().parse::<u32>().ok().filter(|n| *n < 256)).collect::<Option<_>>()?
    };
    if !matches!(rgb.len(), 3 | 4) {
        return None;
    }
    let args: Vec<String> = rgb.iter().map(u32::to_string).collect();
    Some(format!("{}.FromArgb({})", type_ref(style, "Color"), args.join(", ")))
}

/// How a color value reads in the property grid.
pub fn describe_color(expr: &str) -> String {
    if let Some(name) = color_name(expr) {
        return name;
    }
    match parse_color(expr) {
        Some(c) if c & 0xff == 0xff => format!("{}, {}, {}", c >> 24, (c >> 16) & 0xff, (c >> 8) & 0xff),
        Some(c) => format!("{}, {}, {}, {}", c & 0xff, c >> 24, (c >> 16) & 0xff, (c >> 8) & 0xff),
        None => expr.trim().to_owned(),
    }
}

pub fn parse_font(expr: &str) -> Option<FontSpec> {
    let s = unwrap(expr).strip_prefix("new ")?;
    let args = split_args(call_args(s)?);
    let family = parse_string(args.first()?)?;
    let size = parse_number(args.get(1)?)? as f32;
    let styles = args.get(2).map(|a| parse_flags(a, "FontStyle")).unwrap_or_default();
    let has = |n: &str| styles.iter().any(|s| s == n);
    Some(FontSpec { family, size, bold: has("Bold"), italic: has("Italic"), underline: has("Underline"), strikeout: has("Strikeout") })
}

/// `Segoe UI, 12pt, Bold` (the property grid's way of showing a font).
pub fn describe_font(font: &FontSpec) -> String {
    let mut out = format!("{}, {}pt", font.family, trim_float(font.size));
    for (on, name) in [(font.bold, "Bold"), (font.italic, "Italic"), (font.underline, "Underline"), (font.strikeout, "Strikeout")] {
        if on {
            out.push_str(", ");
            out.push_str(name);
        }
    }
    out
}

/// Reads `Segoe UI, 12pt, Bold` (size and styles optional; `pt` optional).
pub fn read_font(input: &str) -> Option<FontSpec> {
    let mut parts = input.split(',').map(str::trim).filter(|p| !p.is_empty());
    let family = parts.next()?.to_owned();
    let mut font = FontSpec { family, size: 9.0, bold: false, italic: false, underline: false, strikeout: false };
    for p in parts {
        if let Ok(n) = p.trim_end_matches("pt").trim().parse::<f32>() {
            font.size = n;
            continue;
        }
        match p.to_ascii_lowercase().as_str() {
            "bold" => font.bold = true,
            "italic" => font.italic = true,
            "underline" => font.underline = true,
            "strikeout" => font.strikeout = true,
            "regular" => {}
            _ => return None,
        }
    }
    (font.size > 0.0).then_some(font)
}

pub fn format_font(style: Style, font: &FontSpec) -> String {
    let styles: Vec<&str> = [(font.bold, "Bold"), (font.italic, "Italic"), (font.underline, "Underline"), (font.strikeout, "Strikeout")]
        .into_iter()
        .filter_map(|(on, n)| on.then_some(n))
        .collect();
    let styles = if styles.is_empty() { vec!["Regular"] } else { styles };
    let flags: Vec<String> = styles.iter().map(|s| format_enum(style, "FontStyle", s)).collect();
    let flags = if style.qualified && flags.len() > 1 { format!("(({})(({})))", type_ref(style, "FontStyle"), flags.join(" | ")) } else { flags.join(" | ") };
    let charset = if style.qualified { "((byte)(0))" } else { "0" };
    format!(
        "new {}({}, {}F, {flags}, {}, {charset})",
        type_ref(style, "Font"),
        format_string(&font.family),
        trim_float(font.size),
        format_enum(style, "GraphicsUnit", "Point")
    )
}

fn trim_float(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// `new decimal(new int[] { 100, 0, 0, 0 })` or a literal like `5M`.
pub fn parse_decimal(expr: &str) -> Option<f64> {
    let s = unwrap(expr);
    if let Some(rest) = s.strip_prefix("new decimal").or_else(|| s.strip_prefix("new System.Decimal")) {
        let inner = call_args(rest)?;
        let open = inner.find('{')?;
        let close = inner.rfind('}')?;
        let parts: Vec<i64> = split_args(&inner[open + 1..close]).iter().map(|p| parse_int(p)).collect::<Option<_>>()?;
        let [lo, mid, hi, flags] = parts.as_slice() else { return None };
        let raw = (*lo as u32 as f64) + (*mid as u32 as f64) * 4294967296.0 + (*hi as u32 as f64) * 18446744073709551616.0;
        let flags = *flags as u32;
        let scale = (flags >> 16) & 0xff;
        let value = raw / 10f64.powi(scale as i32);
        return Some(if flags & 0x8000_0000 != 0 { -value } else { value });
    }
    parse_number(s)
}

pub fn format_decimal(value: f64) -> String {
    let negative = value < 0.0;
    let text = format!("{}", value.abs());
    let scale = text.split('.').nth(1).map_or(0, str::len) as u32;
    let raw: u64 = text.replace('.', "").parse().unwrap_or(0);
    let flags = (scale << 16) | if negative { 0x8000_0000 } else { 0 };
    format!("new decimal(new int[] {{ {}, {}, 0, {} }})", raw as u32 as i32, (raw >> 32) as u32 as i32, flags as i32)
}

pub fn describe_decimal(expr: &str) -> String {
    parse_decimal(expr).map(|v| format!("{v}")).unwrap_or_else(|| expr.trim().to_owned())
}

pub fn named_color(name: &str) -> Option<u32> {
    NAMED_COLORS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

pub fn system_color(name: &str) -> Option<u32> {
    SYSTEM_COLORS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// Windows' system colors (as Windows 10 and 11 have them by default).
pub const SYSTEM_COLORS: &[(&str, u32)] = &[
    ("ActiveBorder", 0xb4b4b4ff),
    ("ActiveCaption", 0x99b4d1ff),
    ("ActiveCaptionText", 0x000000ff),
    ("AppWorkspace", 0xabababff),
    ("ButtonFace", 0xf0f0f0ff),
    ("ButtonHighlight", 0xffffffff),
    ("ButtonShadow", 0xa0a0a0ff),
    ("Control", 0xf0f0f0ff),
    ("ControlDark", 0xa0a0a0ff),
    ("ControlDarkDark", 0x696969ff),
    ("ControlLight", 0xe3e3e3ff),
    ("ControlLightLight", 0xffffffff),
    ("ControlText", 0x000000ff),
    ("Desktop", 0x000000ff),
    ("GradientActiveCaption", 0xb9d1eaff),
    ("GradientInactiveCaption", 0xd7e4f2ff),
    ("GrayText", 0x6d6d6dff),
    ("Highlight", 0x0078d7ff),
    ("HighlightText", 0xffffffff),
    ("HotTrack", 0x0066ccff),
    ("InactiveBorder", 0xf4f7fcff),
    ("InactiveCaption", 0xbfcddbff),
    ("InactiveCaptionText", 0x000000ff),
    ("Info", 0xffffe1ff),
    ("InfoText", 0x000000ff),
    ("Menu", 0xf0f0f0ff),
    ("MenuBar", 0xf0f0f0ff),
    ("MenuHighlight", 0x3399ffff),
    ("MenuText", 0x000000ff),
    ("ScrollBar", 0xc8c8c8ff),
    ("Window", 0xffffffff),
    ("WindowFrame", 0x646464ff),
    ("WindowText", 0x000000ff),
];

/// The web colors System.Drawing.Color names.
pub const NAMED_COLORS: &[(&str, u32)] = &[
    ("Transparent", 0xffffff00),
    ("AliceBlue", 0xf0f8ffff),
    ("AntiqueWhite", 0xfaebd7ff),
    ("Aqua", 0x00ffffff),
    ("Aquamarine", 0x7fffd4ff),
    ("Azure", 0xf0ffffff),
    ("Beige", 0xf5f5dcff),
    ("Bisque", 0xffe4c4ff),
    ("Black", 0x000000ff),
    ("BlanchedAlmond", 0xffebcdff),
    ("Blue", 0x0000ffff),
    ("BlueViolet", 0x8a2be2ff),
    ("Brown", 0xa52a2aff),
    ("BurlyWood", 0xdeb887ff),
    ("CadetBlue", 0x5f9ea0ff),
    ("Chartreuse", 0x7fff00ff),
    ("Chocolate", 0xd2691eff),
    ("Coral", 0xff7f50ff),
    ("CornflowerBlue", 0x6495edff),
    ("Cornsilk", 0xfff8dcff),
    ("Crimson", 0xdc143cff),
    ("Cyan", 0x00ffffff),
    ("DarkBlue", 0x00008bff),
    ("DarkCyan", 0x008b8bff),
    ("DarkGoldenrod", 0xb8860bff),
    ("DarkGray", 0xa9a9a9ff),
    ("DarkGreen", 0x006400ff),
    ("DarkKhaki", 0xbdb76bff),
    ("DarkMagenta", 0x8b008bff),
    ("DarkOliveGreen", 0x556b2fff),
    ("DarkOrange", 0xff8c00ff),
    ("DarkOrchid", 0x9932ccff),
    ("DarkRed", 0x8b0000ff),
    ("DarkSalmon", 0xe9967aff),
    ("DarkSeaGreen", 0x8fbc8fff),
    ("DarkSlateBlue", 0x483d8bff),
    ("DarkSlateGray", 0x2f4f4fff),
    ("DarkTurquoise", 0x00ced1ff),
    ("DarkViolet", 0x9400d3ff),
    ("DeepPink", 0xff1493ff),
    ("DeepSkyBlue", 0x00bfffff),
    ("DimGray", 0x696969ff),
    ("DodgerBlue", 0x1e90ffff),
    ("Firebrick", 0xb22222ff),
    ("FloralWhite", 0xfffaf0ff),
    ("ForestGreen", 0x228b22ff),
    ("Fuchsia", 0xff00ffff),
    ("Gainsboro", 0xdcdcdcff),
    ("GhostWhite", 0xf8f8ffff),
    ("Gold", 0xffd700ff),
    ("Goldenrod", 0xdaa520ff),
    ("Gray", 0x808080ff),
    ("Green", 0x008000ff),
    ("GreenYellow", 0xadff2fff),
    ("Honeydew", 0xf0fff0ff),
    ("HotPink", 0xff69b4ff),
    ("IndianRed", 0xcd5c5cff),
    ("Indigo", 0x4b0082ff),
    ("Ivory", 0xfffff0ff),
    ("Khaki", 0xf0e68cff),
    ("Lavender", 0xe6e6faff),
    ("LavenderBlush", 0xfff0f5ff),
    ("LawnGreen", 0x7cfc00ff),
    ("LemonChiffon", 0xfffacdff),
    ("LightBlue", 0xadd8e6ff),
    ("LightCoral", 0xf08080ff),
    ("LightCyan", 0xe0ffffff),
    ("LightGoldenrodYellow", 0xfafad2ff),
    ("LightGray", 0xd3d3d3ff),
    ("LightGreen", 0x90ee90ff),
    ("LightPink", 0xffb6c1ff),
    ("LightSalmon", 0xffa07aff),
    ("LightSeaGreen", 0x20b2aaff),
    ("LightSkyBlue", 0x87cefaff),
    ("LightSlateGray", 0x778899ff),
    ("LightSteelBlue", 0xb0c4deff),
    ("LightYellow", 0xffffe0ff),
    ("Lime", 0x00ff00ff),
    ("LimeGreen", 0x32cd32ff),
    ("Linen", 0xfaf0e6ff),
    ("Magenta", 0xff00ffff),
    ("Maroon", 0x800000ff),
    ("MediumAquamarine", 0x66cdaaff),
    ("MediumBlue", 0x0000cdff),
    ("MediumOrchid", 0xba55d3ff),
    ("MediumPurple", 0x9370dbff),
    ("MediumSeaGreen", 0x3cb371ff),
    ("MediumSlateBlue", 0x7b68eeff),
    ("MediumSpringGreen", 0x00fa9aff),
    ("MediumTurquoise", 0x48d1ccff),
    ("MediumVioletRed", 0xc71585ff),
    ("MidnightBlue", 0x191970ff),
    ("MintCream", 0xf5fffaff),
    ("MistyRose", 0xffe4e1ff),
    ("Moccasin", 0xffe4b5ff),
    ("NavajoWhite", 0xffdeadff),
    ("Navy", 0x000080ff),
    ("OldLace", 0xfdf5e6ff),
    ("Olive", 0x808000ff),
    ("OliveDrab", 0x6b8e23ff),
    ("Orange", 0xffa500ff),
    ("OrangeRed", 0xff4500ff),
    ("Orchid", 0xda70d6ff),
    ("PaleGoldenrod", 0xeee8aaff),
    ("PaleGreen", 0x98fb98ff),
    ("PaleTurquoise", 0xafeeeeff),
    ("PaleVioletRed", 0xdb7093ff),
    ("PapayaWhip", 0xffefd5ff),
    ("PeachPuff", 0xffdab9ff),
    ("Peru", 0xcd853fff),
    ("Pink", 0xffc0cbff),
    ("Plum", 0xdda0ddff),
    ("PowderBlue", 0xb0e0e6ff),
    ("Purple", 0x800080ff),
    ("Red", 0xff0000ff),
    ("RosyBrown", 0xbc8f8fff),
    ("RoyalBlue", 0x4169e1ff),
    ("SaddleBrown", 0x8b4513ff),
    ("Salmon", 0xfa8072ff),
    ("SandyBrown", 0xf4a460ff),
    ("SeaGreen", 0x2e8b57ff),
    ("SeaShell", 0xfff5eeff),
    ("Sienna", 0xa0522dff),
    ("Silver", 0xc0c0c0ff),
    ("SkyBlue", 0x87ceebff),
    ("SlateBlue", 0x6a5acdff),
    ("SlateGray", 0x708090ff),
    ("Snow", 0xfffafaff),
    ("SpringGreen", 0x00ff7fff),
    ("SteelBlue", 0x4682b4ff),
    ("Tan", 0xd2b48cff),
    ("Teal", 0x008080ff),
    ("Thistle", 0xd8bfd8ff),
    ("Tomato", 0xff6347ff),
    ("Turquoise", 0x40e0d0ff),
    ("Violet", 0xee82eeff),
    ("Wheat", 0xf5deb3ff),
    ("White", 0xffffffff),
    ("WhiteSmoke", 0xf5f5f5ff),
    ("Yellow", 0xffff00ff),
    ("YellowGreen", 0x9acd32ff),
];

#[cfg(test)]
mod tests {
    use super::*;

    const NEW: Style = Style { this: false, qualified: false };
    const OLD: Style = Style { this: true, qualified: true };

    #[test]
    fn reads_values_in_both_styles() {
        assert_eq!(parse_pair("new System.Drawing.Point(12, 34)"), Some((12.0, 34.0)));
        assert_eq!(parse_pair("new SizeF(7F, 15F)"), Some((7.0, 15.0)));
        assert_eq!(parse_string(r#""a \"b\"\n""#).as_deref(), Some("a \"b\"\n"));
        assert_eq!(parse_enum("System.Windows.Forms.DockStyle.Fill").as_deref(), Some("Fill"));
        assert_eq!(
            parse_flags("((System.Windows.Forms.AnchorStyles)((System.Windows.Forms.AnchorStyles.Top | System.Windows.Forms.AnchorStyles.Right)))", "AnchorStyles"),
            ["Top", "Right"]
        );
        assert_eq!(parse_color("System.Drawing.Color.FromArgb(((int)(((byte)(255)))), ((int)(((byte)(128)))), ((int)(((byte)(0)))))"), Some(0xff8000ff));
        assert_eq!(parse_color("SystemColors.Control"), Some(0xf0f0f0ff));
        assert_eq!(parse_color("Color.Red"), Some(0xff0000ff));
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_decimal("new decimal(new int[] { 1000, 0, 0, 0 })"), Some(1000.0));
        assert_eq!(parse_decimal("new decimal(new int[] {\n            25,\n            0,\n            0,\n            65536})"), Some(2.5));
        let font = parse_font("new Font(\"Segoe UI\", 12F, FontStyle.Bold | FontStyle.Italic, GraphicsUnit.Point, 0)").unwrap();
        assert_eq!((font.family.as_str(), font.size, font.bold, font.italic), ("Segoe UI", 12.0, true, true));
    }

    #[test]
    fn writes_values_in_the_files_style() {
        assert_eq!(format_pair(NEW, "Point", 1, 2), "new Point(1, 2)");
        assert_eq!(format_pair(OLD, "Size", 1, 2), "new System.Drawing.Size(1, 2)");
        assert_eq!(format_flags(NEW, "AnchorStyles", &["Top", "Right"]), "AnchorStyles.Top | AnchorStyles.Right");
        assert_eq!(format_color(NEW, "red").as_deref(), Some("Color.Red"));
        assert_eq!(format_color(OLD, "#ff8000").as_deref(), Some("System.Drawing.Color.FromArgb(255, 128, 0)"));
        assert_eq!(format_color(NEW, "SystemColors.control").as_deref(), Some("SystemColors.Control"));
        assert_eq!(format_decimal(2.5), "new decimal(new int[] { 25, 0, 0, 65536 })");
        assert_eq!(parse_decimal(&format_decimal(-7.0)), Some(-7.0));
        let font = read_font("Segoe UI, 12pt, Bold").unwrap();
        assert_eq!(format_font(NEW, &font), "new Font(\"Segoe UI\", 12F, FontStyle.Bold, GraphicsUnit.Point, 0)");
        assert_eq!(describe_font(&font), "Segoe UI, 12pt, Bold");
        assert_eq!(format_string("a\"b\\"), r#""a\"b\\""#);
    }
}
