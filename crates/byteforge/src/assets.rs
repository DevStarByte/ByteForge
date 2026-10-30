//! Icons as SVG assets generated from path data. Themes can replace any icon (`"icons": { "files": "M…" }`);
//! asset paths carry the theme version so GPUI's image cache never serves a stale icon.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{AssetSource, SharedString};
use parking_lot::RwLock;

pub const ICONS: &[(&str, &str)] = &[
    ("file", "M13,9V3.5L18.5,9M6,2C4.89,2 4,2.89 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2H6Z"),
    ("files", "M15,7H20.5L15,1.5V7M8,0H16L22,6V18A2,2 0 0,1 20,20H8C6.89,20 6,19.1 6,18V2A2,2 0 0,1 8,0M4,4V22H20V24H4A2,2 0 0,1 2,22V4H4Z"),
    ("folder", "M10,4H4C2.89,4 2,4.89 2,6V18A2,2 0 0,0 4,20H20A2,2 0 0,0 22,18V8C22,6.89 21.1,6 20,6H12L10,4Z"),
    ("folderOpen", "M19,20H4C2.89,20 2,19.1 2,18V6C2,4.89 2.89,4 4,4H10L12,6H19A2,2 0 0,1 21,8H21L4,8V18L6.14,10H23.21L20.93,18.5C20.7,19.37 19.92,20 19,20Z"),
    ("search", "M9.5,3A6.5,6.5 0 0,1 16,9.5C16,11.11 15.41,12.59 14.44,13.73L14.71,14H15.5L20.5,19L19,20.5L14,15.5V14.71L13.73,14.44C12.59,15.41 11.11,16 9.5,16A6.5,6.5 0 0,1 3,9.5A6.5,6.5 0 0,1 9.5,3M9.5,5C7,5 5,7 5,9.5C5,12 7,14 9.5,14C12,14 14,12 14,9.5C14,7 12,5 9.5,5Z"),
    ("settings", "M3,17V19H9V17H3M3,5V7H13V5H3M13,21V19H21V17H13V15H11V21H13M7,9V11H3V13H7V15H9V9H7M21,13V11H11V13H21M15,9H17V7H21V5H17V3H15V9Z"),
    ("close", "M19,6.41L17.59,5L12,10.59L6.41,5L5,6.41L10.59,12L5,17.59L6.41,19L12,13.41L17.59,19L19,17.59L13.41,12L19,6.41Z"),
    ("error", "M12,2C17.53,2 22,6.47 22,12C22,17.53 17.53,22 12,22C6.47,22 2,17.53 2,12C2,6.47 6.47,2 12,2M15.59,7L12,10.59L8.41,7L7,8.41L10.59,12L7,15.59L8.41,17L12,13.41L15.59,17L17,15.59L13.41,12L17,8.41L15.59,7Z"),
    ("warning", "M13,14H11V10H13M13,18H11V16H13M1,21H23L12,2L1,21Z"),
    ("info", "M13,9H11V7H13M13,17H11V11H13M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2Z"),
    ("chevronUp", "M7.41,15.41L12,10.83L16.59,15.41L18,14L12,8L6,14L7.41,15.41Z"),
    ("chevronDown", "M7.41,8.58L12,13.17L16.59,8.58L18,10L12,16L6,10L7.41,8.58Z"),
    ("chevronRight", "M8.6,16.6L13.2,12L8.6,7.4L10,6L16,12L10,18L8.6,16.6Z"),
    ("play", "M8,5.14V19.14L19,12.14L8,5.14Z"),
    ("stop", "M18,18H6V6H18V18Z"),
    ("build", "M22.7,19L13.6,9.9C14.5,7.6 14,4.9 12.1,3C10.1,1 7.1,0.6 4.7,1.7L9,6L6,9L1.6,4.7C0.4,7.1 0.9,10.1 2.9,12.1C4.8,14 7.5,14.5 9.8,13.6L18.9,22.7C19.3,23.1 19.9,23.1 20.3,22.7L22.6,20.4C23.1,20 23.1,19.3 22.7,19Z"),
    ("logo", "M12,1.5L21.5,7V17L12,22.5L2.5,17V7L12,1.5M8.5,7V17H13C15.2,17 16.5,15.9 16.5,14.2C16.5,13 15.8,12.1 14.7,11.8C15.5,11.4 16,10.6 16,9.6C16,8.1 14.8,7 12.8,7H8.5M10.5,8.7H12.6C13.5,8.7 14,9.2 14,9.9C14,10.6 13.5,11.1 12.6,11.1H10.5V8.7M10.5,12.7H12.8C13.8,12.7 14.4,13.2 14.4,14C14.4,14.8 13.8,15.3 12.8,15.3H10.5V12.7Z"),
    ("windowMinimize", "M5,11.5H19V12.5H5Z"),
    ("windowMaximize", "M5,5H19V19H5V5M6,6V18H18V6H6Z"),
    ("windowRestore", "M5,8H16V19H5V8M6,9V18H15V9H6M8,5H19V16H17V15H18V6H9V7H8V5Z"),
    ("windowClose", "M6.7,6L12,11.3L17.3,6L18,6.7L12.7,12L18,17.3L17.3,18L12,12.7L6.7,18L6,17.3L11.3,12L6,6.7Z"),
    ("layoutSideBar", "M3,4H21V20H3V4M4,5V19H9V5H4M10,5V19H20V5H10Z"),
    ("layoutSideBarRight", "M3,4H21V20H3V4M15,5V19H20V5H15M4,5V19H14V5H4Z"),
    ("layoutPanel", "M3,4H21V20H3V4M4,5V14H20V5H4M4,15V19H20V15H4Z"),
    ("palette", "M17.5,12A1.5,1.5 0 0,1 16,10.5A1.5,1.5 0 0,1 17.5,9A1.5,1.5 0 0,1 19,10.5A1.5,1.5 0 0,1 17.5,12M14.5,8A1.5,1.5 0 0,1 13,6.5A1.5,1.5 0 0,1 14.5,5A1.5,1.5 0 0,1 16,6.5A1.5,1.5 0 0,1 14.5,8M9.5,8A1.5,1.5 0 0,1 8,6.5A1.5,1.5 0 0,1 9.5,5A1.5,1.5 0 0,1 11,6.5A1.5,1.5 0 0,1 9.5,8M6.5,12A1.5,1.5 0 0,1 5,10.5A1.5,1.5 0 0,1 6.5,9A1.5,1.5 0 0,1 8,10.5A1.5,1.5 0 0,1 6.5,12M12,3A9,9 0 0,0 3,12A9,9 0 0,0 12,21A1.5,1.5 0 0,0 13.5,19.5C13.5,19.11 13.35,18.76 13.11,18.5C12.88,18.23 12.73,17.88 12.73,17.5A1.5,1.5 0 0,1 14.23,16H16A5,5 0 0,0 21,11C21,6.58 16.97,3 12,3Z"),
    ("sourceControl", "M6,2A3,3 0 0,1 9,5C9,6.28 8.19,7.38 7.06,7.81C7.15,8.27 7.39,8.83 8,9.63C9,10.92 11,12.83 12,14.17C13,12.83 15,10.92 16,9.63C16.61,8.83 16.85,8.27 16.94,7.81C15.81,7.38 15,6.28 15,5A3,3 0 0,1 18,2A3,3 0 0,1 21,5C21,6.32 20.14,7.45 18.95,7.85C18.87,8.37 18.64,9 18,9.83C17,11.17 15,13.08 14,14.38C13.39,15.17 13.15,15.73 13.06,16.19C14.19,16.62 15,17.72 15,19A3,3 0 0,1 12,22A3,3 0 0,1 9,19C9,17.72 9.81,16.62 10.94,16.19C10.85,15.73 10.61,15.17 10,14.38C9,13.08 7,11.17 6,9.83C5.36,9 5.13,8.37 5.05,7.85C3.86,7.45 3,6.32 3,5A3,3 0 0,1 6,2M6,4A1,1 0 0,0 5,5A1,1 0 0,0 6,6A1,1 0 0,0 7,5A1,1 0 0,0 6,4M18,4A1,1 0 0,0 17,5A1,1 0 0,0 18,6A1,1 0 0,0 19,5A1,1 0 0,0 18,4M12,18A1,1 0 0,0 11,19A1,1 0 0,0 12,20A1,1 0 0,0 13,19A1,1 0 0,0 12,18Z"),
    ("add", "M19,13H13V19H11V13H5V11H11V5H13V11H19V13Z"),
    ("remove", "M19,13H5V11H19V13Z"),
    ("discard", "M12.5,8C9.85,8 7.45,9 5.6,10.6L2,7V16H11L7.38,12.38C8.77,11.22 10.54,10.5 12.5,10.5C16.04,10.5 19.05,12.81 20.1,16L22.47,15.22C21.08,11.03 17.15,8 12.5,8Z"),
    ("refresh", "M17.65,6.35C16.2,4.9 14.21,4 12,4A8,8 0 0,0 4,12A8,8 0 0,0 12,20C15.73,20 18.84,17.45 19.73,14H17.65C16.83,16.33 14.61,18 12,18A6,6 0 0,1 6,12A6,6 0 0,1 12,6C13.66,6 15.14,6.69 16.22,7.78L13,11H20V4L17.65,6.35Z"),
    ("check", "M21,7L9,19L3.5,13.5L4.91,12.09L9,16.17L19.59,5.59L21,7Z"),
    ("arrowUp", "M13,20H11V8L5.5,13.5L4.08,12.08L12,4.16L19.92,12.08L18.5,13.5L13,8V20Z"),
    ("arrowDown", "M11,4H13V16L18.5,10.5L19.92,11.92L12,19.84L4.08,11.92L5.5,10.5L11,16V4Z"),
    ("sync", "M12,18A6,6 0 0,1 6,12C6,11 6.25,10.03 6.7,9.2L5.24,7.74C4.46,8.97 4,10.43 4,12A8,8 0 0,0 12,20V23L16,19L12,15M12,4V1L8,5L12,9V6A6,6 0 0,1 18,12C18,13 17.75,13.97 17.3,14.8L18.76,16.26C19.54,15.03 20,13.57 20,12A8,8 0 0,0 12,4Z"),
    ("diff", "M19,3H14V5H19V18L14,12V21H19A2,2 0 0,0 21,19V5C21,3.89 20.1,3 19,3M10,18H5L10,12M10,3H5C3.89,3 3,3.89 3,5V19A2,2 0 0,0 5,21H10V23H12V1H10V3Z"),
    ("puzzle", "M20.5,11H19V7C19,5.89 18.1,5 17,5H13V3.5A2.5,2.5 0 0,0 10.5,1A2.5,2.5 0 0,0 8,3.5V5H4A2,2 0 0,0 2,7V10.8H3.5C5,10.8 6.2,12 6.2,13.5C6.2,15 5,16.2 3.5,16.2H2V20A2,2 0 0,0 4,22H7.8V20.5C7.8,19 9,17.8 10.5,17.8C12,17.8 13.2,19 13.2,20.5V22H17A2,2 0 0,0 19,20V16H20.5A2.5,2.5 0 0,0 23,13.5A2.5,2.5 0 0,0 20.5,11Z"),
];

