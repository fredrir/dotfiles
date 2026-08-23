//! The agent's tmux verification matrix (`remote/agent.rs`
//! `verified_tmux_target`) compares the socket witnesses `tmux_bootstrap`
//! published — pid, start token, socket dev/ino — against a fresh probe of
//! the live server before it trusts the server's self-reported epoch
//! (ADR 012 WS-A.9 at the remote-side readers, O's and D3's close;
//! acceptance case 27). The model is
//! `operations_flow::a_replaced_tmux_socket_presenting_the_old_epoch_is_refused_everywhere`,
//! driven here through the real `_agent` binary on a scratch `-L`
//! namespace, the way a peer reaches it.

use std::process::Command;
use std::time::Duration;

use dmux::model::Backend;
use dmux::operations::CreatedSpace;
use dmux::remote::protocol::{self, SpacesInfo};
use serde_json::json;
use uuid::Uuid;

use crate::util::{Scratch, envelope, error_code, wait_for};

fn create(scratch: &Scratch, name: &str) -> CreatedSpace {
    let request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        json!({ "name": name, "backend": "tmux", "program": ["sleep", "300"] }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    serde_json::from_value(response.payload.unwrap()).unwrap()
}

fn live_pid(scratch: &Scratch) -> String {
    String::from_utf8_lossy(&scratch.tmux(&["display-message", "-p", "#{pid}"]).stdout)
        .trim()
        .to_string()
}

/// The refusal every mutation and plan must give once the server behind
/// the published row is gone: the typed epoch fault, naming the stale
/// incarnation and its remedy, raised by the witness comparison — not by
/// `verify_epoch`'s pid check, which would name the same fault
/// `wrong_backend_instance`.
fn assert_stale_refusal(code: i32, response: &protocol::Envelope, what: &str) {
    assert_eq!(
        code, 1,
        "{what}: backend_epoch_changed exits 1: {response:?}"
    );
    assert_eq!(
        error_code(response),
        "backend_epoch_changed",
        "{what}: {response:?}"
    );
    let message = &response.error.as_ref().unwrap().message;
    assert!(
        message.contains("stale incarnation") && message.contains("ADR 012 §3.1 state F"),
        "{what}: the witness comparison must name the fault: {message}"
    );
    assert!(
        message.contains("repair retire-incarnation"),
        "{what}: the remedy is the operator's retire-then-bootstrap: {message}"
    );
    assert!(
        !message.contains("tmux server incarnation changed"),
        "{what}: verify_epoch's wrong_backend_instance must not have fired first: {message}"
    );
    assert!(
        response.payload.is_none(),
        "{what}: a refusal carries no payload: {response:?}"
    );
}

/// A replaced server on the same namespace — same socket path, new inode,
/// new pid — that presents the OLD `@dmux_server_epoch` is what the epoch
/// option alone cannot tell apart. Every agent path that builds a tmux
/// target from the registry row now refuses it as a stale incarnation
/// before any native mutation: `rename` renames nothing, `attach_plan`
/// mints no token, `new` creates nothing on the impostor, and `spaces`
/// reports the instance `unreachable` rather than listing the stranger's
/// sessions. The same calls on the published incarnation are the positive
/// control.
#[test]
fn a_replaced_tmux_socket_presenting_the_old_epoch_is_refused_by_every_agent_path() {
    let scratch = Scratch::with_tmux("agent-replaced");
    let ns = scratch.ns.clone().unwrap();
    let created = create(&scratch, "proj");
    let registry = scratch.registry();
    let instance = registry
        .backend_instance_for_backend(Backend::Tmux)
        .unwrap()
        .expect("bootstrapped tmux instance");
    let published = registry.backend_server(instance).unwrap();
    let epoch = published
        .server_epoch
        .expect("bootstrap published an epoch");
    assert!(
        published.socket_dev.is_some() && published.socket_ino.is_some(),
        "tmux_bootstrap publishes the socket witnesses the agent compares"
    );
    drop(registry);

    // Positive control: the published incarnation serves a plan and a
    // complete scan.
    let plan = envelope(
        protocol::methods::ATTACH_PLAN,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "route": "test-direct" }),
    );
    let (code, response) = scratch.agent(&plan);
    assert_eq!(code, 0, "{response:?}");
    let (code, response) = scratch.agent(&envelope(
        protocol::methods::SPACES,
        Uuid::new_v4(),
        json!({}),
    ));
    assert_eq!(code, 0, "{response:?}");
    let info: SpacesInfo = serde_json::from_value(response.payload.unwrap()).unwrap();
    let scan = info
        .scans
        .iter()
        .find(|scan| scan.backend == Backend::Tmux)
        .expect("tmux scan");
    assert_eq!(scan.outcome, "complete", "{scan:?}");

    // Replace the server: kill it, start another on the same namespace with
    // the session name recycled, then copy the old epoch onto it. Nothing
    // the registry recorded survives but the epoch.
    let old_pid = live_pid(&scratch);
    assert!(scratch.tmux(&["kill-server"]).status.success());
    wait_for(
        "the old scratch tmux server to stop answering",
        Duration::from_secs(10),
        || {
            !scratch
                .tmux(&["display-message", "-p", "#{pid}"])
                .status
                .success()
        },
    );
    let out = Command::new("tmux")
        .args([
            "-L",
            &ns,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "proj",
        ])
        .env("DMUX_RUNTIME_DIR", scratch.locks.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "replacement scratch tmux server: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        scratch
            .tmux(&[
                "set-option",
                "-g",
                "@dmux_server_epoch",
                &epoch.0.to_string()
            ])
            .status
            .success()
    );
    assert_ne!(live_pid(&scratch), old_pid, "different process");
    assert_eq!(
        String::from_utf8_lossy(
            &scratch
                .tmux(&["show-option", "-gv", "@dmux_server_epoch"])
                .stdout
        )
        .trim(),
        epoch.0.to_string(),
        "the impostor presents the old epoch"
    );
    let sessions_before = scratch.session_names();
    assert_eq!(sessions_before, vec!["proj".to_string()]);

    // `rename`: refused by the witness comparison, the session untouched.
    let rename = envelope(
        protocol::methods::RENAME,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "new_name": "renamed" }),
    );
    let (code, response) = scratch.agent(&rename);
    assert_stale_refusal(code, &response, "rename");
    assert_eq!(
        scratch.session_names(),
        sessions_before,
        "rename mutated the impostor"
    );
    assert_eq!(
        scratch
            .registry()
            .space(created.space_uid)
            .unwrap()
            .logical_name,
        "proj",
        "the registry row was renamed on an unverified server's word"
    );

    // `attach_plan`: no token minted for a server nothing verified.
    let plan = envelope(
        protocol::methods::ATTACH_PLAN,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "route": "test-direct" }),
    );
    let (code, response) = scratch.agent(&plan);
    assert_stale_refusal(code, &response, "attach_plan");

    // `new`: nothing created on the impostor.
    let fresh = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        json!({ "name": "on-a-stranger", "backend": "tmux", "program": ["sleep", "300"] }),
    );
    let (code, response) = scratch.agent(&fresh);
    assert_stale_refusal(code, &response, "new");
    assert_eq!(
        scratch.session_names(),
        sessions_before,
        "new created on the impostor"
    );
    assert_eq!(
        scratch.registry().spaces().unwrap().len(),
        1,
        "a Space row was minted on an unverified server's word"
    );

    // `spaces`: the resolver's own liveness verdict (WS-B.1) — the instance
    // is `unreachable` with the stale detail and the stranger's session is
    // not a row.
    let (code, response) = scratch.agent(&envelope(
        protocol::methods::SPACES,
        Uuid::new_v4(),
        json!({}),
    ));
    assert_eq!(code, 0, "{response:?}");
    let info: SpacesInfo = serde_json::from_value(response.payload.unwrap()).unwrap();
    let scan = info
        .scans
        .iter()
        .find(|scan| scan.backend == Backend::Tmux)
        .expect("tmux scan");
    assert_eq!(scan.outcome, "unreachable", "{scan:?}");
    assert!(
        scan.detail
            .as_deref()
            .unwrap_or("")
            .contains("stale_incarnation"),
        "{scan:?}"
    );
    assert_eq!(scan.rows, None, "{scan:?}");
}
