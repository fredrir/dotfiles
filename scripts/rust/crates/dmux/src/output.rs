//! Human and versioned JSON rendering (plan §16, ADR 008).
//!
//! P4 shadow mode: these renderers are exercised by tests and the shadow
//! pipeline only; the legacy binary's output is untouched until later
//! phases wire commands through here. The JSON document shape is frozen
//! contract (ADR 008 §1) and golden-tested below.
//!
//! Root-owned (plan §19, W3).

use serde_json::{Value, json};

use crate::error::{ErrorCode, ExitStatus, TypedError};
use crate::inventory::{ManagedRow, ReconRow, UnmanagedRow};
use crate::model::{Backend, Observation};
use crate::operations::SpaceHierarchy;

pub const SCHEMA_VERSION: u32 = 1;

/// The global `--format` selection (plan §7.1). `json` is always the
/// versioned envelope below; a command's own older `--json` keeps emitting
/// its bare legacy payload for one release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

/// One bounded JSON document — exactly one per command, nothing else on
/// stdout (ADR 008 §1).
pub fn document(
    action: &str,
    ok: bool,
    result: Value,
    errors: &[TypedError],
    authority_revision: u64,
) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": ok,
        "action": action,
        "result": result,
        "errors": errors,
        "authority_revision": authority_revision,
    })
}

/// The exit-5 no-mutation document JSON destructive commands emit without
/// `--yes` (plan §7.4).
pub fn confirmation_required(
    action: &str,
    target: &str,
    authority_revision: u64,
) -> (Value, ExitStatus) {
    let mut err = TypedError::new(
        ErrorCode::ConfirmationRequired,
        format!("{action} needs --yes in JSON mode"),
    );
    err.target = Some(target.to_string());
    (
        document(action, false, Value::Null, &[err], authority_revision),
        ExitStatus::ConfirmationRequired,
    )
}

/// The overall exit status for a document: clean success, partial (7) when
/// a result coexists with errors, else the first error's mapping.
pub fn document_exit(ok: bool, has_result: bool, errors: &[TypedError]) -> ExitStatus {
    match errors.first() {
        None => ExitStatus::Success,
        Some(_) if ok || has_result => ExitStatus::Partial,
        Some(first) => first.code.exit_status(),
    }
}

// ---------------------------------------------------------------------------
// Native refs (plan §7.4): `native:<backend>:<base64url-no-padding>`

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn base64url_no_pad(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = [
            B64URL[(n >> 18) as usize & 63],
            B64URL[(n >> 12) as usize & 63],
            B64URL[(n >> 6) as usize & 63],
            B64URL[n as usize & 63],
        ];
        let keep = 1 + chunk.len() * 4 / 3;
        for &c in &chars[..keep.min(4)] {
            out.push(c as char);
        }
    }
    out
}

/// Opaque provider-qualified token for an unmanaged row. Never accepted as
/// a backend command string; `adopt` re-resolves it in a complete scan.
pub fn native_ref(backend: Backend, native_token: &str) -> String {
    format!(
        "native:{}:{}",
        backend.as_str(),
        base64url_no_pad(native_token.as_bytes())
    )
}

/// The inverse of [`native_ref`], for `adopt`. It yields the provider and the
/// token to re-resolve in a fresh complete scan — never a string to hand a
/// backend (plan §7.4) — so anything but an exact encoding is `invalid_ref`
/// rather than a token passed through.
pub fn parse_native_ref(token: &str) -> Result<(Backend, String), TypedError> {
    let reject = || TypedError::new(ErrorCode::InvalidRef, format!("not a native ref: {token}"));
    let (backend, encoded) = token
        .strip_prefix("native:")
        .and_then(|rest| rest.split_once(':'))
        .ok_or_else(reject)?;
    let backend = match backend {
        "wez" => Backend::Wez,
        "tmux" => Backend::Tmux,
        _ => return Err(reject()),
    };
    let native = decode_base64url_no_pad(encoded)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(reject)?;
    Ok((backend, native))
}

