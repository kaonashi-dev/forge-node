use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// A `file://` URI for an absolute path, percent-encoding every byte outside
/// RFC 3986's unreserved set and `/`.
#[must_use]
pub fn path_to_uri(path: &Path) -> String {
    let bytes = path.as_os_str().as_bytes();
    let mut out = String::with_capacity(bytes.len() + 8);
    out.push_str("file://");
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}

/// The path a `file:` URI names, or `None` for another scheme, a remote
/// authority or a malformed escape.
#[must_use]
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let scheme = uri.get(..5)?;
    if !scheme.eq_ignore_ascii_case("file:") {
        return None;
    }
    let rest = &uri[5..];
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let path = if let Some(after) = rest.strip_prefix("//") {
        let slash = after.find('/')?;
        let authority = &after[..slash];
        if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
            return None;
        }
        &after[slash..]
    } else if rest.starts_with('/') {
        rest
    } else {
        return None;
    };
    Some(PathBuf::from(OsString::from_vec(percent_decode(path)?)))
}

fn percent_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Some(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_and_non_ascii_bytes_are_escaped() {
        assert_eq!(
            path_to_uri(Path::new("/a b/#x%?/é.rs")),
            "file:///a%20b/%23x%25%3F/%C3%A9.rs"
        );
        assert_eq!(
            path_to_uri(Path::new("/src/main_1-2.~rs")),
            "file:///src/main_1-2.~rs"
        );
    }

    #[test]
    fn paths_round_trip() {
        let odd = PathBuf::from(OsString::from_vec(b"/tmp/\xff\x01raw".to_vec()));
        for path in [
            PathBuf::from("/"),
            PathBuf::from("/home/dev/a b/c#d/e%f/g?h/ü.rs"),
            PathBuf::from("/x/[y]/{z}/@+=,;:!$&'()*"),
            odd,
        ] {
            let uri = path_to_uri(&path);
            assert_eq!(uri_to_path(&uri).as_deref(), Some(path.as_path()), "{uri}");
        }
    }

    #[test]
    fn decoding_accepts_the_common_spellings_of_a_local_file() {
        let expected = Some(PathBuf::from("/a b/c.rs"));
        assert_eq!(uri_to_path("file:///a%20b/c.rs"), expected);
        assert_eq!(uri_to_path("FILE:///a%20b/c.rs"), expected);
        assert_eq!(uri_to_path("file://localhost/a%20b/c.rs"), expected);
        assert_eq!(uri_to_path("file:/a%20b/c.rs"), expected);
        assert_eq!(uri_to_path("file:///a%20b/c.rs#L3"), expected);
        assert_eq!(uri_to_path("file:///a%2fb"), Some(PathBuf::from("/a/b")));
    }

    #[test]
    fn decoding_refuses_what_is_not_a_local_file() {
        for uri in [
            "untitled:Untitled-1",
            "jar:file:///x.jar!/A.class",
            "https://example.com/a.rs",
            "file://server/share/a.rs",
            "file:relative/a.rs",
            "file:///bad%2",
            "file:///bad%zz",
            "file",
        ] {
            assert_eq!(uri_to_path(uri), None, "{uri}");
        }
    }
}
