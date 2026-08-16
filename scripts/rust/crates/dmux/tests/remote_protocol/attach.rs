//! The `_attach` single-use-token PTY channel, locally against the real
//! binary: a real client attach through a `script(1)` PTY (verified via
//! `tmux list-clients`), token replay rejection, expiry rejection, and
//! server-restart (new epoch) rejection. The exec'd argv comes only from
//! the redeemed record — the tests never hand `_attach` a target.

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use dmux::model::{ChildKind, ProviderHandle, ServerEpoch};
use dmux::operations::CreatedSpace;
use dmux::refs::parse_ref;
use dmux::registry::{AttachTokenSpec, sha256::sha256_hex};
use dmux::remote::protocol::{self, AttachChildRequest, AttachPlan, AttachPlanChild};
use serde_json::json;
use uuid::Uuid;

use crate::util::{DMUX_BIN, Scratch, envelope, wait_for};

fn create_space(scratch: &Scratch, name: &str) -> CreatedSpace {
    let request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        json!({ "name": name, "backend": "tmux", "program": ["sleep", "300"] }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    serde_json::from_value(response.payload.unwrap()).unwrap()
}

fn attach_plan(scratch: &Scratch, created: &CreatedSpace) -> (i32, AttachPlan) {
    let request = envelope(
        protocol::methods::ATTACH_PLAN,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "route": "test-direct" }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    (
        code,
        serde_json::from_value(response.payload.unwrap()).unwrap(),
    )
}

fn attach_args(scratch: &Scratch, token: &str) -> Vec<String> {
    vec![
        "_attach".to_string(),
        "--token".to_string(),
        token.to_string(),
        "--data-dir".to_string(),
        scratch.data.path().display().to_string(),
        "--lock-dir".to_string(),
        scratch.locks.path().display().to_string(),
    ]
}

/// Run `_attach` WITHOUT a PTY — every refusal path exits before exec and
/// needs no terminal.
fn attach_refused(scratch: &Scratch, token: &str) -> Output {
    Command::new(DMUX_BIN)
        .args(attach_args(scratch, token))
        .env_remove("TMUX")
        .output()
        .unwrap()
}

/// Wrap `_attach` in a `script(1)` PTY so the exec'd tmux attach-session
/// has a controlling terminal. stdin stays open (held pipe) so script does
/// not tear the session down on EOF.
fn pty_attach(scratch: &Scratch, token: &str) -> std::process::Child {
    let mut argv: Vec<String> = Vec::new();
    if cfg!(target_os = "macos") {
        argv.extend(["-q".into(), "/dev/null".into(), DMUX_BIN.to_string()]);
        argv.extend(attach_args(scratch, token));
    } else {
        // util-linux script: -c takes one command string; scratch paths
        // contain no shell metacharacters.
        let command = std::iter::once(DMUX_BIN.to_string())
            .chain(attach_args(scratch, token))
            .collect::<Vec<_>>()
            .join(" ");
        argv.extend(["-qec".into(), command, "/dev/null".into()]);
    }
    Command::new("script")
        .args(&argv)
        .env_remove("TMUX")
        .env("TERM", "xterm-256color")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn client_count(scratch: &Scratch) -> usize {
    String::from_utf8_lossy(
        &scratch
            .tmux(&["list-clients", "-F", "#{client_tty}"])
            .stdout,
    )
    .lines()
    .filter(|l| !l.trim().is_empty())
    .count()
}

#[test]
fn plan_token_attaches_once_and_replay_is_refused() {
    let scratch = Scratch::with_tmux("attach");
    let created = create_space(&scratch, "attachee");
    let (_, plan) = attach_plan(&scratch, &created);
    assert_eq!(plan.token.len(), 64, "uuid4+uuid4 simple hex");
    assert!(!plan.replayed);
    assert_eq!(plan.route, "test-direct");
    assert_eq!(plan.space_uid, created.space_uid);

    assert_eq!(client_count(&scratch), 0);
    let mut child = pty_attach(&scratch, &plan.token);
    wait_for("a live tmux client", Duration::from_secs(10), || {
        client_count(&scratch) > 0
    });

    // Single use: the same token again is a refused replay, attached or not.
    let out = attach_refused(&scratch, &plan.token);
    assert_eq!(out.status.code(), Some(6));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already redeemed"), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "one stderr line: {stderr}");

    // Detach the client (each one by its tty — bare `-a` needs a `-t`)
    // and close the held stdin pipe so `script` sees EOF; then reap the
    // wrapper (killing it as a last resort — the attach and replay
    // assertions above are what this test proves).
    let ttys = String::from_utf8_lossy(
        &scratch
            .tmux(&["list-clients", "-F", "#{client_tty}"])
            .stdout,
    )
    .lines()
    .map(str::to_string)
    .collect::<Vec<_>>();
    for tty in &ttys {
        assert!(scratch.tmux(&["detach-client", "-t", tty]).status.success());
    }
    wait_for("the tmux client to detach", Duration::from_secs(10), || {
        client_count(&scratch) == 0
    });
    drop(child.stdin.take());
    let reaped = std::time::Instant::now() + Duration::from_secs(5);
    while !matches!(child.try_wait(), Ok(Some(_))) {
        if std::time::Instant::now() > reaped {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn attach_plan_replay_returns_the_stored_plan_without_the_token() {
    let scratch = Scratch::with_tmux("plan-replay");
    let created = create_space(&scratch, "planned");
    let request = envelope(
        protocol::methods::ATTACH_PLAN,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let plan: AttachPlan = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(!plan.token.is_empty() && !plan.replayed);

    // The raw token is returned exactly once: the ledger replay carries an
    // empty token (only the sha256 was ever persisted).
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let replayed: AttachPlan = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.replayed);
    assert!(replayed.token.is_empty());
    assert_eq!(replayed.space_uid, plan.space_uid);
    assert_eq!(replayed.server_epoch, plan.server_epoch);
}

#[test]
fn child_attach_plan_correlates_parent_and_stores_focus_before_final_attach() {
    let scratch = Scratch::with_tmux("plan-child");
    let created = create_space(&scratch, "focused");
    let group = parse_ref(&format!("x/{}", created.group_ref))
        .unwrap()
        .child
        .unwrap();
    let split = parse_ref(&format!("x/{}", created.split_ref))
        .unwrap()
        .child
        .unwrap();
    let request_uid = Uuid::new_v4();
    let request = envelope(
        protocol::methods::ATTACH_PLAN,
        request_uid,
        json!({
            "space_uid": created.space_uid,
            "route": "test-direct",
            "child": AttachChildRequest {
                kind: ChildKind::Split,
                epoch: split.epoch,
                handle: split.handle.clone(),
            },
        }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let plan: AttachPlan = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(
        plan.child,
        Some(AttachPlanChild::Split {
            epoch: split.epoch,
            group: group.handle.clone(),
            split: split.handle.clone(),
        })
    );

    let ProviderHandle::Tx(group_id) = group.handle else {
        panic!("scratch tmux Group must use tx handle")
    };
    let ProviderHandle::Tx(split_id) = split.handle else {
        panic!("scratch tmux Split must use tx handle")
    };
    let registry = scratch.registry();
    let (native_token, argv_json): (String, String) = registry
        .raw_connection()
        .query_row(
            "SELECT s.native_token, a.attach_argv \
             FROM attach_tokens a \
             JOIN native_bindings s ON s.space_uid = a.space_uid \
               AND s.binding_state = 'current' \
             WHERE a.request_uid = ?1",
            [request_uid.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let argv: Vec<String> = serde_json::from_str(&argv_json).unwrap();
    assert_eq!(
        argv,
        vec![
            "tmux",
            "-L",
            scratch.ns.as_deref().unwrap(),
            "select-window",
            "-t",
            &format!("{native_token}:@{group_id}"),
            ";",
            "select-pane",
            "-t",
            &format!("%{split_id}"),
            ";",
            "attach-session",
            "-t",
            &native_token,
        ]
    );
    assert_eq!(argv.last(), Some(&native_token));
}

#[test]
fn stale_or_missing_attach_child_mints_no_token() {
    let scratch = Scratch::with_tmux("plan-child-refusal");
    let created = create_space(&scratch, "focused");
    let split = parse_ref(&format!("x/{}", created.split_ref))
        .unwrap()
        .child
        .unwrap();
    for child in [
        AttachChildRequest {
            kind: ChildKind::Split,
            epoch: ServerEpoch(Uuid::from_u128(999)),
            handle: split.handle.clone(),
        },
        AttachChildRequest {
            kind: ChildKind::Split,
            epoch: split.epoch,
            handle: ProviderHandle::Tx(u64::MAX),
        },
    ] {
        let request_uid = Uuid::new_v4();
        let request = envelope(
            protocol::methods::ATTACH_PLAN,
            request_uid,
            json!({
                "space_uid": created.space_uid,
                "child": child,
            }),
        );
        let (code, response) = scratch.agent(&request);
        assert!(code == 1 || code == 3, "{response:?}");
        assert!(response.error.is_some(), "{response:?}");
        let registry = scratch.registry();
        let count: i64 = registry
            .raw_connection()
            .query_row(
                "SELECT COUNT(*) FROM attach_tokens WHERE request_uid = ?1",
                [request_uid.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "refused child must mint no token");
    }
}

#[test]
fn expired_and_unknown_tokens_are_refused() {
    let scratch = Scratch::with_tmux("expiry");
    let created = create_space(&scratch, "expired");
    let mut registry = scratch.registry();
    let identity = registry.identity().unwrap();
    let binding = registry
        .current_binding(created.space_uid)
        .unwrap()
        .unwrap();
    let instance = registry
        .spaces()
        .unwrap()
        .into_iter()
        .find(|s| s.space_uid == created.space_uid)
        .unwrap()
        .backend_instance;
    let epoch = registry
        .backend_server(instance)
        .unwrap()
        .server_epoch
        .unwrap();
    // Issue a token that is ALREADY expired (registry-level seam; the
    // agent's own TTL is 60s and cannot be waited out in a test).
    let token = "e".repeat(64);
    registry
        .issue_attach_token(&AttachTokenSpec {
            token_hash: sha256_hex(token.as_bytes()),
            request_uid: Uuid::new_v4(),
            host_uid: identity.host_uid,
            space_uid: created.space_uid,
            server_epoch: epoch,
            route: "test".into(),
            attach_argv: vec![
                "tmux".into(),
                "-L".into(),
                scratch.ns.clone().unwrap(),
                "attach-session".into(),
                "-t".into(),
                binding.native_token,
            ],
            issued_at: "2026-01-01T00:00:00Z".into(),
            expires_at: "2026-01-01T00:01:00Z".into(),
        })
        .unwrap();
    drop(registry);

    let out = attach_refused(&scratch, &token);
    assert_eq!(out.status.code(), Some(6));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("expired"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = attach_refused(&scratch, "never-issued");
    assert_eq!(out.status.code(), Some(6));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unknown"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_restarted_server_refuses_the_token_epoch() {
    let scratch = Scratch::with_tmux("restart");
    let created = create_space(&scratch, "restartee");
    let (_, plan) = attach_plan(&scratch, &created);

    // Kill and restart the scratch server: a fresh incarnation with a
    // fresh epoch after re-bootstrap.
    assert!(scratch.tmux(&["kill-server"]).status.success());
    let out = Command::new("tmux")
        .args([
            "-L",
            scratch.ns.as_deref().unwrap(),
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "fresh",
        ])
        .env("DMUX_RUNTIME_DIR", scratch.locks.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    scratch.bootstrap_tmux();

    let out = attach_refused(&scratch, &plan.token);
    assert_eq!(
        out.status.code(),
        Some(1),
        "backend_epoch_changed maps to 1"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("epoch"), "{stderr}");
    assert_eq!(client_count(&scratch), 0);
}
