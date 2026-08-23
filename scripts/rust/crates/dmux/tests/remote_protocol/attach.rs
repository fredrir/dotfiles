//! The `_attach` single-use-token PTY channel, locally against the real
//! binary: a real client attach through a `script(1)` PTY (verified via
//! `tmux list-clients`), token replay rejection, expiry rejection, and
//! server-restart (new epoch) rejection. The exec'd argv comes only from
//! the redeemed record — the tests never hand `_attach` a target.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use dmux::locks::{LockMode, LockScope, OrderedLocks};
use dmux::model::{ChildKind, ProviderHandle, ServerEpoch};
use dmux::operations::CreatedSpace;
use dmux::refs::parse_ref;
use dmux::registry::{AttachTokenSpec, sha256::sha256_hex};
use dmux::remote::attach::{
    TmuxHookClientClaim, read_client_record, refresh_controller_context_from_tmux_hook,
};
use dmux::remote::protocol::{
    self, AttachChildRequest, AttachPlan, AttachPlanChild, TmuxClientDetachPayload,
    TmuxClientDetachResult, TmuxClientRefreshPayload, TmuxClientRefreshResult,
    TmuxClientStatusPayload, TmuxClientStatusResult, TmuxClientSwitchPayload,
    TmuxClientSwitchResult,
};
use serde_json::json;
use uuid::Uuid;

use crate::util::{DMUX_BIN, Scratch, envelope, error_code, wait_for};

const TMUX_FORMAT_SEPARATOR: &str = "__DMUX_FIELD_7F4A9C2E__";
const TEST_WINDOW_PANE_FORMAT: &str = "#{window_id}__DMUX_FIELD_7F4A9C2E__#{pane_id}";
const TEST_SESSION_WINDOW_PANE_FORMAT: &str =
    "#{session_id}__DMUX_FIELD_7F4A9C2E__#{window_id}__DMUX_FIELD_7F4A9C2E__#{pane_id}";

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
    pty_attach_with_transcript(scratch, token, Path::new("/dev/null"))
}

fn pty_attach_with_transcript(
    scratch: &Scratch,
    token: &str,
    transcript: &Path,
) -> std::process::Child {
    let mut argv: Vec<String> = Vec::new();
    if cfg!(target_os = "macos") {
        argv.extend([
            "-q".into(),
            transcript.display().to_string(),
            DMUX_BIN.to_string(),
        ]);
        argv.extend(attach_args(scratch, token));
    } else {
        // util-linux script: -c takes one command string; scratch paths
        // contain no shell metacharacters.
        let command = std::iter::once(DMUX_BIN.to_string())
            .chain(attach_args(scratch, token))
            .collect::<Vec<_>>()
            .join(" ");
        argv.extend(["-qec".into(), command, transcript.display().to_string()]);
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

fn detach_and_reap(scratch: &Scratch, child: &mut std::process::Child) {
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
        client_count(scratch) == 0
    });
    reap_wrapper(child);
}

