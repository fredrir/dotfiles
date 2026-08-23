//! P4 gate: `dmux ls` on the Wez-first path (plan cases 13, 24, 25, 43).
//!
//! Two layers. The fan-out, the scope refusals, the filters, and every
//! rendering decision run against a test double, because "one unreachable
//! peer still lists the rest" must be assertable without a peer. The two
//! owner-local claims that are about the real pipeline — a stopped Wez
//! service lists as `stopped` and starts nothing, an external workspace
//! stays unmanaged — run against a scratch registry, a real socket probe,
//! and a stub `wezterm`, so no seam can quietly answer them.

use std::collections::HashMap;
use std::fs;
use std::num::NonZeroU64;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;

use dmux::error::{ErrorCode, ExitStatus, TypedError};
use dmux::inventory::{ManagedRow, ReconRow};
use dmux::ls_cli::{self, Authority, HostListing, LsArgs, LsHost, LsSource, ScanFailure};
use dmux::model::{
    Backend, BackendInstanceUid, Health, HostUid, Lifecycle, Observation, ServerEpoch, SpaceNo,
    SpaceUid,
};
use dmux::operations::{
    HierarchyGroup, HierarchySplit, OperationEnv, SpaceHierarchy, TmuxBootstrapOutcome,
    tmux_bootstrap,
};
use dmux::output::{self, OutputFormat};
use dmux::registry::{NativeBindingSpec, NativeKind, Registry, RegistryConfig, SpaceRow};
use dmux::remote::protocol;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Doubles

#[derive(Default)]
struct Fake {
    hosts: Vec<LsHost>,
    listings: HashMap<HostUid, Result<HostListing, TypedError>>,
    hierarchies: HashMap<SpaceUid, SpaceHierarchy>,
}

impl LsSource for Fake {
    fn hosts(&self) -> Result<Vec<LsHost>, TypedError> {
        Ok(self.hosts.clone())
    }

    fn listing(&self, host: &LsHost) -> Result<HostListing, TypedError> {
        self.listings
            .get(&host.host_uid)
            .cloned()
            .unwrap_or_else(|| Ok(HostListing::default()))
    }

    fn hierarchy(&self, host: &LsHost, row: &ManagedRow) -> Option<SpaceHierarchy> {
        host.local
            .then(|| self.hierarchies.get(&row.space.space_uid).cloned())
            .flatten()
    }

    fn authority_revision(&self) -> u64 {
        42
    }
}

fn host(n: u128, alias: &str, label: &str, local: bool) -> LsHost {
    LsHost {
        host_uid: HostUid(Uuid::from_u128(n)),
        alias: alias.into(),
        label: Some(label.into()),
        route: if local { "local".into() } else { "ssh".into() },
        local,
    }
}

fn managed(no: u64, name: &str, backend: Backend) -> ManagedRow {
    ManagedRow {
        space: SpaceRow {
            space_uid: SpaceUid(Uuid::from_u128(0x1000 + no as u128)),
            owner: HostUid(Uuid::nil()),
            space_no: SpaceNo(NonZeroU64::new(no).unwrap()),
            backend_instance: BackendInstanceUid(Uuid::nil()),
            logical_name: name.into(),
            lifecycle: Lifecycle::Active,
            health: Health::Healthy,
            created_at: String::new(),
            updated_at: String::new(),
            deleted_at: None,
        },
        backend,
        observation: Observation::Live,
        groups: 2,
        splits: 4,
        server_epoch: None,
        native_token: Some(format!("native-{no}")),
        multi_window: false,
    }
}

fn local_listing(rows: Vec<ReconRow>) -> HostListing {
    HostListing {
        rows,
        counts: true,
        route: None,
        notes: Vec::new(),
        errors: Vec::new(),
    }
}

/// A peer's answer: the durable rows the frozen `spaces` method carries,
/// with no per-row counts and no unmanaged rows.
fn peer_listing(rows: Vec<ReconRow>) -> HostListing {
    HostListing {
        rows,
        counts: false,
        route: Some("tailscale".into()),
        notes: Vec::new(),
        errors: Vec::new(),
    }
}

fn two_hosts(peer: Result<HostListing, TypedError>) -> Fake {
    let (a, b) = (host(1, "a", "macie", true), host(2, "b", "archie", false));
    let mut listings = HashMap::new();
    listings.insert(
        a.host_uid,
        Ok(local_listing(vec![ReconRow::Managed(managed(
            2,
            "dotfiles",
            Backend::Wez,
        ))])),
    );
    listings.insert(b.host_uid, peer);
    Fake {
        hosts: vec![a, b],
        listings,
        hierarchies: HashMap::new(),
    }
}

fn run(source: &dyn LsSource, format: Option<OutputFormat>, args: LsArgs) -> ls_cli::LsOutput {
    ls_cli::render(source, format, &args)
}

fn json(out: &ls_cli::LsOutput) -> serde_json::Value {
    serde_json::from_str(&out.stdout).expect("stdout is one JSON document")
}

// ---------------------------------------------------------------------------
// Scope distinctness (case 24)

/// Case 24 asks for four *documented* scopes; the help text is where a user
/// learns they differ, so it names all four and both axes.
#[test]
fn the_help_names_every_scope_and_both_axes() {
    for phrase in [
        "dmux ls ",
        "dmux ls --tree",
        "dmux ls --all-hosts",
        "dmux host ls",
        "hosts and their routes only, never Spaces",
        "--all-hosts controls host breadth",
    ] {
        assert!(
            ls_cli::SCOPES_HELP.contains(phrase),
            "SCOPES_HELP is missing {phrase:?}"
        );
    }
}

#[test]
fn the_default_scope_is_one_host() {
    let out = run(
        &two_hosts(Ok(peer_listing(vec![]))),
        None,
        LsArgs::default(),
    );
    assert_eq!(out.status, ExitStatus::Success);
    assert!(out.stdout.contains("dotfiles"));
    assert!(
        !out.stdout.contains("archie"),
        "a bare ls must not query peers: {}",
        out.stdout
    );
}

#[test]
fn a_named_host_selects_exactly_that_host() {
    let source = two_hosts(Ok(peer_listing(vec![ReconRow::Managed(managed(
        7,
        "monitoring",
        Backend::Tmux,
    ))])));
    let out = run(
        &source,
        None,
        LsArgs {
            host: Some("archie".into()),
            ..LsArgs::default()
        },
    );
    assert_eq!(out.status, ExitStatus::Success);
    assert!(out.stdout.contains("monitoring"));
    assert!(!out.stdout.contains("dotfiles"));
    // §6.2: a peer's compact ref is alias+number.
    assert!(out.stdout.contains("b7"), "{}", out.stdout);
}

