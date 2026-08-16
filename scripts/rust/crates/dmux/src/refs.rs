//! Reference grammar: structural parsing and formatting (plan §6.2, §6.3).
//!
//! Parsing here is purely structural — it classifies a token into a ref
//! shape without consulting the registry. Resolution (does this alias exist,
//! is this name ambiguous) happens above, against the authoritative registry
//! plus live inventory. The load-bearing grammar rules:
//!
//! - `SpaceNo` is canonical nonzero decimal `[1-9][0-9]*`; `0` and
//!   leading-zero forms are invalid refs and are NOT reinterpreted as names.
//! - Any `<host-token>:<digits>` is an owner-qualified numeric ref before any
//!   name lookup; a bad host token is an error, never a name fallback.
//! - The fixed lexical classes `^[A-Za-z]+[0-9]+$` and `^[0-9]+$` are
//!   ID-shaped even when the apparent alias is not enrolled; this
//!   classification never changes as hosts are added, and such tokens are
//!   never treated as names.
//! - New managed names use `[A-Za-z][A-Za-z0-9_-]{0,63}` and exclude the
//!   ID-shaped classes, URI prefixes, `:`/child-ref syntax, and `-`.
//!   External legacy names remain operable via stable ID or `--name`.

use std::num::NonZeroU64;

use uuid::Uuid;

use crate::model::{ChildKind, HostUid, ProviderHandle, ServerEpoch, SpaceNo, SpaceUid};

pub const URI_SCHEME: &str = "dmux://";
pub const HOST_LABEL_MAX: usize = 32;
pub const NEW_NAME_MAX: usize = 64;

/// A host position inside a ref, before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostToken {
    /// Full `HostUid` — the portable owner-qualified form.
    Uid(HostUid),
    /// Compact alias (`b`) or label (`archie`); which one is a resolution
    /// question, not a parsing question — the grammars overlap.
    AliasOrLabel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceRefShape {
    /// `dmux://<host-uid>/spaces/<space-uid>` — the strongest identity.
    Canonical { host: HostUid, space: SpaceUid },
    /// `2`, `b2`, `b:2`, `<host-uuid>:2` — owner-scoped permanent number.
    /// `host: None` means the local authority `a`.
    Numbered {
        host: Option<HostToken>,
        no: SpaceNo,
    },
    /// `project`, `b:project`, `archie:project` — exact logical name.
    /// `host: None` means the resolved/current host only; bare names are
    /// never searched across hosts.
    Named {
        host: Option<HostToken>,
        name: String,
    },
}

/// `g<epoch-uuid>.<handle>` / `p<epoch-uuid>.<handle>`, or the URI child
/// segments. Epoch-qualified and live: stale epochs fail, never retarget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildRefShape {
    pub kind: ChildKind,
    pub epoch: ServerEpoch,
    pub handle: ProviderHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRef {
    pub space: SpaceRefShape,
    pub child: Option<ChildRefShape>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefError {
    Empty,
    /// `0`, leading zeros, or otherwise non-canonical SpaceNo digits.
    InvalidSpaceNo,
    /// ID-shaped (`^[A-Za-z]+[0-9]+$`) but not a valid compact ref; never a name.
    IdShapedInvalid,
    MalformedUri,
    MalformedHostToken,
    MalformedChild,
    /// `-` and other tokens the CLI layer owns; not Space refs.
    ReservedToken,
}

pub fn parse_ref(input: &str) -> Result<ParsedRef, RefError> {
    if input.is_empty() {
        return Err(RefError::Empty);
    }
    if input == "-" {
        return Err(RefError::ReservedToken);
    }
    if let Some(rest) = input.strip_prefix(URI_SCHEME) {
        return parse_uri(rest);
    }
    let (space_part, child_part) = match input.split_once('/') {
        Some((s, c)) => (s, Some(c)),
        None => (input, None),
    };
    let child = child_part.map(parse_child_suffix).transpose()?;
    let space = parse_space_part(space_part)?;
    Ok(ParsedRef { space, child })
}

fn parse_space_part(part: &str) -> Result<SpaceRefShape, RefError> {
    if part.is_empty() {
        return Err(RefError::Empty);
    }
    if let Some((host, rest)) = part.split_once(':') {
        if rest.is_empty() || rest.contains(':') {
            return Err(RefError::MalformedHostToken);
        }
        // Numeric right side wins before any name interpretation.
        if rest.bytes().all(|b| b.is_ascii_digit()) {
            let no = parse_space_no(rest)?;
            return Ok(SpaceRefShape::Numbered {
                host: Some(parse_host_token(host)?),
                no,
            });
        }
        return Ok(SpaceRefShape::Named {
            host: Some(parse_host_token(host)?),
            name: rest.to_string(),
        });
    }
    if part.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(SpaceRefShape::Numbered {
            host: None,
            no: parse_space_no(part)?,
        });
    }
    // Compact `b2` / ID-shaped classification.
    if let Some(split) = split_id_shaped(part) {
        let (letters, digits) = split;
        if !letters.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(RefError::IdShapedInvalid);
        }
        let no = parse_space_no(digits)?;
        return Ok(SpaceRefShape::Numbered {
            host: Some(HostToken::AliasOrLabel(letters.to_string())),
            no,
        });
    }
    Ok(SpaceRefShape::Named {
        host: None,
        name: part.to_string(),
    })
}