fn reap_wrapper(child: &mut std::process::Child) {
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

fn exact_request(
    scratch: &Scratch,
    method: &str,
    request_uid: Uuid,
    payload: serde_json::Value,
    space: &CreatedSpace,
) -> dmux::remote::protocol::Envelope {
    let registry = scratch.registry();
    let row = registry.space(space.space_uid).unwrap();
    let epoch = registry
        .backend_server(row.backend_instance)
        .unwrap()
        .server_epoch
        .unwrap();
    let mut request = envelope(method, request_uid, payload);
    request.backend_instance_uid = Some(row.backend_instance);
    request.server_epoch = Some(epoch);
    request
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
    detach_and_reap(&scratch, &mut child);
}

#[test]
fn remote_attach_publishes_no_destination_marker_before_a_live_hook() {
    let scratch = Scratch::with_tmux("attach-no-premature-marker");
    let created = create_space(&scratch, "deferred");
    let (_, plan) = attach_plan(&scratch, &created);
    let transcript = scratch.data.path().join("deferred-marker.typescript");
    let mut child = pty_attach_with_transcript(&scratch, &plan.token, &transcript);
    wait_for("the exact tmux client", Duration::from_secs(10), || {
        client_count(&scratch) == 1
    });

    // This scratch server has no managed client hook. `_attach` may reserve
    // the exact PID/start/TTY record, but must leave every destination user
    // variable unpublished until a post-attach hook proves the live client.
    detach_and_reap(&scratch, &mut child);
    let transcript = std::fs::read(&transcript).unwrap();
    let transcript = String::from_utf8_lossy(&transcript);
    assert!(
        !transcript.contains("SetUserVar=dmux_"),
        "pre-exec attach leaked a destination marker: {transcript:?}"
    );
    assert_eq!(
        read_client_record(scratch.locks.path(), plan.request_uid)
            .unwrap()
            .space_uid,
        created.space_uid
    );
}

#[test]
fn exact_client_rpc_rejects_hidden_marker_switches_once_and_refreshes_context() {
    let scratch = Scratch::with_tmux("client-correlation");
    let first = create_space(&scratch, "first");
    let (_, plan) = attach_plan(&scratch, &first);
    let transcript: PathBuf = scratch.data.path().join("cold-attach.typescript");
    let mut child = pty_attach_with_transcript(&scratch, &plan.token, &transcript);
    wait_for("the exact tmux client", Duration::from_secs(10), || {
        client_count(&scratch) == 1
    });

    let session = scratch
        .registry()
        .current_binding(first.space_uid)
        .unwrap()
        .unwrap()
        .native_token;
    let hidden = scratch.tmux(&[
        "new-window",
        "-d",
        "-t",
        &session,
        "-P",
        "-F",
        TEST_WINDOW_PANE_FORMAT,
    ]);
    assert!(
        hidden.status.success(),
        "{}",
        String::from_utf8_lossy(&hidden.stderr)
    );
    let hidden = String::from_utf8(hidden.stdout).unwrap();
    let (hidden_window, hidden_pane) = hidden.trim().split_once(TMUX_FORMAT_SEPARATOR).unwrap();
    let epoch = plan.server_epoch;
    let hidden_status = TmuxClientStatusPayload {
        client_uid: plan.request_uid,
        space_uid: first.space_uid,
        group_ref: format!("g{}.tx-{}", epoch.0, hidden_window.trim_start_matches('@')),
        split_ref: format!("p{}.tx-{}", epoch.0, hidden_pane.trim_start_matches('%')),
    };
    let request = exact_request(
        &scratch,
        protocol::methods::TMUX_CLIENT_STATUS,
        Uuid::new_v4(),
        serde_json::to_value(hidden_status).unwrap(),
        &first,
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 4, "{response:?}");
    assert_eq!(error_code(&response), "identity_conflict");

    let status = TmuxClientStatusPayload {
        client_uid: plan.request_uid,
        space_uid: first.space_uid,
        group_ref: first.group_ref.clone(),
        split_ref: first.split_ref.clone(),
    };
    let request = exact_request(
        &scratch,
        protocol::methods::TMUX_CLIENT_STATUS,
        Uuid::new_v4(),
        serde_json::to_value(status).unwrap(),
        &first,
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let status: TmuxClientStatusResult = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(status.correlated);

    let second = create_space(&scratch, "second");
    let switch = TmuxClientSwitchPayload {
        client_uid: plan.request_uid,
        from_space_uid: first.space_uid,
        from_group_ref: first.group_ref.clone(),
        from_split_ref: first.split_ref.clone(),
        to_space_uid: second.space_uid,
    };
    let request_uid = Uuid::new_v4();
    let switch_request = exact_request(
        &scratch,
        protocol::methods::TMUX_CLIENT_SWITCH,
        request_uid,
        serde_json::to_value(switch).unwrap(),
        &first,
    );
    let (code, response) = scratch.agent(&switch_request);
    assert_eq!(code, 0, "{response:?}");
    let switched: TmuxClientSwitchResult =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(switched.switched && !switched.replayed);
    let second_session = scratch
        .registry()
        .current_binding(second.space_uid)
        .unwrap()
        .unwrap()
        .native_token;
    let clients = scratch.tmux(&["list-clients", "-F", "#{session_id}"]);
    assert_eq!(
        String::from_utf8_lossy(&clients.stdout).trim(),
        second_session
    );
    let (_, peer_plan) = attach_plan(&scratch, &second);
    let peer_transcript: PathBuf = scratch.data.path().join("peer-refresh.typescript");
    let mut peer = pty_attach_with_transcript(&scratch, &peer_plan.token, &peer_transcript);
    wait_for(
        "two exact clients on the destination Space",
        Duration::from_secs(10),
        || client_count(&scratch) == 2,
    );

    // Model an out-of-band GUI Group focus mutation: no native hook-client
    // facts are available to the command, so the owner refresh RPC must
    // correlate the exact attach UID, constrain the live @/% to the fresh
    // hierarchy, publish it, and make the immediately following key's
    // status preflight succeed.
    let focused = scratch.tmux(&[
        "new-window",
        "-t",
        &second_session,
        "-P",
        "-F",
        TEST_WINDOW_PANE_FORMAT,
    ]);
    assert!(
        focused.status.success(),
        "{}",
        String::from_utf8_lossy(&focused.stderr)
    );
    let focused = String::from_utf8(focused.stdout).unwrap();
    let (focused_window, focused_pane) = focused.trim().split_once(TMUX_FORMAT_SEPARATOR).unwrap();
    let focused_group = format!("g{}.tx-{}", epoch.0, focused_window.trim_start_matches('@'));
    let focused_split = format!("p{}.tx-{}", epoch.0, focused_pane.trim_start_matches('%'));
    let refresh = TmuxClientRefreshPayload {
        client_uid: plan.request_uid,
        space_uid: second.space_uid,
        group_ref: Some(focused_group.clone()),
        split_ref: None,
    };
    let request = exact_request(
        &scratch,
        protocol::methods::TMUX_CLIENT_REFRESH,
        Uuid::new_v4(),
        serde_json::to_value(refresh).unwrap(),
        &second,
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let refreshed_receipt: TmuxClientRefreshResult =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(refreshed_receipt.published);
    assert_eq!(refreshed_receipt.published_clients, 2);
    assert_eq!(refreshed_receipt.group_ref, focused_group);
    assert_eq!(refreshed_receipt.split_ref, focused_split);

    let next_key = TmuxClientStatusPayload {
        client_uid: plan.request_uid,
        space_uid: second.space_uid,
        group_ref: refreshed_receipt.group_ref,
        split_ref: refreshed_receipt.split_ref,
    };
    let request = exact_request(
        &scratch,
        protocol::methods::TMUX_CLIENT_STATUS,
        Uuid::new_v4(),
        serde_json::to_value(next_key).unwrap(),
        &second,
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let next_status: TmuxClientStatusResult =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(next_status.correlated);

    let peer_record = read_client_record(scratch.locks.path(), peer_plan.request_uid).unwrap();
    assert!(
        scratch
            .tmux(&["detach-client", "-t", &peer_record.client_tty])
            .status
            .success()
    );
    wait_for("the peer client to detach", Duration::from_secs(10), || {
        client_count(&scratch) == 1
    });
    reap_wrapper(&mut peer);
    let peer_bytes = std::fs::read(&peer_transcript).unwrap();
    let peer_output = String::from_utf8_lossy(&peer_bytes);
    assert!(
        peer_output
            .match_indices("SetUserVar=dmux_group_ref=")
            .count()
            >= 1,
        "peer transcript must contain the exact post-attach session-focus marker publication"
    );

    let client = scratch.tmux(&[
        "list-clients",
        "-F",
        "#{client_name}__DMUX_FIELD_7F4A9C2E__#{client_pid}__DMUX_FIELD_7F4A9C2E__#{client_tty}__DMUX_FIELD_7F4A9C2E__#{session_id}__DMUX_FIELD_7F4A9C2E__#{window_id}__DMUX_FIELD_7F4A9C2E__#{pane_id}",
    ]);
    assert!(client.status.success());
    let client = String::from_utf8(client.stdout).unwrap();
    let fields = client
        .trim()
        .split(TMUX_FORMAT_SEPARATOR)
        .collect::<Vec<_>>();
    let [
        hook_client,
        client_pid,
        client_tty,
        session_id,
        window_id,
        pane_id,
    ] = fields.as_slice()
    else {
        panic!("malformed scratch list-client row: {client:?}")
    };
    let claim = TmuxHookClientClaim {
        namespace: scratch.ns.clone().unwrap(),
        hook_client: (*hook_client).to_string(),
        client_pid: client_pid.parse().unwrap(),
        client_tty: (*client_tty).to_string(),
        session_id: (*session_id).to_string(),
        window_id: (*window_id).to_string(),
        pane_id: (*pane_id).to_string(),
    };
    let mut maintenance = OrderedLocks::new(scratch.locks.path());
    maintenance
        .acquire(LockScope::AuthorityGate, LockMode::Exclusive)
        .unwrap();
    let busy = refresh_controller_context_from_tmux_hook(&scratch.env(), &claim).unwrap_err();
    assert_eq!(busy.code, dmux::error::ErrorCode::OperationInProgress);
    drop(maintenance);
    let refreshed = refresh_controller_context_from_tmux_hook(&scratch.env(), &claim).unwrap();
    assert_eq!(
        refreshed.space_uid, second.space_uid,
        "hook refresh must derive the post-switch Space from the exact live row, not the immutable attach hint"
    );

    let (code, response) = scratch.agent(&switch_request);
    assert_eq!(code, 0, "{response:?}");
    let replayed: TmuxClientSwitchResult =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.switched && replayed.replayed);

    detach_and_reap(&scratch, &mut child);
    let transcript = std::fs::read(&transcript).unwrap();
    let transcript = String::from_utf8_lossy(&transcript);
    for field in [
        "dmux_tmux_client_uid",
        "dmux_context_version",
        "dmux_host_uid",
        "dmux_space_uid",
        "dmux_server_epoch",
        "dmux_group_ref",
        "dmux_split_ref",
    ] {
        assert!(
            transcript.contains(&format!("SetUserVar={field}=")),
            "post-attach switch transcript omitted {field}"
        );
    }
}

#[test]
fn exact_client_detach_removes_only_that_client_preserves_panes_and_replays() {
    let scratch = Scratch::with_tmux("client-detach");
    let created = create_space(&scratch, "shared");
    let (_, first_plan) = attach_plan(&scratch, &created);
    let (_, second_plan) = attach_plan(&scratch, &created);
    let mut first = pty_attach(&scratch, &first_plan.token);
    wait_for(
        "the first exact tmux client",
        Duration::from_secs(10),
        || client_count(&scratch) == 1,
    );
    let mut second = pty_attach(&scratch, &second_plan.token);
    wait_for("two exact tmux clients", Duration::from_secs(10), || {
        client_count(&scratch) == 2
    });
    let first_record = read_client_record(scratch.locks.path(), first_plan.request_uid).unwrap();
    let second_record = read_client_record(scratch.locks.path(), second_plan.request_uid).unwrap();
    assert_ne!(first_record.client_pid, second_record.client_pid);
    let panes_before = scratch.tmux(&["list-panes", "-a", "-F", TEST_SESSION_WINDOW_PANE_FORMAT]);
    assert!(panes_before.status.success());

    let payload = TmuxClientDetachPayload {
        client_uid: first_plan.request_uid,
        space_uid: created.space_uid,
        group_ref: created.group_ref.clone(),
        split_ref: created.split_ref.clone(),
    };
    let request = exact_request(
        &scratch,
        protocol::methods::TMUX_CLIENT_DETACH,
        Uuid::new_v4(),
        serde_json::to_value(payload).unwrap(),
        &created,
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let detached: TmuxClientDetachResult =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(detached.detached && !detached.replayed);
    wait_for("one untouched tmux client", Duration::from_secs(10), || {
        client_count(&scratch) == 1
    });
    let remaining = scratch.tmux(&[
        "list-clients",
        "-F",
        "#{client_pid}__DMUX_FIELD_7F4A9C2E__#{client_tty}",
    ]);
    assert!(remaining.status.success());
    assert_eq!(
        String::from_utf8_lossy(&remaining.stdout).trim(),
        format!(
            "{}{}{}",
            second_record.client_pid, TMUX_FORMAT_SEPARATOR, second_record.client_tty
        )
    );
    let panes_after = scratch.tmux(&["list-panes", "-a", "-F", TEST_SESSION_WINDOW_PANE_FORMAT]);
    assert_eq!(panes_after.stdout, panes_before.stdout);

    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let replayed: TmuxClientDetachResult =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.detached && replayed.replayed);
    assert_eq!(client_count(&scratch), 1);

    reap_wrapper(&mut first);
    detach_and_reap(&scratch, &mut second);
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
    // fresh epoch after re-bootstrap. Linux tmux may acknowledge
    // `kill-server` just before the old process stops accepting clients.
    // A stale socket pathname may legitimately remain, so wait on the
    // read-only server identity probe rather than path absence.
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
            "fresh",
        ])
        .env("DMUX_RUNTIME_DIR", scratch.locks.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "replacement scratch tmux failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
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

/// The `_attach` reader's own witness comparison (ADR 012 WS-A.9 at
/// `remote/attach.rs::verify`, O's close → R): a replaced server on the
/// same namespace that presents the OLD epoch — the case the epoch option
/// alone cannot tell apart, and the one the restart test above does not
/// exercise because it re-bootstraps a fresh epoch — is refused as a stale
/// incarnation before the exec, the token stays consumed, and no client is
/// attached to the impostor. The refusal is the witness comparison's, not
/// `verify_epoch`'s pid check.
#[test]
fn a_replaced_server_presenting_the_old_epoch_refuses_the_token_as_a_stale_incarnation() {
    let scratch = Scratch::with_tmux("stale-token");
    let created = create_space(&scratch, "staleee");
    let (_, plan) = attach_plan(&scratch, &created);
    let registry = scratch.registry();
    let instance = registry
        .backend_instance_for_backend(dmux::model::Backend::Tmux)
        .unwrap()
        .expect("bootstrapped tmux instance");
    let published = registry.backend_server(instance).unwrap();
    let epoch = published
        .server_epoch
        .expect("bootstrap published an epoch");
    assert!(
        published.socket_dev.is_some() && published.socket_ino.is_some(),
        "tmux_bootstrap publishes the socket witnesses the reader compares"
    );
    drop(registry);

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
            "staleee",
        ])
        .env("DMUX_RUNTIME_DIR", scratch.locks.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "replacement scratch tmux failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    // The impostor presents the published epoch; nothing is re-bootstrapped,
    // so the registry row still names the dead incarnation.
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

    let out = attach_refused(&scratch, &plan.token);
    assert_eq!(
        out.status.code(),
        Some(1),
        "backend_epoch_changed maps to 1: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("stale incarnation") && stderr.contains("ADR 012 §3.1 state F"),
        "the reader's own witness comparison must name the fault: {stderr}"
    );
    assert!(
        !stderr.contains("restarted since the plan was issued"),
        "verify_epoch's pid comparison must not have fired first: {stderr}"
    );
    assert_eq!(
        client_count(&scratch),
        0,
        "a client attached to the impostor"
    );

    // Single use is not negotiable: the refused token is consumed.
    let again = attach_refused(&scratch, &plan.token);
    assert_ne!(again.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&again.stderr).contains("stale incarnation"),
        "a consumed token is refused before any server probe: {}",
        String::from_utf8_lossy(&again.stderr)
    );
}
