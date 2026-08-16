use crate::{ByteRange, Diagnostic, LineIndex, RepoPath, Severity, SourceText, Stage};

/// The one path carrying the source version declaration.
pub const PROFILES_PATH: &str = "config/profiles.dotfile";

/// The only supported authored source version.
pub const SOURCE_VERSION_1: &str = "1";

const DECLARATION: &[u8] = b"@dotfile-version";
const BOM: &[u8] = b"\xef\xbb\xbf";

/// An authored source version selected by the bootstrap reader. Version
/// identifiers are opaque strings, not quantities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceVersion {
    V1,
}

/// A successful bootstrap: the selected version, the exact declaration
/// range, and whether a leading BOM was present.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bootstrap {
    pub version: SourceVersion,
    pub declaration: ByteRange,
    pub bom: bool,
}

/// Reads the source version declaration from raw bytes.
///
/// The bootstrap reader recognizes one optional UTF-8 BOM at byte offset
/// zero followed by the exact ASCII declaration as the first non-comment
/// entry of `config/profiles.dotfile`. Unsupported, absent, duplicate,
/// interpolated, bound, or otherwise malformed declarations fail without
/// invoking any legacy parser (ADR 0006). A declaration in any other file is
/// an error.
///
/// Returns `Ok(Some(_))` for a valid profiles file, `Ok(None)` for another
/// file without a declaration, and `Err(_)` with every bootstrap diagnostic
/// otherwise.
pub fn read_bootstrap(
    path: &RepoPath,
    text: &SourceText,
) -> Result<Option<Bootstrap>, Vec<Diagnostic>> {
    let bytes = text.as_bytes();
    let index = LineIndex::new(bytes);
    let mut errors = Vec::new();
    let mut position = if bytes.starts_with(BOM) { 3 } else { 0 };
    let bom = position == 3;

    // One pass over physical lines records the first non-comment entry and
    // every line whose first token spells `@dotfile-version`.
    let mut first_entry: Option<(usize, usize)> = None;
    let mut candidates: Vec<(usize, usize)> = Vec::new();
    while position < bytes.len() {
        let line_end = bytes[position..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|relative| position + relative)
            .unwrap_or(bytes.len());
        let mut content_end = line_end;
        if content_end > position && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let mut first = position;
        while first < content_end && matches!(bytes[first], b' ' | b'\t') {
            first += 1;
        }
        let blank = first == content_end;
        let comment = !blank && bytes[first] == b'#';
        if !blank && !comment {
            let entry = (first, content_end);
            if first_entry.is_none() {
                first_entry = Some(entry);
            }
            if bytes[first..content_end].starts_with(DECLARATION)
                && declaration_boundary(bytes, first + DECLARATION.len(), content_end)
            {
                candidates.push(entry);
            }
        }
        position = line_end + 1;
    }

    let make_span = |range: ByteRange| crate::Span::new(path, range, text, &index);
    let context = |summary: &str, remedy: &str, span: crate::Span| {
        Diagnostic::new(
            "schema/context",
            Stage::Schema,
            Severity::Error,
            summary,
            remedy,
            span,
        )
    };

    if path.as_str() != PROFILES_PATH {
        for (start, end) in candidates {
            let range = ByteRange::new(start as u64, end as u64, text.len()).unwrap();
            errors.push(context(
                "the source version declaration is only valid in config/profiles.dotfile",
                "remove the @dotfile-version entry from this file",
                make_span(range),
            ));
        }
        return if errors.is_empty() {
            Ok(None)
        } else {
            Err(errors)
        };
    }

    let Some((entry_start, entry_end)) = first_entry else {
        let range = ByteRange::at(0, text.len()).unwrap();
        errors.push(context(
            "config/profiles.dotfile has no source version declaration",
            "add @dotfile-version = \"1\" as the first non-comment entry",
            make_span(range),
        ));
        return Err(errors);
    };

    let Some(&(first_start, first_end)) = candidates.first() else {
        let range = ByteRange::new(entry_start as u64, entry_end as u64, text.len()).unwrap();
        errors.push(context(
            "the first non-comment entry is not the source version declaration",
            "add @dotfile-version = \"1\" as the first non-comment entry",
            make_span(range),
        ));
        return Err(errors);
    };

    let mut result = None;
    if (first_start, first_end) != (entry_start, entry_end) {
        let range = ByteRange::new(first_start as u64, first_end as u64, text.len()).unwrap();
        errors.push(context(
            "the source version declaration must be the first non-comment entry",
            "move @dotfile-version = \"1\" above every other entry",
            make_span(range),
        ));
    } else {
        match validate_declaration(bytes, first_start, first_end) {
            Declaration::Valid { value_end } => {
                result = Some(Bootstrap {
                    version: SourceVersion::V1,
                    declaration: ByteRange::new(first_start as u64, value_end as u64, text.len())
                        .unwrap(),
                    bom,
                });
            }
            Declaration::Unsupported {
                value_start,
                value_end,
            } => {
                let span = make_span(
                    ByteRange::new(first_start as u64, first_end as u64, text.len()).unwrap(),
                );
                let found = String::from_utf8_lossy(&bytes[value_start..value_end]).into_owned();
                errors.push(
                    context(
                        "the authored source version is unsupported",
                        "declare @dotfile-version = \"1\"",
                        span,
                    )
                    .with_detail("unsupported_dotfile_version")
                    .with_expected("version", SOURCE_VERSION_1)
                    .with_actual("version", found),
                );
            }
            Declaration::Malformed => {
                let span = make_span(
                    ByteRange::new(first_start as u64, first_end as u64, text.len()).unwrap(),
                );
                errors.push(context(
                    "the source version declaration is malformed",
                    "write exactly @dotfile-version = \"1\" with no binding, interpolation, soft break, or trailing comma",
                    span,
                ));
            }
        }
    }

    for &(start, end) in &candidates[1..] {
        let range = ByteRange::new(start as u64, end as u64, text.len()).unwrap();
        errors.push(Diagnostic::new(
            "schema/duplicate",
            Stage::Schema,
            Severity::Error,
            "duplicate source version declaration",
            "keep exactly one @dotfile-version entry",
            make_span(range),
        ));
    }

    if errors.is_empty() {
        Ok(result)
    } else {
        Err(errors)
    }
}

