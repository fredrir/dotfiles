//! The binding the agent hands its adapters carries the registry's recorded
//! binding epoch through `operations::binding_epoch_for_adapter` (ADR 012
//! WS-A.8; O's close handed the remote `rename`/`inspect`/`attach_plan`
//! paths on), never the verified target's pin copied across. A tmux binding
//! recorded under another incarnation — the server restarted and published
//! a fresh epoch, the session id possibly recycled — is refused typed before
//! any native command, exactly as the local verbs refuse it.

use std::process::Command;
use std::time::Duration;

use dmux::model::Backend;
use dmux::operations::CreatedSpace;
use dmux::remote::protocol::{self};
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

fn assert_stale_binding_refusal(code: i32, response: &protocol::Envelope, what: &str) {
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
        message.contains("was recorded under server epoch")
            && message.contains("dmux repair rebind"),
        "{what}: the registry's recorded binding epoch must be the refusal: {message}"
    );
    assert!(
        response.payload.is_none(),
        "{what}: a refusal carries no payload: {response:?}"
    );
}

/// Restart the scratch server with the Space's session name recycled and
/// publish the fresh incarnation through the real bootstrap: the registry
/// now publishes the new epoch while the binding stays recorded under the
/// old one.
fn restart_and_republish(scratch: &Scratch, session: &str) {
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
            scratch.ns.as_deref().unwrap(),
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            session,
        ])
        .env("DMUX_RUNTIME_DIR", scratch.locks.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "replacement scratch tmux server: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    scratch.bootstrap_tmux();
}

#[test]
fn a_tmux_binding_recorded_under_another_incarnation_is_refused_over_rename_and_attach_plan() {
    let scratch = Scratch::with_tmux("binding-epoch");
    let created = create(&scratch, "proj");
    let registry = scratch.registry();
    let instance = registry
        .backend_instance_for_backend(Backend::Tmux)
        .unwrap()
        .expect("bootstrapped tmux instance");
    let old_epoch = registry
        .backend_server(instance)
        .unwrap()
        .server_epoch
        .expect("bootstrap published an epoch");
    assert_eq!(
        registry.current_binding_epoch(created.space_uid).unwrap(),
        Some(old_epoch),
        "the create recorded its binding under the incarnation it ran on"
    );
    drop(registry);

    restart_and_republish(&scratch, "proj");
    let registry = scratch.registry();
    let new_epoch = registry
        .backend_server(instance)
        .unwrap()
        .server_epoch
        .expect("re-bootstrap published the fresh incarnation");
    assert_ne!(new_epoch, old_epoch, "a restart is a new incarnation");
    assert_eq!(
        registry.current_binding_epoch(created.space_uid).unwrap(),
        Some(old_epoch),
        "the binding is still recorded under the old incarnation"
    );
    drop(registry);
    let sessions_before = scratch.session_names();
    assert_eq!(sessions_before, vec!["proj".to_string()]);

    // The ordinary `rename` arm (operations::rename_space, WS-A.8 already).
    let rename = envelope(
        protocol::methods::RENAME,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "new_name": "renamed" }),
    );
    let (code, response) = scratch.agent(&rename);
    assert_stale_binding_refusal(code, &response, "rename");

    // The agent's own resume arm: an unfinished rename journaled under this
    // request UID makes the handler repeat the native rename itself. It used
    // to hand the adapter the verified target's epoch — the pin copied
    // across — so the stale binding was never seen and `$N` on the new
    // server was renamed on the old row's word. Now the registry's recorded
    // epoch refuses it the same way, before any tmux command, and the
    // unfinished operation is left for `repair rebind` to settle.
    let request_uid = Uuid::new_v4();
    let operation_uid = scratch
        .registry()
        .begin_rename(created.space_uid, "resumed", request_uid)
        .unwrap();
    let resume = envelope(
        protocol::methods::RENAME,
        request_uid,
        json!({ "space_uid": created.space_uid, "new_name": "resumed" }),
    );
    let (code, response) = scratch.agent(&resume);
    assert_stale_binding_refusal(code, &response, "rename (resume)");
    let registry = scratch.registry();
    let unfinished = registry
        .unfinished_operation(created.space_uid)
        .unwrap()
        .expect("the unfinished rename is left for repair, not committed");
    assert_eq!(unfinished.operation_uid, operation_uid);
    assert_eq!(
        registry.space(created.space_uid).unwrap().logical_name,
        "proj",
        "the registry row was renamed on a stale binding's word"
    );
    drop(registry);

    // `attach_plan`: the binding is refused before the adapter inspects it
    // and before any token is minted.
    let plan = envelope(
        protocol::methods::ATTACH_PLAN,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "route": "test-direct" }),
    );
    let (code, response) = scratch.agent(&plan);
    assert_stale_binding_refusal(code, &response, "attach_plan");

    assert_eq!(
        scratch.session_names(),
        sessions_before,
        "a native command reached the new incarnation"
    );
}
