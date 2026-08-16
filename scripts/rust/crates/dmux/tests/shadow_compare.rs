//! P4 gate: old-vs-shadow comparison (plan §18 P4). Root-owned.
//!
//! The legacy binary lists via PATH stubs; the shadow pipeline lists via the
//! provider adapters + read-only reconciliation. Both sides are fed one
//! equivalent dataset:
//!
//! - tmux: a REAL scratch server (`-L`) holding sessions `alpha` (2 windows)
//!   and `beta` (1 window); the legacy stub prints the same names/counts in
//!   the legacy `list-sessions` format.
//! - wez: one workspace `work` (1 tab, 1 pane). The shadow side's canned
//!   list additionally contains the reserved sentinel row, which the strict
//!   adapter must extract as the epoch and EXCLUDE — the one documented,
//!   intentional divergence from what the legacy path would have shown.
//!
//! With an empty registry every native resource must surface exactly once,
//! as an explicit unmanaged row: no session lost, none invented.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::Duration;

use dmux::backend::tmux::TmuxProvider;
use dmux::backend::wez::{
    ProbeOutcome, RunError, RunOutput, WezInvocation, WezProvider, WezRunner,
};
use dmux::backend::{InventoryScope, Provider};
use dmux::inventory::{BackendScans, ReconRow, reconcile};
use dmux::model::Backend;

const NS: &str = "dmux-p4-shadow";

struct TmuxGuard;

impl Drop for TmuxGuard {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", NS, "kill-server"])
            .output();
    }
}

fn tmux(args: &[&str]) {
    let out = Command::new("tmux")
        .args(["-L", NS, "-f", "/dev/null"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "tmux {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct CannedWez {
    json: String,
}

impl WezRunner for CannedWez {
    fn probe(&self, _socket: &str, _pid: Option<u32>) -> ProbeOutcome {
        ProbeOutcome::Connectable
    }
    fn run(&self, invocation: &WezInvocation, _deadline: Duration) -> Result<RunOutput, RunError> {
        assert!(invocation.argv.contains(&"--no-auto-start".to_string()));
        Ok(RunOutput {
            status: 0,
            stdout: self.json.clone().into_bytes(),
            stderr: Vec::new(),
        })
    }
}

const EPOCH: &str = "12345678-9abc-4ef0-8234-56789abcdef0";

fn wez_row(window: u32, tab: u32, pane: u32, workspace: &str) -> String {
    format!(
        r#"{{"window_id":{window},"tab_id":{tab},"pane_id":{pane},"workspace":"{workspace}",
           "size":{{"rows":24,"cols":80,"pixel_width":640,"pixel_height":384,"dpi":0}},
           "title":"sh","cwd":"file:///tmp/","tab_title":"","window_title":"",
           "is_active":false,"is_zoomed":false,"tty_name":"/dev/ttys000"}}"#
    )
}

#[test]
fn legacy_and_shadow_agree_on_the_native_row_set() {
    // --- Legacy side: hermetic PATH stubs, exactly like tests/cli.rs. ---
    let bin = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let stub = |name: &str, script: &str| {
        let path = bin.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    stub(
        "tmux",
        "case \"$1\" in\n\
         list-sessions) printf 'alpha|1700000000|2|1\\nbeta|1700000100|1|0\\n' ;;\n\
         esac",
    );
    stub(
        "wezterm",
        r#"printf '[{"window_id":1,"pane_id":1,"workspace":"work"}]'"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .args(["ls", "--json"])
        .env("PATH", bin.path())
        .env("XDG_STATE_HOME", state.path())
        .env("DMUX_DRY_RUN", "1")
        .env_remove("TMUX")
        .env_remove("WEZTERM_UNIX_SOCKET")
        .env_remove("WEZTERM_PANE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let legacy: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let mut legacy_set: Vec<(String, String, u64)> = legacy
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["kind"].as_str().unwrap().to_string(),
                row["name"].as_str().unwrap().to_string(),
                row["windows"].as_u64().unwrap_or(1),
            )
        })
        .collect();
    legacy_set.sort();

    // --- Shadow side: real tmux scratch server with the same dataset. ---
    let _guard = TmuxGuard;
    tmux(&["new-session", "-d", "-s", "alpha"]);
    tmux(&["new-window", "-t", "alpha"]);
    tmux(&["new-session", "-d", "-s", "beta"]);

    let tmux_provider = TmuxProvider::new(NS);
    let tmux_scope = InventoryScope {
        backend: Backend::Tmux,
        endpoint: NS.into(),
        expected_epoch: None,
    };
    let tmux_outcome = tmux_provider.inventory(&tmux_scope);

    let sentinel = wez_row(0, 0, 0, &format!("dmux:system:{EPOCH}"));
    let work = wez_row(1, 1, 1, "work");
    let wez_provider = WezProvider::with_runner(
        "/opt/homebrew/bin/wezterm",
        "/dev/null",
        CannedWez {
            json: format!("[{sentinel},{work}]"),
        },
    );
    let wez_scope = InventoryScope {
        backend: Backend::Wez,
        endpoint: "/tmp/dmux-p4-fake.sock".into(),
        expected_epoch: None,
    };
    let wez_outcome = wez_provider.inventory(&wez_scope);

    let scans = BackendScans {
        wez: wez_outcome,
        tmux: tmux_outcome,
    };
    assert!(
        scans.both_determinate(),
        "shadow scans must be complete: {scans:?}"
    );

    // Empty registry: everything is an explicit unmanaged row.
    let rows = reconcile(&[], |_| None, &scans);
    let mut shadow_set: Vec<(String, String, u64)> = rows
        .iter()
        .map(|row| match row {
            ReconRow::Unmanaged(u) => (
                u.backend.as_str().to_string(),
                u.native_name.clone(),
                u.groups as u64,
            ),
            ReconRow::Managed(m) => panic!("empty registry produced a managed row: {m:?}"),
        })
        .collect();
    shadow_set.sort();

    // The one documented divergence: shadow consumed the sentinel as the
    // epoch handshake and excluded it from user rows.
    assert!(
        !shadow_set
            .iter()
            .any(|(_, name, _)| name.starts_with("dmux:system:")),
        "sentinel must never appear as a user row: {shadow_set:?}"
    );
    if let dmux::backend::InventoryOutcome::Complete(inv) = &scans.wez {
        assert_eq!(inv.server_epoch.unwrap().0.to_string(), EPOCH);
    }

    // Same native rows, same per-row group counts, nothing lost or invented.
    assert_eq!(shadow_set, legacy_set, "old-vs-shadow row sets diverged");
    assert_eq!(
        shadow_set,
        vec![
            ("tmux".to_string(), "alpha".to_string(), 2),
            ("tmux".to_string(), "beta".to_string(), 1),
            ("wez".to_string(), "work".to_string(), 1),
        ]
    );
}