/// A candidate declaration is `@dotfile-version` followed by a token
/// boundary: horizontal whitespace, `=`, or end of line.
fn declaration_boundary(bytes: &[u8], position: usize, content_end: usize) -> bool {
    if position >= content_end {
        return true;
    }
    matches!(bytes[position], b' ' | b'\t' | b'=')
}

enum Declaration {
    Valid {
        value_end: usize,
    },
    Unsupported {
        value_start: usize,
        value_end: usize,
    },
    Malformed,
}

/// Validates the exact ASCII preamble on one entry line: `@dotfile-version`,
/// optional horizontal whitespace, `=`, optional horizontal whitespace, and
/// exactly `"1"`, followed by end of line or horizontal whitespace and an
/// optional trailing comment.
fn validate_declaration(bytes: &[u8], start: usize, end: usize) -> Declaration {
    let mut position = start + DECLARATION.len();
    let skip_whitespace = |bytes: &[u8], mut position: usize, end: usize| {
        while position < end && matches!(bytes[position], b' ' | b'\t') {
            position += 1;
        }
        position
    };
    position = skip_whitespace(bytes, position, end);
    if position >= end || bytes[position] != b'=' {
        return Declaration::Malformed;
    }
    position = skip_whitespace(bytes, position + 1, end);
    if position >= end || bytes[position] != b'"' {
        return Declaration::Malformed;
    }
    let value_start = position + 1;
    // Scan the quoted value on this one line; the declaration cannot use
    // escapes or interpolation, so a raw closing quote ends it.
    let mut cursor = value_start;
    let value_end = loop {
        if cursor >= end {
            return Declaration::Malformed;
        }
        match bytes[cursor] {
            b'"' => break cursor,
            b'\\' => return Declaration::Malformed,
            _ => cursor += 1,
        }
    };
    let after = value_end + 1;
    if &bytes[value_start..value_end] != b"1" {
        return Declaration::Unsupported {
            value_start,
            value_end,
        };
    }
    let mut rest = skip_whitespace(bytes, after, end);
    if rest < end && bytes[rest] == b'#' {
        rest = end;
    }
    if rest != end {
        return Declaration::Malformed;
    }
    Declaration::Valid { value_end: after }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles(bytes: &str) -> Result<Option<Bootstrap>, Vec<Diagnostic>> {
        let path = RepoPath::new(PROFILES_PATH).unwrap();
        read_bootstrap(&path, &SourceText::from(bytes))
    }

    fn other(path: &str, bytes: &str) -> Result<Option<Bootstrap>, Vec<Diagnostic>> {
        let path = RepoPath::new(path).unwrap();
        read_bootstrap(&path, &SourceText::from(bytes))
    }

    #[test]
    fn accepts_the_exact_preamble() {
        let bootstrap = profiles("@dotfile-version = \"1\"\n").unwrap().unwrap();
        assert_eq!(bootstrap.version, SourceVersion::V1);
        assert_eq!(bootstrap.declaration, ByteRange::new(0, 22, 23).unwrap());
        assert!(!bootstrap.bom);
    }

    #[test]
    fn accepts_bom_comments_whitespace_and_trailing_comment() {
        let input = "\u{feff}# header\n\n  # another\n@dotfile-version = \"1\" # pinned\n";
        let bootstrap = profiles(input).unwrap().unwrap();
        assert!(bootstrap.bom);
        let declaration = &input.as_bytes()
            [bootstrap.declaration.start() as usize..bootstrap.declaration.end() as usize];
        assert_eq!(declaration, b"@dotfile-version = \"1\"");
    }

    #[test]
    fn accepts_crlf_and_compact_spacing() {
        assert!(profiles("@dotfile-version = \"1\"\r\n").is_ok());
        assert!(profiles("@dotfile-version=\"1\"\n").is_ok());
        assert!(profiles("@dotfile-version  =\t\"1\"").is_ok());
    }

    #[test]
    fn rejects_an_unsupported_version() {
        let errors = profiles("@dotfile-version = \"2\"\n").unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "schema/context");
        assert_eq!(
            errors[0].detail.as_deref(),
            Some("unsupported_dotfile_version")
        );
        assert_eq!(errors[0].actual["version"], "2");
    }

    #[test]
    fn rejects_malformed_declarations() {
        for input in [
            "@dotfile-version = $version\n",
            "@dotfile-version =\n\"1\"\n",
            "@dotfile-version = \"1\",\n",
            "@dotfile-version = \"1\" extra\n",
            "@dotfile-version\n",
            "@dotfile-version = \"\\u{31}\"\n",
        ] {
            let errors = profiles(input).unwrap_err();
            assert_eq!(errors[0].code, "schema/context", "{input:?}");
            assert_eq!(errors[0].detail, None, "{input:?}");
        }
    }

    #[test]
    fn rejects_an_interpolated_declaration_as_unsupported() {
        let errors = profiles("@dotfile-version = \"${version}\"\n").unwrap_err();
        assert_eq!(
            errors[0].detail.as_deref(),
            Some("unsupported_dotfile_version")
        );
        assert_eq!(errors[0].actual["version"], "${version}");
    }

    #[test]
    fn rejects_absent_and_misplaced_declarations() {
        let errors = profiles("").unwrap_err();
        assert_eq!(
            errors[0].summary,
            "config/profiles.dotfile has no source version declaration"
        );

        let errors = profiles("# only comments\n").unwrap_err();
        assert_eq!(
            errors[0].summary,
            "config/profiles.dotfile has no source version declaration"
        );

        let errors = profiles("@groups {}\n").unwrap_err();
        assert_eq!(
            errors[0].summary,
            "the first non-comment entry is not the source version declaration"
        );

        let errors = profiles("@groups {}\n@dotfile-version = \"1\"\n").unwrap_err();
        assert_eq!(
            errors[0].summary,
            "the source version declaration must be the first non-comment entry"
        );

        let errors = profiles("foo\n@dotfile-version = \"1\"\n").unwrap_err();
        assert_eq!(
            errors[0].summary,
            "the source version declaration must be the first non-comment entry"
        );
    }

    #[test]
    fn rejects_duplicate_declarations() {
        let errors = profiles("@dotfile-version = \"1\"\n@dotfile-version = \"1\"\n").unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "schema/duplicate");
        assert_eq!(errors[0].primary_span.start_line, 2);
    }

    #[test]
    fn rejects_declarations_in_other_files() {
        let errors = other("shared/zsh/package.dotfile", "@dotfile-version = \"1\"\n").unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "schema/context");
        assert!(
            other("shared/zsh/package.dotfile", "zsh\n")
                .unwrap()
                .is_none()
        );
    }
}