/// `^([A-Za-z]+)([0-9]+)$` — the fixed ID-shaped lexical class.
fn split_id_shaped(s: &str) -> Option<(&str, &str)> {
    let first_digit = s.bytes().position(|b| b.is_ascii_digit())?;
    if first_digit == 0 {
        return None; // all-digit case handled by the caller
    }
    let (letters, digits) = s.split_at(first_digit);
    let shaped = letters.bytes().all(|b| b.is_ascii_alphabetic())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !digits.is_empty();
    shaped.then_some((letters, digits))
}

fn parse_space_no(digits: &str) -> Result<SpaceNo, RefError> {
    if !is_canonical_space_no(digits) {
        return Err(RefError::InvalidSpaceNo);
    }
    let n: u64 = digits.parse().map_err(|_| RefError::InvalidSpaceNo)?;
    Ok(SpaceNo(NonZeroU64::new(n).ok_or(RefError::InvalidSpaceNo)?))
}

/// Canonical nonzero decimal: `[1-9][0-9]*`.
pub fn is_canonical_space_no(s: &str) -> bool {
    let mut bytes = s.bytes();
    matches!(bytes.next(), Some(b'1'..=b'9')) && bytes.all(|b| b.is_ascii_digit())
}

fn parse_host_token(token: &str) -> Result<HostToken, RefError> {
    if let Ok(uid) = Uuid::try_parse(token) {
        // Hyphenated form only: the compact 32-hex form is not ref grammar.
        if token.len() == 36 {
            return Ok(HostToken::Uid(HostUid(uid)));
        }
    }
    if is_alias_or_label_shaped(token) {
        return Ok(HostToken::AliasOrLabel(token.to_string()));
    }
    Err(RefError::MalformedHostToken)
}

/// Union of the alias grammar (`[a-z]+`) and label grammar
/// (`[a-z][a-z0-9-]{0,31}`).
fn is_alias_or_label_shaped(s: &str) -> bool {
    let mut bytes = s.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && s.len() <= HOST_LABEL_MAX
}

fn parse_uri(rest: &str) -> Result<ParsedRef, RefError> {
    let mut segments = rest.split('/');
    let host = segments.next().ok_or(RefError::MalformedUri)?;
    let host = match parse_host_token(host)? {
        HostToken::Uid(uid) => uid,
        HostToken::AliasOrLabel(_) => return Err(RefError::MalformedUri),
    };
    if segments.next() != Some("spaces") {
        return Err(RefError::MalformedUri);
    }
    let space = segments.next().ok_or(RefError::MalformedUri)?;
    let space = SpaceUid(parse_uuid36(space).ok_or(RefError::MalformedUri)?);
    let child = match segments.next() {
        None => None,
        Some(kind_seg) => {
            let kind = match kind_seg {
                "groups" => ChildKind::Group,
                "splits" => ChildKind::Split,
                _ => return Err(RefError::MalformedUri),
            };
            let epoch = segments.next().ok_or(RefError::MalformedUri)?;
            let epoch = ServerEpoch(parse_uuid36(epoch).ok_or(RefError::MalformedUri)?);
            let handle = segments.next().ok_or(RefError::MalformedUri)?;
            let handle = parse_provider_handle(handle).ok_or(RefError::MalformedUri)?;
            Some(ChildRefShape {
                kind,
                epoch,
                handle,
            })
        }
    };
    if segments.next().is_some() {
        return Err(RefError::MalformedUri);
    }
    Ok(ParsedRef {
        space: SpaceRefShape::Canonical { host, space },
        child,
    })
}

