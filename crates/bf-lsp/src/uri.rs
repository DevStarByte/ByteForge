//! `file://` URI <-> filesystem path conversion (Windows drive letters, UNC, percent-encoding).

use std::path::{Path, PathBuf};

/// Converts a filesystem path into a `file://` URI, e.g. `C:\dev\x.cs` -> `file:///C:/dev/x.cs`.
pub fn path_to_uri(path: &Path) -> String {
    path_str_to_uri(&path.to_string_lossy(), cfg!(windows))
}

/// Converts a `file://` URI into a path. Accepts `file:///C:/x`, `file:///c%3A/x` and
/// `file://host/share/x`. Drive letters are normalized to upper case. `None` for other schemes.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    uri_to_path_str(uri, cfg!(windows)).map(PathBuf::from)
}

pub(crate) fn path_str_to_uri(path: &str, windows: bool) -> String {
    let mut out = String::from("file://");
    if !windows {
        if !path.starts_with('/') {
            out.push('/');
        }
        encode_into(path, &mut out);
        return out;
    }
    let mut p = path.replace('\\', "/");
    if let Some(rest) = p.strip_prefix("//?/UNC/") {
        p = format!("//{rest}");
    } else if let Some(rest) = p.strip_prefix("//?/") {
        p = rest.to_string();
    }
    if let Some(rest) = p.strip_prefix("//") {
        let (host, tail) = rest.split_once('/').unwrap_or((rest, ""));
        out.push_str(host);
        out.push('/');
        encode_into(tail, &mut out);
    } else if let Some((drive, tail)) = split_drive(&p) {
        out.push('/');
        out.push(drive.to_ascii_uppercase());
        out.push(':');
        if !tail.starts_with('/') {
            out.push('/');
        }
        encode_into(tail, &mut out);
    } else {
        if !p.starts_with('/') {
            out.push('/');
        }
        encode_into(&p, &mut out);
    }
    out
}

pub(crate) fn uri_to_path_str(uri: &str, windows: bool) -> Option<String> {
    let scheme_end = uri.find("://")?;
    if !uri[..scheme_end].eq_ignore_ascii_case("file") {
        return None;
    }
    let rest = &uri[scheme_end + 3..];
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let authority = percent_decode(authority);
    let path = percent_decode(path);
    let remote = !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost");

    if !windows {
        return Some(if remote {
            format!("//{authority}{path}")
        } else {
            path
        });
    }
    let drive = path.strip_prefix('/').and_then(split_drive);
    Some(if let Some((drive, tail)) = drive {
        let tail = tail.replace('/', "\\");
        let tail = if tail.is_empty() {
            "\\".to_string()
        } else {
            tail
        };
        format!("{}:{tail}", drive.to_ascii_uppercase())
    } else if remote {
        format!(r"\\{authority}{}", path.replace('/', "\\"))
    } else {
        path.replace('/', "\\")
    })
}

/// Splits `C:rest` into (`C`, `rest`).
fn split_drive(p: &str) -> Option<(char, &str)> {
    let mut chars = p.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    (chars.next() == Some(':')).then(|| (drive, &p[2..]))
}

fn encode_into(s: &str, out: &mut String) {
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_path_to_uri() {
        assert_eq!(path_str_to_uri(r"C:\dev\x.cs", true), "file:///C:/dev/x.cs");
        assert_eq!(path_str_to_uri(r"c:\dev\x.cs", true), "file:///C:/dev/x.cs");
        assert_eq!(
            path_str_to_uri(r"C:\My Projects\a#b\Grüße.cs", true),
            "file:///C:/My%20Projects/a%23b/Gr%C3%BC%C3%9Fe.cs"
        );
        assert_eq!(
            path_str_to_uri(r"\\?\C:\dev\x.cs", true),
            "file:///C:/dev/x.cs"
        );
        assert_eq!(
            path_str_to_uri(r"\\server\share\x.cs", true),
            "file://server/share/x.cs"
        );
        assert_eq!(path_str_to_uri("C:", true), "file:///C:/");
    }

    #[test]
    fn windows_uri_to_path() {
        let p = |u| uri_to_path_str(u, true).unwrap();
        assert_eq!(p("file:///C:/dev/x.cs"), r"C:\dev\x.cs");
        assert_eq!(p("file:///c%3A/dev/x.cs"), r"C:\dev\x.cs");
        assert_eq!(p("file:///c%3a/My%20Projects/x.cs"), r"C:\My Projects\x.cs");
        assert_eq!(p("FILE:///d:/a%23b/Gr%C3%BC%C3%9Fe.cs"), r"D:\a#b\Grüße.cs");
        assert_eq!(p("file://server/share/x.cs"), r"\\server\share\x.cs");
        assert_eq!(p("file://localhost/C:/x.cs"), r"C:\x.cs");
        assert_eq!(p("file:///C:"), r"C:\");
        assert_eq!(uri_to_path_str("https://example.com/x", true), None);
    }

    #[test]
    fn unix_round_trip() {
        let path = "/home/me/my proj/x+y%.cs";
        let uri = path_str_to_uri(path, false);
        assert_eq!(uri, "file:///home/me/my%20proj/x%2By%25.cs");
        assert_eq!(uri_to_path_str(&uri, false).unwrap(), path);
        // Backslashes are legal file name characters on Unix.
        assert_eq!(
            uri_to_path_str(&path_str_to_uri(r"/a\b", false), false).unwrap(),
            r"/a\b"
        );
    }

    #[test]
    fn windows_round_trip() {
        for path in [
            r"C:\dev\ByteForge\src\Program.cs",
            r"E:\a b\c~d\e.cs",
            r"\\srv\share\f.cs",
        ] {
            let uri = path_str_to_uri(path, true);
            assert_eq!(uri_to_path_str(&uri, true).unwrap(), path, "{uri}");
        }
    }

    #[test]
    fn native_round_trip() {
        let path = std::env::temp_dir().join("bf lsp").join("x.cs");
        assert_eq!(uri_to_path(&path_to_uri(&path)).unwrap(), path);
    }
}