/// Strict: one encoding per token. A trailing character that carries no byte,
/// or nonzero padding bits, would give a second spelling of the same native
/// resource and is rejected instead.
fn decode_base64url_no_pad(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        let value = B64URL.iter().position(|c| *c == byte)? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    (bits < 6 && acc & ((1 << bits) - 1) == 0).then_some(out)
}

// ---------------------------------------------------------------------------
// ls rows (ADR 008 §1 shapes)

/// Context the row renderers need about the owner (shadow mode is
/// local-only; P7 extends this to enrolled remotes).
pub struct OwnerContext {
    pub host_uid: String,
    pub alias: String,
    pub label: Option<String>,
    pub route: String,
}

pub fn managed_row_json(row: &ManagedRow, owner: &OwnerContext) -> Value {
    let uri = format!("dmux://{}/spaces/{}", owner.host_uid, row.space.space_uid.0);
    json!({
        "managed": true,
        "uri": uri,
        "portable_ref": format!("{}:{}", owner.host_uid, row.space.space_no),
        "compact_ref": compact_ref(&owner.alias, row.space.space_no.get()),
        "space_uid": row.space.space_uid.0.to_string(),
        "space_no": row.space.space_no.get(),
        "name": row.space.logical_name,
        "owner": owner_json(owner),
        "backend": row.backend.as_str(),
        "backend_instance": row.space.backend_instance.0.to_string(),
        "groups": row.groups,
        "splits": row.splits,
        "lifecycle": row.space.lifecycle,
        "observation": row.observation,
        "health": row.space.health,
        "client": "unknown",
        "route": owner.route,
        "stale": false,
    })
}

pub fn unmanaged_row_json(row: &UnmanagedRow, owner: &OwnerContext) -> Value {
    json!({
        "managed": false,
        "native_ref": native_ref(row.backend, &row.native_token),
        "provider": row.backend.as_str(),
        "native_name": row.native_name,
        "owner": owner_json(owner),
        "backend_instance": Value::Null,
        "server_epoch": row.server_epoch.map(|e| e.0.to_string()),
        "groups": row.groups,
        "splits": row.splits,
        "health": "unknown",
    })
}

fn owner_json(owner: &OwnerContext) -> Value {
    json!({ "host_uid": owner.host_uid, "alias": owner.alias, "label": owner.label })
}

/// Compact display ref: the local authority `a` renders as the bare number
/// (`2`), remotes as alias+number (`b2`) — plan §6.2 examples.
pub fn compact_ref(alias: &str, space_no: u64) -> String {
    if alias == "a" {
        format!("{space_no}")
    } else {
        format!("{alias}{space_no}")
    }
}

// ---------------------------------------------------------------------------
// Human ls table (plan §16.1)

pub fn render_ls(rows: &[ReconRow], owner: &OwnerContext) -> String {
    const HEADERS: [&str; 10] = [
        "REF", "NAME", "BACKEND", "HOST", "GROUPS", "SPLITS", "SERVER", "CLIENT", "ROUTE", "STATE",
    ];
    let mut table: Vec<[String; 10]> = Vec::with_capacity(rows.len());
    for row in rows {
        table.push(match row {
            ReconRow::Managed(m) => [
                compact_ref(&owner.alias, m.space.space_no.get()),
                m.space.logical_name.clone(),
                m.backend.as_str().into(),
                owner.label.clone().unwrap_or_else(|| owner.alias.clone()),
                m.groups.to_string(),
                m.splits.to_string(),
                server_column(m.observation).into(),
                "unknown".into(),
                owner.route.clone(),
                state_column(m).into(),
            ],
            ReconRow::Unmanaged(u) => [
                "-".into(),
                u.native_name.clone(),
                u.backend.as_str().into(),
                owner.label.clone().unwrap_or_else(|| owner.alias.clone()),
                u.groups.to_string(),
                u.splits.to_string(),
                "running".into(),
                "unknown".into(),
                owner.route.clone(),
                if u.unepoched {
                    "unmanaged:unepoched".into()
                } else {
                    "unmanaged".into()
                },
            ],
        });
    }
    let mut widths: Vec<usize> = HEADERS.iter().map(|h| h.len()).collect();
    for row in &table {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    let mut out = String::new();
    let emit = |out: &mut String, cells: &[String]| {
        let line: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<w$}", w = widths[i]))
            .collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    };
    emit(&mut out, &HEADERS.map(String::from));
    for row in &table {
        emit(&mut out, row);
    }
    out
}

