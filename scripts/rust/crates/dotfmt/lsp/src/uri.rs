//! File URI conversion without pulling in a URL crate.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

/// A filesystem-independent map key for a URI.
///
/// File URIs use their normalized path identity, so every spelling of the same
/// file (`%28`/`(`, `..`, symlinks) shares one entry. Non-file URIs fall back
/// to their raw spelling.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    Path(PathBuf),
    Uri(String),
}

impl Key {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path),
            Self::Uri(_) => None,
        }
    }

    /// The canonical URI for this key. Open documents are resolved to their
    /// client spelling by the diagnostics store before publication.
    pub fn uri(&self) -> Uri {
        match self {
            Self::Path(path) => from_path(path),
            Self::Uri(uri) => Uri::from_str(uri).expect("stored URI is valid"),
        }
    }
}

/// Whether a URI addresses a file on disk.
pub fn is_file(uri: &Uri) -> bool {
    uri.scheme().is_some_and(|scheme| scheme.as_str() == "file")
}

/// The filesystem path behind a `file:` URI.
pub fn path(uri: &Uri) -> Option<PathBuf> {
    if !is_file(uri) {
        return None;
    }
    let decoded = decode(uri.path().as_str())?;
    #[cfg(windows)]
    let decoded = {
        let bytes = decoded.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
            decoded[1..].to_owned()
        } else {
            decoded
        }
    };
    Some(PathBuf::from(decoded))
}

/// The normalized path identity behind a file URI.
pub fn identity(uri: &Uri) -> Option<PathBuf> {
    path(uri).map(|path| crate::workspace::normalize(&path))
}

/// The identity key used by tsc groups, diagnostics and configuration errors.
pub fn key(uri: &Uri) -> Key {
    match identity(uri) {
        Some(path) => Key::Path(path),
        None => Key::Uri(uri.as_str().to_owned()),
    }
}

/// A `file:` URI for an absolute path, encoded like Node's `pathToFileURL`.
///
/// The safe set is `encodeURI` minus `~`, `[` and `]`, and the URL parser's
/// path escapes (`#`, `?`, quotes, braces and controls) are applied too. This
/// leaves route-group parentheses and `@` literal, matching VS Code.
pub fn from_path(path: &Path) -> Uri {
    #[cfg(windows)]
    let text = path.to_string_lossy().replace('\\', "/");
    #[cfg(not(windows))]
    let text = path.to_string_lossy();
    let mut encoded = String::with_capacity(text.len() + 8);
    encoded.push_str("file://");
    #[cfg(windows)]
    if !text.starts_with('/') {
        encoded.push('/');
    }
    for byte in text.bytes() {
        if safe(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex(byte >> 4));
            encoded.push(hex(byte & 0xf));
        }
    }
    Uri::from_str(&encoded).expect("encoded file URI is valid")
}

fn safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b'-'
                | b'.'
                | b'/'
                | b':'
                | b';'
                | b'='
                | b'@'
                | b'_'
        )
}

fn hex(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'A' + nibble - 10) as char,
    }
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position] == b'%' {
            let high = *bytes.get(position + 1)?;
            let low = *bytes.get(position + 2)?;
            decoded.push((nibble(high)? << 4) | nibble(low)?);
            position += 3;
        } else {
            decoded.push(bytes[position]);
            position += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn nibble(byte: u8) -> Option<u8> {
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
    fn round_trips_paths_with_spaces_and_unicode() {
        let file = Path::new("/tmp/a folder/ünïcode#1.json");
        let uri = from_path(file);
        assert_eq!(path(&uri).as_deref(), Some(file));
    }

    #[test]
    fn rejects_non_file_uris() {
        assert!(path(&Uri::from_str("untitled:Untitled-1").unwrap()).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn encodes_like_path_to_file_url() {
        for (path, expected) in [
            (
                "/tmp/app/(dashboard)/page.tsx",
                "file:///tmp/app/(dashboard)/page.tsx",
            ),
            (
                "/tmp/app/(.)photo/page.tsx",
                "file:///tmp/app/(.)photo/page.tsx",
            ),
            (
                "/tmp/app/@modal/page.tsx",
                "file:///tmp/app/@modal/page.tsx",
            ),
            ("/tmp/[slug]/page.tsx", "file:///tmp/%5Bslug%5D/page.tsx"),
            (
                "/tmp/~ ! $ & ' * + , ; = # %",
                "file:///tmp/%7E%20!%20$%20&%20'%20*%20+%20,%20;%20=%20%23%20%25",
            ),
            ("/tmp/a folder/page.tsx", "file:///tmp/a%20folder/page.tsx"),
            (
                "/tmp/üñïçødé/page.tsx",
                "file:///tmp/%C3%BC%C3%B1%C3%AF%C3%A7%C3%B8d%C3%A9/page.tsx",
            ),
        ] {
            assert_eq!(from_path(Path::new(path)).as_str(), expected, "{path}");
        }
    }

    #[test]
    fn identity_ignores_spelling_differences() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let directory = root.join("app").join("(dashboard)");
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("page.tsx");
        std::fs::write(&file, "").unwrap();

        let literal = Uri::from_str(&format!("file://{}", file.display())).unwrap();
        let encoded = Uri::from_str(&format!(
            "file://{}/app/%28dashboard%29/page.tsx",
            root.display()
        ))
        .unwrap();
        let dotted = Uri::from_str(&format!(
            "file://{}/app/./other/../%28dashboard%29/page.tsx",
            root.display()
        ))
        .unwrap();
        assert_eq!(identity(&literal), identity(&encoded));
        assert_eq!(identity(&literal), identity(&dotted));
        assert_eq!(key(&literal), key(&encoded));
        assert_eq!(key(&literal), key(&dotted));
        assert_eq!(path(&encoded).as_deref(), Some(file.as_path()));
    }

    #[test]
    fn non_file_keys_keep_their_spelling() {
        let uri = Uri::from_str("untitled:Untitled-1").unwrap();
        assert_eq!(key(&uri), Key::Uri("untitled:Untitled-1".into()));
        assert_eq!(key(&uri).uri(), uri);
    }
}