/// `g<epoch-uuid>.<handle>` or `p<epoch-uuid>.<handle>`.
fn parse_child_suffix(suffix: &str) -> Result<ChildRefShape, RefError> {
    let mut chars = suffix.chars();
    let kind = match chars.next() {
        Some('g') => ChildKind::Group,
        Some('p') => ChildKind::Split,
        _ => return Err(RefError::MalformedChild),
    };
    let rest = chars.as_str();
    if !rest.is_ascii() || rest.len() < 37 || rest.as_bytes()[36] != b'.' {
        return Err(RefError::MalformedChild);
    }
    let epoch = ServerEpoch(parse_uuid36(&rest[..36]).ok_or(RefError::MalformedChild)?);
    let handle = parse_provider_handle(&rest[37..]).ok_or(RefError::MalformedChild)?;
    Ok(ChildRefShape {
        kind,
        epoch,
        handle,
    })
}

fn parse_uuid36(s: &str) -> Option<Uuid> {
    (s.len() == 36).then(|| Uuid::try_parse(s).ok()).flatten()
}

/// `wz-<decimal>` | `tx-<decimal>` | `x-<base64url-no-padding>`.
fn parse_provider_handle(s: &str) -> Option<ProviderHandle> {
    if let Some(d) = s.strip_prefix("wz-") {
        return parse_native_decimal(d).map(ProviderHandle::Wz);
    }
    if let Some(d) = s.strip_prefix("tx-") {
        return parse_native_decimal(d).map(ProviderHandle::Tx);
    }
    if let Some(b) = s.strip_prefix("x-") {
        let ok = !b.is_empty()
            && b.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
        return ok.then(|| ProviderHandle::Opaque(b.to_string()));
    }
    None
}

/// Native IDs may legitimately be 0; leading zeros are still malformed.
fn parse_native_decimal(s: &str) -> Option<u64> {
    let canonical = s == "0" || is_canonical_space_no(s);
    (canonical && !s.is_empty())
        .then(|| s.parse().ok())
        .flatten()
}

// ---- formatting ------------------------------------------------------------

pub fn canonical_uri(host: HostUid, space: SpaceUid) -> String {
    format!("{URI_SCHEME}{}/spaces/{}", host.0, space.0)
}

pub fn canonical_child_uri(host: HostUid, space: SpaceUid, child: &ChildRefShape) -> String {
    let seg = match child.kind {
        ChildKind::Group => "groups",
        ChildKind::Split => "splits",
    };
    format!(
        "{}/{seg}/{}/{}",
        canonical_uri(host, space),
        child.epoch.0,
        child.handle
    )
}

/// The `<SPACE_REF>/g<epoch>.<handle>` suffix form.
pub fn child_suffix(child: &ChildRefShape) -> String {
    let k = match child.kind {
        ChildKind::Group => 'g',
        ChildKind::Split => 'p',
    };
    format!("{k}{}.{}", child.epoch.0, child.handle)
}

// ---- name and label validation --------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    BadStart,
    BadChar,
    /// Matches an ID-shaped lexical class; reserved by the new-name grammar.
    IdShaped,
}

/// Validates a NEW managed name: `[A-Za-z][A-Za-z0-9_-]{0,63}`, excluding the
/// ID-shaped classes. (Legacy names bypass this via stable ID or `--name`.)
pub fn validate_new_name(name: &str) -> Result<(), NameError> {
    let bytes = name.as_bytes();
    match bytes.first() {
        None => return Err(NameError::Empty),
        Some(b) if b.is_ascii_alphabetic() => {}
        Some(_) => return Err(NameError::BadStart),
    }
    if name.len() > NEW_NAME_MAX {
        return Err(NameError::TooLong);
    }
    if !bytes[1..]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
    {
        return Err(NameError::BadChar);
    }
    if split_id_shaped(name).is_some() {
        return Err(NameError::IdShaped);
    }
    Ok(())
}