fn server_column(observation: Observation) -> &'static str {
    match observation {
        Observation::Live | Observation::Absent => "running",
        Observation::Stopped => "stopped",
        Observation::Unreachable | Observation::Unmanaged => "unreachable",
        Observation::Incompatible => "incompatible",
    }
}

fn state_column(m: &ManagedRow) -> &'static str {
    use crate::model::Health;
    if m.space.health != Health::Healthy {
        return match m.space.health {
            Health::MultiWindow => "multi_window",
            Health::NativeKeyCollision => "native_key_collision",
            Health::Unstamped => "unstamped",
            Health::Unknown => "unknown",
            Health::Healthy => unreachable!(),
        };
    }
    match m.observation {
        Observation::Live => "live",
        Observation::Absent => "absent",
        Observation::Stopped => "stopped",
        Observation::Unreachable => "unreachable",
        Observation::Incompatible => "incompatible",
        Observation::Unmanaged => "unmanaged",
    }
}

/// `ls --tree`: the same table, with each managed Space's live Groups and
/// Splits indented beneath its row. A Space whose hierarchy was not read
/// contributes no children — a missing hierarchy is not an empty one, and
/// unmanaged rows have no addressable children at all (plan §11.2).
pub fn render_tree<'a>(
    rows: &[ReconRow],
    owner: &OwnerContext,
    hierarchy_of: impl Fn(&ManagedRow) -> Option<&'a SpaceHierarchy>,
) -> String {
    let table = render_ls(rows, owner);
    let mut lines = table.lines();
    let mut out = String::new();
    if let Some(header) = lines.next() {
        out.push_str(header);
        out.push('\n');
    }
    // render_ls emits exactly one line per row, in order.
    for (row, line) in rows.iter().zip(lines) {
        out.push_str(line);
        out.push('\n');
        let ReconRow::Managed(managed) = row else {
            continue;
        };
        let Some(hierarchy) = hierarchy_of(managed) else {
            continue;
        };
        for group in &hierarchy.groups {
            out.push_str(&child_line(2, &group.group_ref, group.title.as_deref()));
            for split in &group.splits {
                out.push_str(&child_line(4, &split.split_ref, split.title.as_deref()));
            }
        }
    }
    out
}