static OVERRIDES: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);
static VERSION: AtomicUsize = AtomicUsize::new(0);

/// Installs theme icon overrides and bumps the icon version.
pub fn set_icon_overrides(overrides: HashMap<String, String>) {
    *OVERRIDES.write() = Some(overrides);
    VERSION.fetch_add(1, Ordering::SeqCst);
}

/// Asset path for an icon name (or `None` for unknown names, which callers can treat as raw path data).
pub fn icon_path(name: &str) -> SharedString {
    format!("icons/{}/{}.svg", VERSION.load(Ordering::SeqCst), name).into()
}

fn icon_data(name: &str) -> Option<String> {
    if let Some(custom) = OVERRIDES.read().as_ref().and_then(|o| o.get(name)) {
        return Some(custom.clone());
    }
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, d)| (*d).to_owned()).or_else(|| {
        // Unknown names that look like path data are rendered directly (plugins may send raw paths).
        name.starts_with(['M', 'm']).then(|| name.to_owned())
    })
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let Some(rest) = path.strip_prefix("icons/") else { return Ok(None) };
        let name = rest.split_once('/').map_or(rest, |(_, n)| n).trim_end_matches(".svg");
        let Some(data) = icon_data(name) else { return Ok(None) };
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill-rule="evenodd" fill="black" d="{data}"/></svg>"#
        );
        Ok(Some(Cow::Owned(svg.into_bytes())))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(ICONS.iter().map(|(n, _)| SharedString::from(format!("icons/{n}.svg"))).collect())
    }
}

/// An icon element tinted with the given color.
pub fn icon(name: &str, size: gpui::Pixels, color: gpui::Hsla) -> gpui::Svg {
    use gpui::Styled;
    gpui::svg().path(icon_path(name)).size(size).flex_none().text_color(color)
}