/// Host labels: lowercase ASCII `[a-z][a-z0-9-]{0,31}`.
pub fn is_valid_host_label(s: &str) -> bool {
    is_alias_or_label_shaped(s)
}

// ---- alias arithmetic ------------------------------------------------------

/// Bijective base-26: 1 → `a`, 26 → `z`, 27 → `aa` (plan §6.1: `z -> aa`).
/// Index 1 is always the local authority `a`.
pub fn index_to_alias(mut index: u64) -> Option<String> {
    if index == 0 {
        return None;
    }
    let mut out = Vec::new();
    while index > 0 {
        index -= 1;
        out.push(b'a' + (index % 26) as u8);
        index /= 26;
    }
    out.reverse();
    Some(String::from_utf8(out).expect("ascii"))
}

pub fn alias_to_index(alias: &str) -> Option<u64> {
    if alias.is_empty() {
        return None;
    }
    let mut index: u64 = 0;
    for b in alias.bytes() {
        if !b.is_ascii_lowercase() {
            return None;
        }
        index = index.checked_mul(26)?.checked_add((b - b'a') as u64 + 1)?;
    }
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no(n: u64) -> SpaceNo {
        SpaceNo(NonZeroU64::new(n).unwrap())
    }

    #[test]
    fn plan_grammar_truth_table() {
        // `2` — SpaceNo 2 on this machine.
        assert_eq!(
            parse_ref("2").unwrap().space,
            SpaceRefShape::Numbered {
                host: None,
                no: no(2)
            }
        );
        // `b2` — compact shorthand.
        assert_eq!(
            parse_ref("b2").unwrap().space,
            SpaceRefShape::Numbered {
                host: Some(HostToken::AliasOrLabel("b".into())),
                no: no(2)
            }
        );
        // `b:2` — expanded relative reference (same shape as compact).
        assert_eq!(parse_ref("b:2").unwrap(), parse_ref("b2").unwrap());
        // `b:project` and `archie:project` — exact host-qualified names.
        assert_eq!(
            parse_ref("archie:project").unwrap().space,
            SpaceRefShape::Named {
                host: Some(HostToken::AliasOrLabel("archie".into())),
                name: "project".into()
            }
        );
        // `<host-uuid>:2` — portable owner-qualified numeric reference.
        let uid = "0192aaaa-bbbb-7ccc-8ddd-eeeeffff0000";
        match parse_ref(&format!("{uid}:2")).unwrap().space {
            SpaceRefShape::Numbered {
                host: Some(HostToken::Uid(h)),
                no: n,
            } => {
                assert_eq!(h.0.to_string(), uid);
                assert_eq!(n, no(2));
            }
            other => panic!("wrong shape: {other:?}"),
        }
        // Bare logical name, local scope only.
        assert_eq!(
            parse_ref("project").unwrap().space,
            SpaceRefShape::Named {
                host: None,
                name: "project".into()
            }
        );
    }

    #[test]
    fn canonical_uri_round_trips_with_and_without_child() {
        let host = HostUid(Uuid::try_parse("0192aaaa-bbbb-4ccc-8ddd-eeeeffff0000").unwrap());
        let space = SpaceUid(Uuid::try_parse("0192aaaa-bbbb-7ccc-8ddd-eeeeffff1111").unwrap());
        let uri = canonical_uri(host, space);
        assert_eq!(
            parse_ref(&uri).unwrap(),
            ParsedRef {
                space: SpaceRefShape::Canonical { host, space },
                child: None
            }
        );
        let child = ChildRefShape {
            kind: ChildKind::Split,
            epoch: ServerEpoch(Uuid::try_parse("0192aaaa-bbbb-4ccc-8ddd-eeeeffff2222").unwrap()),
            handle: ProviderHandle::Wz(3),
        };
        let child_uri = canonical_child_uri(host, space, &child);
        let parsed = parse_ref(&child_uri).unwrap();
        assert_eq!(parsed.child.as_ref(), Some(&child));
    }

    #[test]
    fn child_suffix_round_trips_on_relative_refs() {
        let epoch = "0192aaaa-bbbb-4ccc-8ddd-eeeeffff2222";
        for (raw, kind, handle) in [
            (
                format!("b2/g{epoch}.wz-3"),
                ChildKind::Group,
                ProviderHandle::Wz(3),
            ),
            (
                format!("2/p{epoch}.tx-0"),
                ChildKind::Split,
                ProviderHandle::Tx(0),
            ),
            (
                format!("b:proj/p{epoch}.x-aGk_-1"),
                ChildKind::Split,
                ProviderHandle::Opaque("aGk_-1".into()),
            ),
        ] {
            let parsed = parse_ref(&raw).unwrap();
            let child = parsed.child.expect("child present");
            assert_eq!(child.kind, kind);
            assert_eq!(child.handle, handle);
            assert_eq!(child.epoch.0.to_string(), epoch);
            // Formatting round-trip.
            assert!(raw.ends_with(&child_suffix(&child)));
        }
    }

    #[test]
    fn zero_and_leading_zero_are_invalid_refs_never_names() {
        for bad in ["0", "007", "b:0", "b:012", "b0", "b007"] {
            let err = parse_ref(bad).unwrap_err();
            assert_eq!(err, RefError::InvalidSpaceNo, "{bad}");
        }
    }

    #[test]
    fn id_shaped_tokens_are_never_names() {
        // Lowercase prefix: valid compact shape even for unenrolled aliases.
        assert_eq!(
            parse_ref("main2").unwrap().space,
            SpaceRefShape::Numbered {
                host: Some(HostToken::AliasOrLabel("main".into())),
                no: no(2)
            }
        );
        // Non-lowercase prefix: invalid ref, never a name fallback.
        assert_eq!(parse_ref("B2").unwrap_err(), RefError::IdShapedInvalid);
        assert_eq!(parse_ref("Proj7").unwrap_err(), RefError::IdShapedInvalid);
    }

    #[test]
    fn malformed_host_tokens_error_rather_than_fall_back() {
        for bad in ["B:2", "Arch:proj", "a_b:2", "a:b:2", "b:"] {
            assert_eq!(
                parse_ref(bad).unwrap_err(),
                RefError::MalformedHostToken,
                "{bad}"
            );
        }
    }

    #[test]
    fn reserved_and_malformed_tokens() {
        assert_eq!(parse_ref("-").unwrap_err(), RefError::ReservedToken);
        assert_eq!(parse_ref("").unwrap_err(), RefError::Empty);
        // A slash without a valid child suffix is malformed, not a name.
        assert_eq!(parse_ref("foo/bar").unwrap_err(), RefError::MalformedChild);
        assert_eq!(
            parse_ref("dmux://nope").unwrap_err(),
            RefError::MalformedUri
        );
    }

    #[test]
    fn new_name_grammar() {
        assert_eq!(validate_new_name("project"), Ok(()));
        assert_eq!(validate_new_name("my-proj_2x"), Ok(()));
        assert_eq!(validate_new_name(""), Err(NameError::Empty));
        assert_eq!(validate_new_name("2proj"), Err(NameError::BadStart));
        assert_eq!(validate_new_name("has space"), Err(NameError::BadChar));
        assert_eq!(validate_new_name("proj2"), Err(NameError::IdShaped));
        assert_eq!(validate_new_name(&"x".repeat(65)), Err(NameError::TooLong));
        assert_eq!(validate_new_name(&"x".repeat(64)), Ok(()));
    }

    #[test]
    fn alias_rollover_is_bijective_base_26() {
        let cases = [
            (1, "a"),
            (2, "b"),
            (26, "z"),
            (27, "aa"),
            (28, "ab"),
            (52, "az"),
            (53, "ba"),
            (702, "zz"),
            (703, "aaa"),
        ];
        for (i, s) in cases {
            assert_eq!(index_to_alias(i).as_deref(), Some(s), "{i}");
            assert_eq!(alias_to_index(s), Some(i), "{s}");
        }
        assert_eq!(index_to_alias(0), None);
        assert_eq!(alias_to_index(""), None);
        assert_eq!(alias_to_index("A"), None);
    }
}