#[test]
fn an_unknown_host_is_not_found() {
    let out = run(
        &two_hosts(Ok(peer_listing(vec![]))),
        None,
        LsArgs {
            host: Some("nowhere".into()),
            ..LsArgs::default()
        },
    );
    assert_eq!(out.status, ExitStatus::NotFound);
    assert!(out.stdout.is_empty());
}

/// `--all-hosts` is declared `conflicts_with = "host"`, but clap enforces
/// that only when both follow the subcommand: `dmux --host h ls --all-hosts`
/// arrives with both set and has to be refused here.
#[test]
fn a_global_host_cannot_narrow_all_hosts() {
    let out = run(
        &two_hosts(Ok(peer_listing(vec![]))),
        None,
        LsArgs {
            host: Some("archie".into()),
            all_hosts: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(out.status, ExitStatus::Usage);
    assert!(out.stdout.is_empty(), "a refusal lists nothing");
    assert!(
        out.stderr.iter().any(|line| line.contains("--all-hosts")),
        "{:?}",
        out.stderr
    );
}

#[test]
fn a_refused_json_listing_is_still_one_document() {
    let out = run(
        &two_hosts(Ok(peer_listing(vec![]))),
        Some(OutputFormat::Json),
        LsArgs {
            host: Some("archie".into()),
            all_hosts: true,
            ..LsArgs::default()
        },
    );
    let doc = json(&out);
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["errors"][0]["code"], "usage");
    assert_eq!(doc["result"], serde_json::Value::Null);
}

// ---------------------------------------------------------------------------
// --all-hosts

#[test]
fn all_hosts_lists_every_enrolled_host() {
    let source = two_hosts(Ok(peer_listing(vec![ReconRow::Managed(managed(
        7,
        "monitoring",
        Backend::Tmux,
    ))])));
    let out = run(
        &source,
        None,
        LsArgs {
            all_hosts: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(out.status, ExitStatus::Success);
    assert!(out.stdout.contains("dotfiles") && out.stdout.contains("monitoring"));
    assert!(out.stdout.contains("macie") && out.stdout.contains("archie"));
    // The route each host's rows arrived over, not a guess made up front.
    assert!(out.stdout.contains("tailscale"), "{}", out.stdout);
}

/// A peer that cannot be reached is a typed `errors[]` entry beside the
/// hosts that answered, and exit 7 — not a failed listing (§16.2).
#[test]
fn an_unreachable_peer_is_a_partial_listing() {
    let source = two_hosts(Err(TypedError::new(
        ErrorCode::RouteUnavailable,
        "no enabled route to host",
    )));
    let out = run(
        &source,
        Some(OutputFormat::Json),
        LsArgs {
            all_hosts: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(out.status, ExitStatus::Partial);
    let doc = json(&out);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["result"].as_array().unwrap().len(), 1);
    assert_eq!(doc["errors"][0]["code"], "route_unavailable");
    assert_eq!(
        doc["errors"][0]["target"],
        HostUid(Uuid::from_u128(2)).0.to_string()
    );
    assert!(
        out.stderr.iter().any(|line| line.contains("archie")),
        "an unavailable host is visibly reported: {:?}",
        out.stderr
    );
}

/// With nothing left to report, the failure keeps its own status rather
/// than being softened to partial.
#[test]
fn every_host_failing_keeps_the_error_status() {
    let mut source = two_hosts(Err(TypedError::new(
        ErrorCode::RouteUnavailable,
        "no enabled route to host",
    )));
    source.listings.insert(
        HostUid(Uuid::from_u128(1)),
        Err(TypedError::new(ErrorCode::ProviderUnavailable, "no mux")),
    );
    let out = run(
        &source,
        None,
        LsArgs {
            all_hosts: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(out.status, ExitStatus::Unavailable);
}

/// Peer rows carry no per-Space counts, and saying `0` would be a lie the
/// operator cannot see through — so the columns read `-`, `null` in JSON,
/// with a visible per-host note.
#[test]
fn peer_rows_report_unknown_counts_not_zero() {
    let source = two_hosts(Ok(peer_listing(vec![ReconRow::Managed(managed(
        7,
        "monitoring",
        Backend::Tmux,
    ))])));
    let human = run(
        &source,
        None,
        LsArgs {
            all_hosts: true,
            ..LsArgs::default()
        },
    );
    let row = human
        .stdout
        .lines()
        .find(|line| line.contains("monitoring"))
        .unwrap();
    let cells: Vec<&str> = row.split_whitespace().collect();
    assert_eq!((cells[4], cells[5]), ("-", "-"), "{row:?}");
    assert!(
        human
            .stderr
            .iter()
            .any(|line| line.contains("archie") && line.contains("Group/Split counts")),
        "{:?}",
        human.stderr
    );

    let doc = json(&run(
        &source,
        Some(OutputFormat::Json),
        LsArgs {
            all_hosts: true,
            ..LsArgs::default()
        },
    ));
    let peer = doc["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "monitoring")
        .unwrap()
        .clone();
    assert!(peer["groups"].is_null() && peer["splits"].is_null());
    let local = doc["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "dotfiles")
        .unwrap();
    assert_eq!(local["groups"], 2, "local counts stay real");
}

// ---------------------------------------------------------------------------
// --tree

#[test]
fn tree_indents_live_children_under_their_space() {
    let mut source = two_hosts(Ok(peer_listing(vec![])));
    let space = SpaceUid(Uuid::from_u128(0x1000 + 2));
    source.hierarchies.insert(
        space,
        SpaceHierarchy {
            space_uid: space,
            server_epoch: ServerEpoch(Uuid::from_u128(9)),
            groups: vec![HierarchyGroup {
                group_ref: "g00000000-0000-0000-0000-000000000009.wz1".into(),
                title: Some("editor".into()),
                splits: vec![HierarchySplit {
                    split_ref: "p00000000-0000-0000-0000-000000000009.wz3".into(),
                    title: None,
                    cwd: None,
                }],
            }],
        },
    );
    let flat = run(&source, None, LsArgs::default());
    let tree = run(
        &source,
        None,
        LsArgs {
            tree: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(flat.stdout.lines().count(), 2, "header plus one Space");
    let lines: Vec<&str> = tree.stdout.lines().collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(
        lines[2],
        "  g00000000-0000-0000-0000-000000000009.wz1  editor"
    );
    assert_eq!(lines[3], "    p00000000-0000-0000-0000-000000000009.wz3");
}

/// `--tree` used to pair children with rows by zipping the rendered lines,
/// so a `logical_name` holding a newline filed the next Space's children
/// under the wrong parent and dropped the last row entirely. Names arrive
/// here unvalidated — `adopt` passes `--name` or the native workspace name
/// straight through, and §17 step 8 batch-adopts legacy ones.
#[test]
fn a_newline_in_a_name_cannot_misfile_children_or_drop_a_row() {
    let mut source = two_hosts(Ok(peer_listing(vec![])));
    source.listings.insert(
        HostUid(Uuid::from_u128(1)),
        Ok(local_listing(vec![
            ReconRow::Managed(managed(2, "evil\nROW  injected", Backend::Wez)),
            ReconRow::Managed(managed(3, "honest", Backend::Wez)),
        ])),
    );
    let space = SpaceUid(Uuid::from_u128(0x1000 + 3));
    source.hierarchies.insert(
        space,
        SpaceHierarchy {
            space_uid: space,
            server_epoch: ServerEpoch(Uuid::from_u128(9)),
            groups: vec![HierarchyGroup {
                group_ref: "g00000000-0000-0000-0000-000000000009.wz1".into(),
                title: Some("editor".into()),
                splits: vec![HierarchySplit {
                    split_ref: "p00000000-0000-0000-0000-000000000009.wz3".into(),
                    title: None,
                    cwd: None,
                }],
            }],
        },
    );
    let tree = run(
        &source,
        None,
        LsArgs {
            tree: true,
            ..LsArgs::default()
        },
    );
    let lines: Vec<&str> = tree.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        5,
        "header, two rows, one Group, one Split: {lines:?}"
    );
    assert!(lines[1].starts_with('2') && lines[1].contains("evil\\nROW"));
    assert!(
        lines[2].starts_with('3'),
        "the second Space keeps its own row: {lines:?}"
    );
    assert_eq!(
        lines[3], "  g00000000-0000-0000-0000-000000000009.wz1  editor",
        "children belong to the Space they were read from"
    );
    assert_eq!(lines[4], "    p00000000-0000-0000-0000-000000000009.wz3");

    // `--names` is one name per line for the shell wrappers, so the same
    // name must not read as two Spaces neither of which exists.
    let names = run(
        &source,
        None,
        LsArgs {
            names: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(names.stdout, "evil\\nROW  injected\nhonest\n");
}

/// Column widths are terminal columns, not bytes: legacy `list::render`
/// measured with `unicode_width` and the Wez-first table has to as well, or
/// one accented name shifts every column to its right.
#[test]
fn a_wide_name_keeps_the_columns_aligned() {
    let mut source = two_hosts(Ok(peer_listing(vec![])));
    source.listings.insert(
        HostUid(Uuid::from_u128(1)),
        Ok(local_listing(vec![
            // Both names are six columns wide, so both rows must pad the
            // NAME column identically.
            ReconRow::Managed(managed(2, "日本語", Backend::Wez)),
            ReconRow::Managed(managed(3, "abcdef", Backend::Wez)),
        ])),
    );
    let out = run(&source, None, LsArgs::default());
    let lines: Vec<&str> = out.stdout.lines().collect();
    let tail = |line: &str| line[line.find("wez").unwrap()..].to_string();
    assert_eq!(tail(lines[1]), tail(lines[2]));
    let spaces = |line: &str, name: &str| {
        line[line.find(name).unwrap() + name.len()..]
            .chars()
            .take_while(|c| *c == ' ')
            .count()
    };
    assert_eq!(spaces(lines[1], "日本語"), spaces(lines[2], "abcdef"));
}

#[test]
fn tree_says_so_when_a_peer_cannot_be_expanded() {
    let source = two_hosts(Ok(peer_listing(vec![ReconRow::Managed(managed(
        7,
        "monitoring",
        Backend::Tmux,
    ))])));
    let out = run(
        &source,
        None,
        LsArgs {
            all_hosts: true,
            tree: true,
            ..LsArgs::default()
        },
    );
    assert!(
        out.stderr
            .iter()
            .any(|line| line.contains("owner-local Spaces only")),
        "{:?}",
        out.stderr
    );
}

// ---------------------------------------------------------------------------
// Output shapes (case 43)

#[test]
fn format_json_is_the_versioned_envelope_and_json_stays_bare() {
    let source = two_hosts(Ok(peer_listing(vec![])));
    let envelope = json(&run(&source, Some(OutputFormat::Json), LsArgs::default()));
    assert_eq!(envelope["schema_version"], output::SCHEMA_VERSION);
    assert_eq!(envelope["action"], "list");
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["authority_revision"], 42);
    assert_eq!(envelope["result"][0]["name"], "dotfiles");
    assert_eq!(envelope["result"][0]["compact_ref"], "2");

    let bare = json(&run(
        &source,
        None,
        LsArgs {
            json: true,
            ..LsArgs::default()
        },
    ));
    assert!(bare.is_array(), "--json stays a bare row array: {bare}");
    assert_eq!(bare[0]["name"], "dotfiles");
}

/// ADR 011 D2: the deprecated `--json` keeps emitting *today's* bare
/// payload, so a script that reads `.index`/`.kind`/`.windows`/`.attached`
/// survives the flip. Field names, field order, and types are the frozen
/// legacy `list::Row` shape `cli::ls_json_carries_the_full_rows` pins on the
/// other side of the gate.
#[test]
fn the_deprecated_json_payload_is_the_legacy_row_shape() {
    let source = two_hosts(Ok(peer_listing(vec![])));
    let out = run(
        &source,
        None,
        LsArgs {
            json: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(
        out.stdout,
        "[{\"index\":1,\"name\":\"dotfiles\",\"kind\":\"wez\",\"host\":\"macie\",\
         \"created\":null,\"windows\":2,\"attached\":false}]\n"
    );
}

/// The deprecation hints are stderr-only: `--json` consumers compare stdout
/// byte for byte, so a hint there would break them (§7.4, §17).
#[test]
fn deprecated_flags_hint_on_stderr_only() {
    let source = two_hosts(Ok(peer_listing(vec![])));
    let out = run(
        &source,
        None,
        LsArgs {
            json: true,
            only_wez: true,
            ..LsArgs::default()
        },
    );
    assert!(json(&out).is_array());
    let hints = out.stderr.join("\n");
    assert!(hints.contains("--json is deprecated; use --format json"));
    assert!(hints.contains("--wez is deprecated; use --backend wez"));
}

#[test]
fn the_backend_filter_and_its_deprecated_spellings_agree() {
    let mut source = two_hosts(Ok(peer_listing(vec![])));
    source.listings.insert(
        HostUid(Uuid::from_u128(1)),
        Ok(local_listing(vec![
            ReconRow::Managed(managed(2, "dotfiles", Backend::Wez)),
            ReconRow::Managed(managed(3, "logs", Backend::Tmux)),
        ])),
    );
    let by_flag = run(
        &source,
        None,
        LsArgs {
            only_tmux: true,
            ..LsArgs::default()
        },
    );
    let by_backend = run(
        &source,
        None,
        LsArgs {
            backend: Some(Backend::Tmux),
            ..LsArgs::default()
        },
    );
    assert_eq!(by_flag.stdout, by_backend.stdout);
    assert!(by_flag.stdout.contains("logs") && !by_flag.stdout.contains("dotfiles"));

    let contradiction = run(
        &source,
        None,
        LsArgs {
            only_wez: true,
            backend: Some(Backend::Tmux),
            ..LsArgs::default()
        },
    );
    assert_eq!(contradiction.status, ExitStatus::Usage);
}

/// §16.2 admits exactly one document under `--format json`. clap already
/// refuses `--names --json`; the global spelling is the same collision and
/// must not answer it with bare names on stdout.
/// A `--backend` filter narrows what was asked for, so the other backend's
/// failed scan must not turn that answer into a partial one — and must not
/// be reported as if it had.
#[test]
fn a_backend_filter_drops_the_other_backends_scan_failure() {
    let mut source = two_hosts(Ok(peer_listing(vec![])));
    let mut listing = local_listing(vec![ReconRow::Managed(managed(
        2,
        "dotfiles",
        Backend::Wez,
    ))]);
    listing.errors.push(ScanFailure {
        backend: Backend::Tmux,
        error: TypedError::new(
            ErrorCode::ProviderUnavailable,
            "tmux inventory is indeterminate: socket refused",
        ),
    });
    source
        .listings
        .insert(HostUid(Uuid::from_u128(1)), Ok(listing));

    let wez_only = run(
        &source,
        None,
        LsArgs {
            backend: Some(Backend::Wez),
            ..LsArgs::default()
        },
    );
    assert_eq!(wez_only.status, ExitStatus::Success);
    assert!(
        !wez_only.stderr.iter().any(|line| line.contains("tmux")),
        "{:?}",
        wez_only.stderr
    );
    assert_eq!(
        run(&source, None, LsArgs::default()).status,
        ExitStatus::Partial,
        "unfiltered, the same failure is a partial listing"
    );
}

#[test]
fn names_and_format_json_is_refused_as_one_document() {
    let out = run(
        &two_hosts(Ok(peer_listing(vec![]))),
        Some(OutputFormat::Json),
        LsArgs {
            names: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(out.status, ExitStatus::Usage);
    let doc = json(&out);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["errors"][0]["code"], "usage");
    assert_eq!(doc["result"], serde_json::Value::Null);
    assert_eq!(
        doc["authority_revision"], 42,
        "a refusal that could read the registry reports its real head"
    );
}

#[test]
fn names_prints_one_name_per_line() {
    let out = run(
        &two_hosts(Ok(peer_listing(vec![]))),
        None,
        LsArgs {
            names: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(out.stdout, "dotfiles\n");
}

/// The multi-host table and the shared single-owner renderer must not
/// drift: an extra host with no rows changes which one runs and nothing
/// else, so the bytes have to match.
#[test]
fn the_multi_host_table_agrees_with_the_shared_renderer() {
    let source = two_hosts(Ok(peer_listing(vec![])));
    let rows = vec![ReconRow::Managed(managed(2, "dotfiles", Backend::Wez))];
    let expected = output::render_ls(&rows, &host(1, "a", "macie", true).owner());
    let out = run(
        &source,
        None,
        LsArgs {
            all_hosts: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(out.stdout, expected);
}

// ---------------------------------------------------------------------------
// The owner-local pipeline, end to end

const EPOCH: &str = "12345678-9abc-4ef0-8234-56789abcdef0";

struct Scratch {
    dir: tempfile::TempDir,
    /// Unique `-L` namespace, so the tmux side of every scan reaches a
    /// server this test owns and never the developer's own sessions.
    namespace: String,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
    }
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("locks")).unwrap();
        Scratch {
            dir,
            namespace: format!("dmux-ls-{tag}-{}", std::process::id()),
        }
    }

    fn tmux(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
        std::process::Command::new("tmux")
            .args(["-L", &self.namespace])
            .args(args)
            .output()
    }

    /// The production source with both endpoints pointed at this scratch.
    fn source(&self, wez_bin: String) -> Authority {
        Authority::with_wez(self.env(), wez_bin, "/dev/null")
            .with_tmux_namespace(self.namespace.clone())
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.dir.path().join("registry.sqlite3"),
            lock_dir: self.dir.path().join("locks"),
        }
    }

    fn registry(&self) -> Registry {
        let env = self.env();
        Registry::open(RegistryConfig::new(env.db_path, env.lock_dir)).unwrap()
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).display().to_string()
    }

    /// A `wezterm` stand-in that records every invocation, so "never starts
    /// a stopped server" is proven by the absence of the witness rather than
    /// by trusting the argv.
    fn stub_wezterm(&self, stdout: &str) -> String {
        let path = self.dir.path().join("wezterm");
        let witness = self.path("wezterm-ran");
        fs::write(
            &path,
            format!("#!/bin/sh\necho ran >> '{witness}'\nprintf '%s' '{stdout}'\n"),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    fn ran_wezterm(&self) -> bool {
        Path::new(&self.path("wezterm-ran")).exists()
    }
}

fn wez_pane(window: u32, tab: u32, pane: u32, workspace: &str) -> String {
    format!(r#"{{"window_id":{window},"tab_id":{tab},"pane_id":{pane},"workspace":"{workspace}"}}"#)
}

fn register_wez(scratch: &Scratch, socket: &str) -> (BackendInstanceUid, ServerEpoch) {
    let mut registry = scratch.registry();
    let instance = registry
        .register_backend_instance(Backend::Wez, Some(socket), None)
        .unwrap();
    let epoch = ServerEpoch(Uuid::try_parse(EPOCH).unwrap());
    registry
        .publish_backend_server(instance, epoch, None, None, None, None)
        .unwrap();
    (instance, epoch)
}

/// Case 25: with the GUI closed, the listing targets the exact recorded
/// socket, classifies it as stopped, and never starts a server. The scope
/// comes from the registry's recorded endpoint precisely so this is
/// reachable — the verified-descriptor path hard-errors instead.
#[test]
fn a_stopped_wez_service_lists_as_stopped_and_starts_nothing() {
    let scratch = Scratch::new("stopped");
    let socket = scratch.path("wez.sock");
    let (instance, epoch) = register_wez(&scratch, &socket);
    let mut registry = scratch.registry();
    let reservation = registry
        .reserve_space("dotfiles", instance, Uuid::new_v4())
        .unwrap();
    registry
        .finalize_create(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: "dmux:ws:dotfiles".into(),
                native_kind: NativeKind::WezWorkspaceKey,
                server_epoch: Some(epoch),
            },
        )
        .unwrap();
    drop(registry);
    assert!(!Path::new(&socket).exists(), "the service is down");

    let source = scratch.source(scratch.stub_wezterm("[]"));
    let out = ls_cli::render(&source, None, &LsArgs::default());
    assert_eq!(out.status, ExitStatus::Success);
    let row = out
        .stdout
        .lines()
        .find(|line| line.contains("dotfiles"))
        .unwrap_or_else(|| panic!("no dotfiles row in {}", out.stdout));
    assert!(row.contains("stopped"), "{row:?}");
    assert!(
        !scratch.ran_wezterm(),
        "a listing must never start a stopped server"
    );
}

/// Case 13's listing half: an external workspace is a separate row with no
/// fabricated identity. `ls` reports it and changes nothing about it.
#[test]
fn an_external_workspace_stays_unmanaged() {
    let scratch = Scratch::new("external");
    let socket = scratch.path("wez.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    register_wez(&scratch, &socket);
    let stub = scratch.stub_wezterm(&format!(
        "[{},{}]",
        wez_pane(0, 0, 0, &format!("dmux:system:{EPOCH}")),
        wez_pane(1, 1, 1, "scratchpad")
    ));

    let source = scratch.source(stub);
    let out = ls_cli::render(&source, None, &LsArgs::default());
    assert_eq!(out.status, ExitStatus::Success);
    assert!(scratch.ran_wezterm(), "a live socket is scanned");
    let row = out
        .stdout
        .lines()
        .find(|line| line.contains("scratchpad"))
        .unwrap_or_else(|| panic!("no scratchpad row in {}", out.stdout));
    assert!(row.contains("unmanaged"), "{row:?}");
    assert!(row.starts_with('-'), "no fabricated ref: {row:?}");
    assert!(
        !out.stdout.contains("dmux:system:"),
        "the sentinel is never a user row: {}",
        out.stdout
    );

    let doc: serde_json::Value = serde_json::from_str(
        &ls_cli::render(&source, Some(OutputFormat::Json), &LsArgs::default()).stdout,
    )
    .unwrap();
    let external = &doc["result"][0];
    assert_eq!(external["managed"], false);
    assert_eq!(
        external["native_ref"],
        output::native_ref(Backend::Wez, "scratchpad")
    );
    assert!(external.get("space_no").is_none());
}

/// Case 25, and §16.2's definition of a partial result: the live server
/// publishes a different sentinel epoch than the registry recorded, so the
/// scan is rejected and its rows are discarded. Every Space on that backend
/// is now an unverified observation, which a JSON consumer has to be able to
/// see — typed `errors[]` and exit 7, never a clean listing.
#[test]
fn a_rejected_scan_is_a_partial_listing_not_a_clean_one() {
    let scratch = Scratch::new("rejected");
    let socket = scratch.path("wez.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    let (instance, epoch) = register_wez(&scratch, &socket);
    let mut registry = scratch.registry();
    let reservation = registry
        .reserve_space("dotfiles", instance, Uuid::new_v4())
        .unwrap();
    registry
        .finalize_create(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: "dmux:ws:dotfiles".into(),
                native_kind: NativeKind::WezWorkspaceKey,
                server_epoch: Some(epoch),
            },
        )
        .unwrap();
    drop(registry);

    // A replacement server: same socket path, a sentinel from another epoch.
    let replacement = "0f0f0f0f-1111-4222-8333-444444444444";
    let stub = scratch.stub_wezterm(&format!(
        "[{},{}]",
        wez_pane(0, 0, 0, &format!("dmux:system:{replacement}")),
        wez_pane(1, 1, 1, "dmux:ws:dotfiles")
    ));
    let source = scratch.source(stub);

    let out = ls_cli::render(&source, Some(OutputFormat::Json), &LsArgs::default());
    assert_eq!(out.status, ExitStatus::Partial);
    let doc: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("wez inventory is indeterminate"),
        "{}",
        doc["errors"][0]
    );
    assert!(
        doc["errors"][0]["target"].is_string(),
        "the host it is about"
    );
    assert_eq!(
        doc["result"][0]["observation"], "unreachable",
        "the rejected server's rows are discarded, not believed"
    );
    assert!(
        out.stderr
            .iter()
            .any(|line| line.contains("wez inventory is indeterminate")),
        "the operator still sees it: {:?}",
        out.stderr
    );
}

/// Case 25's wrong-server half, through the gap a NULL epoch opens: the
/// instance is registered and addressable, but its server incarnation was
/// never published, so there is no epoch to pin the scan to. An unpinned
/// scope makes the adapter skip verification entirely, and the server that
/// answers — here a replacement publishing another sentinel — is then
/// believed *complete*, which demotes the live Space to `absent` and exits
/// 0. The registered instance must refuse before it is probed instead.
#[test]
fn a_managed_instance_without_a_published_epoch_refuses_to_scan() {
    let scratch = Scratch::new("unpublished");
    let socket = scratch.path("wez.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    let mut registry = scratch.registry();
    // Exactly what `dmux-mux-start.sh` leaves behind when it registers the
    // instance and coordination never publishes an incarnation.
    let instance = registry
        .register_backend_instance(Backend::Wez, Some(&socket), None)
        .unwrap();
    let reservation = registry
        .reserve_space("dotfiles", instance, Uuid::new_v4())
        .unwrap();
    registry
        .finalize_create(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: "dmux:ws:dotfiles".into(),
                native_kind: NativeKind::WezWorkspaceKey,
                server_epoch: None,
            },
        )
        .unwrap();
    assert!(
        registry
            .backend_server(instance)
            .unwrap()
            .server_epoch
            .is_none(),
        "the fixture is only meaningful with server_epoch NULL"
    );
    drop(registry);

    let stub = scratch.stub_wezterm(&format!(
        "[{},{}]",
        wez_pane(0, 0, 0, "dmux:system:0f0f0f0f-1111-4222-8333-444444444444"),
        wez_pane(1, 1, 1, "dmux:ws:someone-elses")
    ));
    let source = scratch.source(stub);

    let out = ls_cli::render(&source, Some(OutputFormat::Json), &LsArgs::default());
    assert_eq!(out.status, ExitStatus::Partial, "{}", out.stdout);
    let doc: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed");
    let message = doc["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("published no server epoch"),
        "the operator is told what is missing: {message}"
    );
    // Nothing holds the instance and no recovery lease exists, so this is
    // state C — and the advice is to wait for coordination, never to
    // restart a service that may be mid-bootstrap (ADR 012 WS-B.2).
    assert!(
        message.contains("instance state C") && message.contains("`dmux doctor`"),
        "and what to do about it: {message}"
    );
    assert!(
        !message.to_lowercase().contains("restart"),
        "the destructive advice is gone: {message}"
    );
    let dotfiles = doc["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "dotfiles")
        .unwrap_or_else(|| panic!("no dotfiles row in {}", out.stdout));
    assert_eq!(
        dotfiles["observation"], "unreachable",
        "an unverified server proves nothing about this Space"
    );
    assert!(
        !out.stdout.contains("someone-elses"),
        "no unverified server's rows are published: {}",
        out.stdout
    );
    assert!(
        !scratch.ran_wezterm(),
        "a managed endpoint with nothing to verify against is refused before it is probed"
    );

    // The human surface makes the same call: exit 7 and the typed detail,
    // never a clean table that quietly says `absent`.
    let human = ls_cli::render(&source, None, &LsArgs::default());
    assert_eq!(human.status, ExitStatus::Partial);
    assert!(
        human
            .stderr
            .iter()
            .any(|line| line.contains("published no server epoch")),
        "{:?}",
        human.stderr
    );
    let row = human
        .stdout
        .lines()
        .find(|line| line.contains("dotfiles"))
        .unwrap_or_else(|| panic!("no dotfiles row in {}", human.stdout));
    assert!(row.contains("unreachable"), "{row:?}");
}

/// Review finding #19's A/B, inverted (ADR 012 WS-B.2; report 04 rows C/D):
/// the same instance under the identical held exclusive lock, changing only
/// whether an epoch is published, produces two different remedies — and the
/// unpublished one is "a coordinator is in flight; wait", never "restart the
/// managed mux service", because the exclusive holder between registering
/// and publishing IS the first bootstrap a restart would destroy. Free, the
/// same instance is state C with its own advice; a recovery lease a killed
/// coordinator left behind is state D again. Nothing is probed in any of
/// the unpublished cases.
#[test]
fn an_unpublished_instance_tells_a_coordinator_in_flight_from_an_idle_one_and_never_says_restart() {
    use dmux::locks::{self, LockMode, LockScope};
    use dmux::registry::{LeaseHolder, LeaseScope};
    use std::time::Duration;

    let scratch = Scratch::new("cd");
    let socket = scratch.path("wez.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    let instance = scratch
        .registry()
        .register_backend_instance(Backend::Wez, Some(&socket), None)
        .unwrap();
    let source = scratch.source(scratch.stub_wezterm("[]"));
    let message = |out: &ls_cli::LsOutput| -> String {
        assert_eq!(out.status, ExitStatus::Partial, "{}", out.stdout);
        let doc: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed", "{doc}");
        doc["errors"][0]["message"].as_str().unwrap().to_string()
    };
    let render = || ls_cli::render(&source, Some(OutputFormat::Json), &LsArgs::default());

    // Free and unpublished: state C.
    let idle = message(&render());
    assert!(idle.contains("instance state C"), "{idle}");
    assert!(
        idle.contains("wait for the managed mux to coordinate"),
        "{idle}"
    );
    assert!(!idle.to_lowercase().contains("restart"), "{idle}");

    // The identical exclusive lock a coordinator holds between registering
    // and publishing: state D, wait and re-run.
    let held = locks::acquire(
        &scratch.env().lock_dir,
        LockScope::BackendInstance(instance),
        LockMode::Exclusive,
    )
    .unwrap();
    let in_flight = message(&render());
    assert!(in_flight.contains("instance state D"), "{in_flight}");
    assert!(
        in_flight.contains("held exclusively") && in_flight.contains("re-run `dmux ls`"),
        "{in_flight}"
    );
    assert!(!in_flight.to_lowercase().contains("restart"), "{in_flight}");
    assert_ne!(idle, in_flight, "C and D are two remedies, not one");

    // The report's A/B: the same lock, now with an epoch published (by a
    // live incarnation — this process). The published arm has always said
    // what it is; the unpublished arm now does too.
    let epoch = ServerEpoch(Uuid::new_v4());
    scratch
        .registry()
        .publish_backend_server(
            instance,
            epoch,
            Some(i64::from(std::process::id())),
            Some(&dmux::runtime::process_start_token_for_pid(std::process::id()).unwrap()),
            None,
            None,
        )
        .unwrap();
    let out = render();
    assert_eq!(out.status, ExitStatus::Partial, "{}", out.stdout);
    let doc: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("recovering or mutating"),
        "{doc}"
    );
    assert!(
        !scratch.ran_wezterm(),
        "nothing is probed under a held fence"
    );

    // A recovery lease a coordinator took and never released (it was
    // killed; the kernel lock died with it): state D on the lease alone.
    let mut registry = scratch.registry();
    registry.retire_backend_server(instance, epoch).unwrap();
    registry
        .acquire_lease(
            &LeaseScope::Recovery(instance),
            &LeaseHolder::current(Uuid::new_v4()),
            Duration::from_secs(60),
            &held,
            None,
        )
        .unwrap();
    drop(held);
    drop(registry);
    let orphaned = message(&render());
    assert!(orphaned.contains("instance state D"), "{orphaned}");
    assert!(
        orphaned.contains(&format!(
            "a recovery lease is held by pid {} (alive)",
            std::process::id()
        )),
        "{orphaned}"
    );
    assert!(!orphaned.contains("held exclusively"), "{orphaned}");
    assert!(!scratch.ran_wezterm());
}

/// "No instance is registered" is not a failed scan — nothing was probed.
/// A machine that runs no mux is a normal state, so it costs no error, no
/// remark, and no exit 7 (legacy `list.rs` takes the same position).
#[test]
fn an_unregistered_backend_is_not_a_failed_scan() {
    let scratch = Scratch::new("unregistered");
    let source = scratch.source(scratch.stub_wezterm("[]"));
    let out = ls_cli::render(&source, Some(OutputFormat::Json), &LsArgs::default());
    assert_eq!(out.status, ExitStatus::Success);
    let doc: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["errors"], serde_json::json!([]));
    assert_eq!(doc["result"], serde_json::json!([]));
    assert!(
        !out.stderr.iter().any(|line| line.contains("indeterminate")),
        "nothing was probed, so there is nothing to report: {:?}",
        out.stderr
    );
    assert!(
        !scratch.ran_wezterm(),
        "ADR 006: an unregistered mux has no address to guess"
    );
}

/// The other half of the same rule: a registered instance the registry
/// cannot address is not "nothing was probed" — it is a managed backend
/// this process failed to establish anything about, so it is typed and
/// exit 7 like any other rejected scan.
#[test]
fn a_registered_instance_without_an_endpoint_is_an_error() {
    let scratch = Scratch::new("unaddressable");
    let mut registry = scratch.registry();
    registry
        .register_backend_instance(Backend::Wez, None, None)
        .unwrap();
    drop(registry);

    let source = scratch.source(scratch.stub_wezterm("[]"));
    let out = ls_cli::render(&source, Some(OutputFormat::Json), &LsArgs::default());
    assert_eq!(out.status, ExitStatus::Partial);
    let doc: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(doc["errors"][0]["code"], "provider_unavailable");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("no recorded endpoint"),
        "{}",
        doc["errors"][0]
    );
    assert!(!scratch.ran_wezterm());
}

/// Case 13's listing half on an unmigrated machine: the registry knows no
/// tmux instance yet, and the sessions the user already has still have to be
/// listed — as `unmanaged`, with no fabricated ref (§16.1, case 27). Legacy
/// `ls` showed them; the Wez-first listing must not be blinder.
#[test]
fn a_live_native_tmux_session_is_listed_when_nothing_is_registered() {
    let scratch = Scratch::new("native");
    let started = scratch.tmux(&["-f", "/dev/null", "new-session", "-d", "-s", "seed"]);
    assert!(
        started.as_ref().is_ok_and(|out| out.status.success()),
        "{started:?}"
    );

    let source = scratch.source(scratch.stub_wezterm("[]"));
    let out = ls_cli::render(&source, None, &LsArgs::default());
    assert_eq!(out.status, ExitStatus::Success);
    let row = out
        .stdout
        .lines()
        .find(|line| line.contains("seed"))
        .unwrap_or_else(|| panic!("no seed row in {}", out.stdout));
    assert!(row.starts_with('-'), "no fabricated ref: {row:?}");
    assert!(row.contains("unmanaged"), "{row:?}");
    assert!(!scratch.ran_wezterm());
}

/// A registered tmux instance is scanned under the epoch `tmux_bootstrap`
/// published for it — the production path on a migrated machine, which no
/// `ls` test registered before (review report 02's gap; ADR 012 WS-A.13
/// follow-up). The Space bound to the live session is `live`, the live
/// server's session is not a second, unmanaged row, and wezterm is never
/// run for a backend nothing is registered for.
#[test]
fn a_registered_tmux_instance_is_scanned_under_its_published_epoch() {
    let scratch = Scratch::new("registered");
    let started = scratch.tmux(&["-f", "/dev/null", "new-session", "-d", "-s", "proj"]);
    assert!(
        started.as_ref().is_ok_and(|out| out.status.success()),
        "{started:?}"
    );
    let epoch = match tmux_bootstrap(&scratch.env(), &scratch.namespace).unwrap() {
        TmuxBootstrapOutcome::Bootstrapped { epoch } => epoch,
        other => panic!("a fresh server bootstraps: {other:?}"),
    };
    let session = String::from_utf8(
        scratch
            .tmux(&["display-message", "-p", "-t", "proj", "#{session_id}"])
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let mut registry = scratch.registry();
    let instance = registry
        .backend_instance_for_backend(Backend::Tmux)
        .unwrap()
        .expect("tmux_bootstrap registered the instance");
    assert_eq!(
        registry.backend_server(instance).unwrap().server_epoch,
        Some(epoch)
    );
    let reservation = registry
        .reserve_space("proj", instance, Uuid::new_v4())
        .unwrap();
    registry
        .finalize_create(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: session.clone(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: Some(epoch),
            },
        )
        .unwrap();
    registry
        .set_space_health(reservation.space_uid, Health::Healthy)
        .unwrap();
    drop(registry);

    let source = scratch.source(scratch.stub_wezterm("[]"));
    let out = ls_cli::render(&source, Some(OutputFormat::Json), &LsArgs::default());
    assert_eq!(
        out.status,
        ExitStatus::Success,
        "{} {:?}",
        out.stdout,
        out.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["errors"], serde_json::json!([]), "{doc}");
    let rows = doc["result"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "the bound session is one managed row: {doc}");
    assert_eq!(rows[0]["managed"], true, "{doc}");
    assert_eq!(rows[0]["name"], "proj", "{doc}");
    assert_eq!(rows[0]["backend"], "tmux", "{doc}");
    assert_eq!(rows[0]["observation"], "live", "{doc}");
    assert_eq!(rows[0]["backend_instance"], instance.0.to_string(), "{doc}");
    assert!(
        !scratch.ran_wezterm(),
        "no wez instance is registered, so nothing probes wezterm"
    );
}

/// The same machine before the bootstrap hook has run: the tmux instance is
/// registered on the namespace with no published epoch, and a Space is
/// bound to the live server's `$0`. The listing is Partial with the typed
/// epoch fault, the Space is `unreachable`, and the live server's sessions
/// are not discovered — a managed endpoint nothing vouches for is refused,
/// never probed as first contact (case 27; ADR 012 WS-A.4).
#[test]
fn a_registered_tmux_instance_without_a_published_epoch_refuses_and_discovers_nothing() {
    let scratch = Scratch::new("nullepoch");
    let started = scratch.tmux(&["-f", "/dev/null", "new-session", "-d", "-s", "seed"]);
    assert!(
        started.as_ref().is_ok_and(|out| out.status.success()),
        "{started:?}"
    );
    let mut registry = scratch.registry();
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some(&scratch.namespace), None)
        .unwrap();
    assert!(
        registry
            .backend_server(instance)
            .unwrap()
            .server_epoch
            .is_none(),
        "the fixture is only meaningful with server_epoch NULL"
    );
    let reservation = registry
        .reserve_space("seed", instance, Uuid::new_v4())
        .unwrap();
    registry
        .finalize_create(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: "$0".into(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: None,
            },
        )
        .unwrap();
    registry
        .set_space_health(reservation.space_uid, Health::Healthy)
        .unwrap();
    drop(registry);

    let source = scratch.source(scratch.stub_wezterm("[]"));
    let out = ls_cli::render(&source, Some(OutputFormat::Json), &LsArgs::default());
    assert_eq!(out.status, ExitStatus::Partial, "{}", out.stdout);
    let doc: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(doc["ok"], false, "{doc}");
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed", "{doc}");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("has published no server epoch"),
        "{doc}"
    );
    let rows = doc["result"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only the durable row: {doc}");
    assert_eq!(rows[0]["managed"], true, "{doc}");
    assert_eq!(rows[0]["observation"], "unreachable", "{doc}");
    assert!(
        rows.iter().all(|row| row["managed"] == true),
        "the live server's sessions are not discoveries: {doc}"
    );
    assert!(!scratch.ran_wezterm());
}

// ---------------------------------------------------------------------------
// The peer wire mapping

fn space_info(no: u64, name: &str, backend: Backend, bound: bool) -> protocol::SpaceInfo {
    protocol::SpaceInfo {
        space_uid: SpaceUid(Uuid::from_u128(0x2000 + no as u128)),
        space_no: no,
        name: name.into(),
        backend,
        backend_instance_uid: BackendInstanceUid(Uuid::from_u128(3)),
        lifecycle: Lifecycle::Active,
        health: Health::Healthy,
        native_token: bound.then(|| format!("native-{no}")),
    }
}

fn scan(backend: Backend, outcome: &str) -> protocol::ScanSummary {
    protocol::ScanSummary {
        backend,
        outcome: outcome.into(),
        detail: None,
        rows: None,
        server_epoch: None,
    }
}

/// A peer backend whose scan established nothing leaves that peer's rows
/// unverified, so it is a partial listing exactly as a local rejection is —
/// while a determinate `server_stopped` stays a remark and keeps exit 0.
#[test]
fn a_peer_scan_that_established_nothing_is_partial() {
    let indeterminate = ls_cli::peer_listing(
        HostUid(Uuid::from_u128(2)),
        protocol::SpacesInfo {
            spaces: vec![space_info(7, "monitoring", Backend::Wez, true)],
            scans: vec![scan(Backend::Wez, "unreachable")],
        },
        Some("usb".into()),
    );
    assert!(indeterminate.notes.is_empty());
    let out = run(
        &two_hosts(Ok(indeterminate)),
        Some(OutputFormat::Json),
        LsArgs {
            all_hosts: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(out.status, ExitStatus::Partial);
    let doc = json(&out);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["errors"][0]["code"], "provider_unavailable");
    assert_eq!(
        doc["errors"][0]["target"],
        HostUid(Uuid::from_u128(2)).0.to_string()
    );
    assert_eq!(
        doc["result"].as_array().unwrap().len(),
        2,
        "the rest still lists"
    );

    let stopped = ls_cli::peer_listing(
        HostUid(Uuid::from_u128(2)),
        protocol::SpacesInfo {
            spaces: vec![space_info(7, "monitoring", Backend::Wez, true)],
            scans: vec![scan(Backend::Wez, "server_stopped")],
        },
        Some("usb".into()),
    );
    assert!(stopped.errors.is_empty(), "a stopped server is an answer");
    let out = run(
        &two_hosts(Ok(stopped)),
        None,
        LsArgs {
            all_hosts: true,
            ..LsArgs::default()
        },
    );
    assert_eq!(out.status, ExitStatus::Success);
}

/// `spaces` carries durable rows plus one summary per backend and nothing
/// per row, so liveness is only claimed where the owner's own record and
/// its scan both support it, and a stopped peer backend says `stopped`.
#[test]
fn a_peer_answer_becomes_rows_without_inventing_liveness() {
    let listing = ls_cli::peer_listing(
        HostUid(Uuid::from_u128(2)),
        protocol::SpacesInfo {
            spaces: vec![
                space_info(1, "bound", Backend::Wez, true),
                space_info(2, "unbound", Backend::Wez, false),
                space_info(3, "stopped-backend", Backend::Tmux, true),
                protocol::SpaceInfo {
                    lifecycle: Lifecycle::Deleted,
                    ..space_info(4, "gone", Backend::Wez, true)
                },
            ],
            scans: vec![
                scan(Backend::Wez, "complete"),
                scan(Backend::Tmux, "server_stopped"),
            ],
        },
        Some("usb".into()),
    );
    assert!(!listing.counts);
    assert_eq!(listing.route.as_deref(), Some("usb"));
    let observations: Vec<(String, Observation)> = listing
        .rows
        .iter()
        .map(|row| match row {
            ReconRow::Managed(managed) => (managed.space.logical_name.clone(), managed.observation),
            ReconRow::Unmanaged(_) => panic!("a peer never reports unmanaged rows"),
        })
        .collect();
    assert_eq!(
        observations,
        vec![
            ("bound".to_string(), Observation::Live),
            ("unbound".to_string(), Observation::Absent),
            ("stopped-backend".to_string(), Observation::Stopped),
        ],
        "terminal rows are dropped and liveness is never assumed"
    );
    assert!(
        listing
            .notes
            .iter()
            .any(|note| note.contains("server_stopped")),
        "{:?}",
        listing.notes
    );
}