fn child_line(indent: usize, child_ref: &str, title: Option<&str>) -> String {
    match title {
        Some(title) => format!("{:indent$}{child_ref}  {title}\n", ""),
        None => format!("{:indent$}{child_ref}\n", ""),
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use uuid::Uuid;

    use super::*;
    use crate::model::{
        BackendInstanceUid, Health, HostUid, Lifecycle, ServerEpoch, SpaceNo, SpaceUid,
    };
    use crate::operations::{HierarchyGroup, HierarchySplit};
    use crate::registry::SpaceRow;

    fn owner() -> OwnerContext {
        OwnerContext {
            host_uid: "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002".into(),
            alias: "a".into(),
            label: Some("macie".into()),
            route: "local".into(),
        }
    }

    fn managed() -> ManagedRow {
        ManagedRow {
            space: SpaceRow {
                space_uid: SpaceUid(
                    Uuid::try_parse("0192aaaa-bbbb-7ccc-8ddd-eeeeffff1111").unwrap(),
                ),
                owner: HostUid(Uuid::try_parse("0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002").unwrap()),
                space_no: SpaceNo(NonZeroU64::new(2).unwrap()),
                backend_instance: BackendInstanceUid(
                    Uuid::try_parse("0192aaaa-bbbb-4ccc-8ddd-eeeeffff0004").unwrap(),
                ),
                logical_name: "dotfiles".into(),
                lifecycle: Lifecycle::Active,
                health: Health::Healthy,
                created_at: "2026-08-16T00:00:00Z".into(),
                updated_at: "2026-08-16T00:00:00Z".into(),
                deleted_at: None,
            },
            backend: Backend::Wez,
            observation: Observation::Live,
            groups: 2,
            splits: 4,
            server_epoch: None,
            native_token: Some("dmux:host:space".into()),
            multi_window: false,
        }
    }

    /// The ADR 008 §1 golden shape: field-for-field.
    #[test]
    fn managed_row_matches_adr_008_shape() {
        let v = managed_row_json(&managed(), &owner());
        assert_eq!(
            v["uri"],
            "dmux://0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002/spaces/0192aaaa-bbbb-7ccc-8ddd-eeeeffff1111"
        );
        assert_eq!(v["portable_ref"], "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002:2");
        assert_eq!(v["compact_ref"], "2");
        assert_eq!(v["space_no"], 2);
        assert_eq!(v["backend"], "wez");
        assert_eq!(v["lifecycle"], "active");
        assert_eq!(v["observation"], "live");
        assert_eq!(v["health"], "healthy");
        assert_eq!(v["managed"], true);
        assert_eq!(v["owner"]["alias"], "a");
        assert_eq!(v["stale"], false);
    }

    #[test]
    fn unmanaged_row_has_no_fabricated_identity() {
        let row = UnmanagedRow {
            backend: Backend::Wez,
            native_token: "scratch".into(),
            native_name: "scratch".into(),
            groups: 1,
            splits: 1,
            server_epoch: None,
            multi_window: false,
            unepoched: false,
        };
        let v = unmanaged_row_json(&row, &owner());
        assert_eq!(v["managed"], false);
        assert_eq!(v["native_ref"], "native:wez:c2NyYXRjaA");
        assert!(v.get("space_no").is_none());
        assert!(v.get("compact_ref").is_none());
        assert!(v.get("uri").is_none());
    }

    #[test]
    fn document_shape_and_exit_mapping() {
        let doc = document("list", true, json!([]), &[], 42);
        assert_eq!(doc["schema_version"], 1);
        assert_eq!(doc["ok"], true);
        assert_eq!(doc["action"], "list");
        assert_eq!(doc["errors"], json!([]));
        assert_eq!(doc["authority_revision"], 42);
        assert_eq!(document_exit(true, true, &[]), ExitStatus::Success);

        let err = TypedError::new(ErrorCode::ProviderUnavailable, "wez down");
        assert_eq!(
            document_exit(true, true, std::slice::from_ref(&err)),
            ExitStatus::Partial
        );
        assert_eq!(document_exit(false, false, &[err]), ExitStatus::Unavailable);
    }

    #[test]
    fn confirmation_required_is_exit_5_and_mutates_nothing_by_shape() {
        let (doc, exit) = confirmation_required("rm", "dmux://h/spaces/s", 42);
        assert_eq!(exit, ExitStatus::ConfirmationRequired);
        assert_eq!(doc["ok"], false);
        assert_eq!(doc["errors"][0]["code"], "confirmation_required");
        assert_eq!(doc["errors"][0]["target"], "dmux://h/spaces/s");
        assert_eq!(doc["result"], Value::Null);
    }

    #[test]
    fn base64url_vectors() {
        assert_eq!(base64url_no_pad(b""), "");
        assert_eq!(base64url_no_pad(b"f"), "Zg");
        assert_eq!(base64url_no_pad(b"fo"), "Zm8");
        assert_eq!(base64url_no_pad(b"foo"), "Zm9v");
        assert_eq!(base64url_no_pad(b"foob"), "Zm9vYg");
        assert_eq!(base64url_no_pad(&[0xfb, 0xff, 0xfe]), "-__-");
    }

    #[test]
    fn base64url_decodes_every_vector_back() {
        for bytes in [
            b"".as_slice(),
            b"f",
            b"fo",
            b"foo",
            b"foob",
            &[0xfb, 0xff, 0xfe],
        ] {
            let encoded = base64url_no_pad(bytes);
            assert_eq!(decode_base64url_no_pad(&encoded).as_deref(), Some(bytes));
        }
    }

    #[test]
    fn native_refs_round_trip_and_reject_anything_else() {
        for token in ["scratch", "dmux:host:space", "a b", "=oops"] {
            for backend in [Backend::Wez, Backend::Tmux] {
                let encoded = native_ref(backend, token);
                assert_eq!(
                    parse_native_ref(&encoded).unwrap(),
                    (backend, token.to_string())
                );
            }
        }
        for bad in [
            "scratch",
            "native:wez",
            "native:ssh:c2NyYXRjaA",
            "native:wez:c2NyYXRja",
            "native:wez:c2NyYXRjaA=",
            "native:wez:Zh",
        ] {
            let error = parse_native_ref(bad).expect_err(bad);
            assert_eq!(error.code, ErrorCode::InvalidRef);
        }
    }

    #[test]
    fn tree_indents_children_under_their_managed_row_only() {
        let unmanaged = UnmanagedRow {
            backend: Backend::Tmux,
            native_token: "scratch".into(),
            native_name: "scratch".into(),
            groups: 1,
            splits: 1,
            server_epoch: None,
            multi_window: false,
            unepoched: true,
        };
        let rows = vec![ReconRow::Managed(managed()), ReconRow::Unmanaged(unmanaged)];
        let hierarchy = SpaceHierarchy {
            space_uid: managed().space.space_uid,
            server_epoch: ServerEpoch(
                Uuid::try_parse("0192aaaa-bbbb-4ccc-8ddd-eeeeffff2222").unwrap(),
            ),
            groups: vec![HierarchyGroup {
                group_ref: "g0192aaaa-bbbb-4ccc-8ddd-eeeeffff2222.wz1".into(),
                title: Some("editor".into()),
                splits: vec![HierarchySplit {
                    split_ref: "p0192aaaa-bbbb-4ccc-8ddd-eeeeffff2222.wz3".into(),
                    title: None,
                    cwd: Some("/tmp".into()),
                }],
            }],
        };
        let text = render_tree(&rows, &owner(), |row| {
            (row.space.space_uid == hierarchy.space_uid).then_some(&hierarchy)
        });
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("REF"));
        assert!(lines[1].starts_with('2'));
        assert_eq!(
            lines[2],
            "  g0192aaaa-bbbb-4ccc-8ddd-eeeeffff2222.wz1  editor"
        );
        assert_eq!(lines[3], "    p0192aaaa-bbbb-4ccc-8ddd-eeeeffff2222.wz3");
        assert!(lines[4].contains("unmanaged:unepoched"));
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn human_table_columns() {
        let rows = vec![ReconRow::Managed(managed())];
        let text = render_ls(&rows, &owner());
        let mut lines = text.lines();
        let header = lines.next().unwrap();
        assert!(header.starts_with("REF"));
        for col in [
            "NAME", "BACKEND", "HOST", "GROUPS", "SPLITS", "SERVER", "CLIENT", "ROUTE", "STATE",
        ] {
            assert!(header.contains(col), "{col}");
        }
        let row = lines.next().unwrap();
        assert!(row.starts_with('2'));
        assert!(row.contains("dotfiles"));
        assert!(row.contains("wez"));
        assert!(row.contains("running"));
        assert!(row.contains("live"));
    }
}
