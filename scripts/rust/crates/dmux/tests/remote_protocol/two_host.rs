//! Two-host P7 gate leg against Archie over real ssh (ADR 009 §4 rules):
//! everything runs in a scratch workspace (`~/.cache/dmux-w5/rust`), a
//! scratch registry under a remote mktemp dir, and a scratch tmux `-L`
//! namespace — never `~/dotfiles`, the default tmux server, or any
//! production path. Skips gracefully (with a reason) when
//! `ssh -o BatchMode=yes archie true` fails; when Archie answers, it runs.

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use dmux::operations::CreatedSpace;
use dmux::registry::{NetworkClass, RouteSpec, Transport};
use dmux::remote::client::{
    AgentInvocation, PeerExpectation, SshInvoker, call_over_routes, request_envelope,
};
use dmux::remote::enroll::enroll_target;
use dmux::remote::lineage::PeerLineage;
use dmux::remote::protocol::{self, AttachPlan, Envelope};
use dmux::remote::routes::outcome;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::util::{Scratch, wait_for};

/// Remote dmux spelling relative to the ssh login dir ($HOME).
const REMOTE_BIN: &str = ".cache/dmux-w5/rust/target/debug/dmux";
const HOST: &str = "archie";

/// Shared multiplexed master for the test's many probe/setup connections,
/// so rapid polling never trips sshd's unauthenticated-connection limits.
/// The production transport (`SshInvoker`) stays unmultiplexed.
const CONTROL: &[&str] = &[
    "-oControlMaster=auto",
    "-oControlPath=/tmp/dmux-w5-p7-cm-%C",
    "-oControlPersist=120",
];

fn ssh_raw(command: &str) -> Output {
    Command::new("ssh")
        .args(CONTROL)
        .args(["-oBatchMode=yes", "-oConnectTimeout=10", HOST, command])
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// The production invoker plus the shared control master (Archie's sshd
/// applies per-source penalties to failed/aborted fresh connections — the
/// fault legs below deliberately cause some — so the healthy traffic rides
/// one authenticated master).
fn invoker() -> SshInvoker {
    SshInvoker {
        extra_options: CONTROL.iter().map(|s| s.to_string()).collect(),
        ..SshInvoker::default()
    }
}

fn ssh_ok(command: &str) -> String {
    let out = ssh_raw(command);
    assert!(
        out.status.success(),
        "ssh {HOST} {command:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn archie_reachable() -> bool {
    // A few spaced attempts: one transiently dropped connection (sshd
    // MaxStartups pressure from a previous run) must not skip the gate.
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        let ok = Command::new("ssh")
            .args(CONTROL)
            .args(["-oBatchMode=yes", "-oConnectTimeout=5", HOST, "true"])
            .stdin(Stdio::null())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return true;
        }
    }
    false
}

/// Remote scratch state, torn down on drop even when an assert fails.
struct RemoteScratch {
    base: String,
    ns: String,
}

impl RemoteScratch {
    fn provision() -> RemoteScratch {
        // Sync the CURRENT sources and rebuild in the scratch workspace
        // (deps are cached there from the W5 warm-up build).
        let rsync = Command::new("rsync")
            .args([
                "-a",
                "--delete",
                "--exclude",
                "target/",
                "/Users/fredrir/dotfiles/scripts/rust/",
                "archie:.cache/dmux-w5/rust/",
            ])
            .output()
            .unwrap();
        assert!(
            rsync.status.success(),
            "rsync: {}",
            String::from_utf8_lossy(&rsync.stderr)
        );
        let build = ssh_raw(
            "cd ~/.cache/dmux-w5/rust && PATH=$HOME/.cargo/bin:$PATH \
             cargo build -p dmux --bins 2>&1 | tail -3",
        );
        assert!(
            build.status.success(),
            "remote build: {}",
            String::from_utf8_lossy(&build.stdout)
        );

        let base = ssh_ok("mktemp -d /tmp/dmux-w5-p7.XXXXXX");
        assert!(base.starts_with("/tmp/dmux-w5-p7."), "{base}");
        ssh_ok(&format!("mkdir -p {base}/data {base}/locks"));
        let ns = format!("dmux-w5-p7-{}", std::process::id());
        ssh_ok(&format!(
            "DMUX_RUNTIME_DIR={base}/locks tmux -L {ns} -f /dev/null \
             new-session -d -s seed"
        ));
        // LANG: `_tmux-bootstrap` is root-owned and does not self-normalize
        // its locale; a POSIX-locale tmux client mangles the provider's
        // U+001F separators (the `_agent`/`_attach` endpoints normalize
        // themselves — see remote::normalize_utf8_locale).
        ssh_ok(&format!(
            "LANG=C.UTF-8 {REMOTE_BIN} _tmux-bootstrap --namespace {ns} \
             --data-dir {base}/data --lock-dir {base}/locks"
        ));
        RemoteScratch { base, ns }
    }

    fn sessions(&self) -> Vec<String> {
        String::from_utf8_lossy(
            &ssh_raw(&format!(
                "tmux -L {} list-sessions -F '#{{session_name}}' 2>/dev/null",
                self.ns
            ))
            .stdout,
        )
        .lines()
        .map(str::to_string)
        .collect()
    }

    fn clients(&self) -> usize {
        String::from_utf8_lossy(
            &ssh_raw(&format!(
                "tmux -L {} list-clients -F '#{{client_tty}}' 2>/dev/null",
                self.ns
            ))
            .stdout,
        )
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
    }

    fn invocation(&self, method: &str) -> AgentInvocation {
        let mut invocation = AgentInvocation::new(method);
        invocation.remote_bin = REMOTE_BIN.to_string();
        invocation.data_dir = Some(format!("{}/data", self.base));
        invocation.lock_dir = Some(format!("{}/locks", self.base));
        invocation
    }
}

impl Drop for RemoteScratch {
    fn drop(&mut self) {
        // Scratch only: kill the scratch namespace and remove the temp
        // dirs; ~/.cache/dmux-w5 itself stays for the root's gate run.
        let _ = ssh_raw(&format!("tmux -L {} kill-server 2>/dev/null", self.ns));
        let _ = ssh_raw(&format!("rm -rf {}", self.base));
    }
}

fn request(client: &Scratch, method: &str, payload: Value) -> Envelope {
    let registry = client.registry();
    let identity = registry.identity().unwrap();
    let head = registry.authority_head().unwrap();
    request_envelope(&identity, &head, method, Uuid::new_v4(), payload)
}

fn call(
    client: &Scratch,
    remote: &RemoteScratch,
    expectation: &PeerExpectation,
    envelope: &Envelope,
) -> Result<Envelope, dmux::error::TypedError> {
    let mut registry = client.registry();
    let outcome = call_over_routes(
        &mut registry,
        expectation,
        envelope,
        &invoker(),
        &remote.invocation(&envelope.method),
        Duration::from_secs(60),
    )?;
    Ok(outcome.envelope)
}

/// The whole two-host matrix in one ordered flow: enrollment identity,
/// remote create/replay/rename/rm with cross-invocation replay, the
/// envelope epoch claim refusal, the real-PTY attach with token replay,
/// and the real-ssh route fault legs.
#[test]
fn archie_end_to_end_matrix() {
    if !archie_reachable() {
        eprintln!(
            "SKIP two_host::archie_end_to_end_matrix: \
             `ssh -o BatchMode=yes archie true` failed (host unreachable)"
        );
        return;
    }
    let remote = RemoteScratch::provision();
    let client = Scratch::new("archie-client");

    // --- Enrollment: hello over real ssh, idempotent by HostUid. ---
    let enrollment = enroll_target(
        &client.env(),
        HOST,
        &invoker(),
        &remote.invocation(protocol::methods::HELLO),
        Duration::from_secs(60),
    )
    .expect("enrollment against the scratch Archie agent");
    assert!(enrollment.host.newly_enrolled);
    assert_eq!(enrollment.host.alias, "b", "first remote gets alias b");
    assert_eq!(enrollment.network_class, NetworkClass::Usb);
    assert_eq!(enrollment.lineage, PeerLineage::FirstContact);
    let owner_uid = enrollment.hello.host_uid;
    assert_ne!(
        owner_uid,
        client.registry().identity().unwrap().host_uid,
        "remote authority is its own identity"
    );
    let tmux_backend = enrollment
        .hello
        .backends
        .iter()
        .find(|backend| {
            backend.backend == dmux::model::Backend::Tmux && backend.server_epoch.is_some()
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "bootstrapped tmux instance visible in hello: {:?}",
                enrollment.hello.backends
            )
        });

    // Re-enrolling matches the SAME HostUid (idempotent, same alias).
    let again = enroll_target(
        &client.env(),
        HOST,
        &invoker(),
        &remote.invocation(protocol::methods::HELLO),
        Duration::from_secs(60),
    )
    .unwrap();
    assert!(!again.host.newly_enrolled);
    assert_eq!(again.host.alias, "b");
    assert_eq!(again.hello.host_uid, owner_uid);

    let expectation = PeerExpectation {
        host_uid: owner_uid,
        need_capability: None,
        claimed_current: false,
    };

    // --- Remote create + cross-invocation replay (acceptance 22). ---
    let new_request = request(
        &client,
        protocol::methods::NEW,
        json!({ "name": "w5proj", "backend": "tmux", "program": ["sleep", "300"] }),
    );
    let response = call(&client, &remote, &expectation, &new_request).unwrap();
    let created: CreatedSpace = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(!created.replayed);
    assert!(remote.sessions().contains(&"w5proj".to_string()));

    let response = call(&client, &remote, &expectation, &new_request).unwrap();
    let replayed: CreatedSpace = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.replayed, "ack-loss retry returns the original");
    assert_eq!(replayed.space_uid, created.space_uid);
    assert_eq!(
        remote.sessions().iter().filter(|n| *n == "w5proj").count(),
        1
    );

    // --- Envelope epoch claim verification against the live server. ---
    let mut stale = request(
        &client,
        protocol::methods::NEW,
        json!({ "name": "stale-claim", "backend": "tmux" }),
    );
    stale.server_epoch = Some(dmux::model::ServerEpoch(Uuid::from_u128(42)));
    let error =
        call(&client, &remote, &expectation, &stale).expect_err("stale epoch claim must refuse");
    assert_eq!(error.code, dmux::error::ErrorCode::BackendEpochChanged);
    assert!(!remote.sessions().contains(&"stale-claim".to_string()));

    // --- Remote rename with replay. ---
    let rename_request = request(
        &client,
        protocol::methods::RENAME,
        json!({ "space_uid": created.space_uid, "new_name": "w5renamed" }),
    );
    call(&client, &remote, &expectation, &rename_request).unwrap();
    assert!(remote.sessions().contains(&"w5renamed".to_string()));
    let response = call(&client, &remote, &expectation, &rename_request).unwrap();
    let result: protocol::RenameResult = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(result.replayed);

    // --- P8b remote hierarchy over real ssh: read, group_new + replay,
    // stale-epoch refusal, split_new, group_rm. ---
    let response = call(
        &client,
        &remote,
        &expectation,
        &request(
            &client,
            protocol::methods::HIERARCHY,
            json!({ "space_uid": created.space_uid }),
        ),
    )
    .unwrap();
    let tree: dmux::operations::SpaceHierarchy =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(tree.groups.len(), 1);
    assert_eq!(tree.groups[0].splits.len(), 1);

    let group_request = request(
        &client,
        protocol::methods::GROUP_NEW,
        json!({ "space_uid": created.space_uid, "program": ["sleep", "300"] }),
    );
    let response = call(&client, &remote, &expectation, &group_request).unwrap();
    let group: dmux::operations::CreatedChild =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(group.kind, dmux::model::ChildKind::Group);
    let windows = ssh_ok(&format!(
        "tmux -L {} list-windows -t w5renamed -F '#{{window_id}}'",
        remote.ns
    ));
    assert_eq!(windows.lines().count(), 2);

    // Cross-invocation replay: identical envelope, no second window.
    let response = call(&client, &remote, &expectation, &group_request).unwrap();
    let replayed_group: dmux::operations::CreatedChild =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed_group.replayed);
    assert_eq!(replayed_group.group_ref, group.group_ref);
    let windows = ssh_ok(&format!(
        "tmux -L {} list-windows -t w5renamed -F '#{{window_id}}'",
        remote.ns
    ));
    assert_eq!(windows.lines().count(), 2, "replay must not spawn");

    // Stale-epoch child ref: typed refusal, nothing created.
    let handle = group.group_ref.split_once('.').unwrap().1;
    let stale_ref = format!("g{}.{handle}", Uuid::from_u128(7));
    let error = call(
        &client,
        &remote,
        &expectation,
        &request(
            &client,
            protocol::methods::SPLIT_NEW,
            json!({ "space_uid": created.space_uid, "group_ref": stale_ref }),
        ),
    )
    .expect_err("stale child epoch must refuse");
    assert_eq!(error.code, dmux::error::ErrorCode::BackendEpochChanged);

    let response = call(
        &client,
        &remote,
        &expectation,
        &request(
            &client,
            protocol::methods::SPLIT_NEW,
            json!({
                "space_uid": created.space_uid,
                "group_ref": group.group_ref,
                "direction": "right",
                "percent": 30,
                "program": ["sleep", "300"],
            }),
        ),
    )
    .unwrap();
    let split: dmux::operations::CreatedChild =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(split.group_ref, group.group_ref);

    let response = call(
        &client,
        &remote,
        &expectation,
        &request(
            &client,
            protocol::methods::GROUP_RM,
            json!({ "space_uid": created.space_uid, "group_ref": group.group_ref }),
        ),
    )
    .unwrap();
    let removed: dmux::operations::RemovedChild =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(removed.kind, dmux::model::ChildKind::Group);
    let windows = ssh_ok(&format!(
        "tmux -L {} list-windows -t w5renamed -F '#{{window_id}}'",
        remote.ns
    ));
    assert_eq!(windows.lines().count(), 1, "hierarchy leg cleaned up");

    // --- Real PTY attach over `ssh -tt` (acceptance: PTY attach). ---
    let plan_request = request(
        &client,
        protocol::methods::ATTACH_PLAN,
        json!({ "space_uid": created.space_uid, "route": HOST }),
    );
    let response = call(&client, &remote, &expectation, &plan_request).unwrap();
    let plan: AttachPlan = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(plan.host_uid, owner_uid);
    assert_eq!(plan.token.len(), 64);

    assert_eq!(remote.clients(), 0);
    let attach_cmd = format!(
        "{REMOTE_BIN} _attach --token {} --data-dir {}/data --lock-dir {}/locks",
        plan.token, remote.base, remote.base
    );
    let mut attached = Command::new("ssh")
        .args(CONTROL)
        .args(["-oBatchMode=yes", "-tt", HOST, &attach_cmd])
        .env("TERM", "xterm-256color")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("a live client on Archie", Duration::from_secs(30), || {
        remote.clients() > 0
    });

    // Token replay over the same channel: typed refusal, no attach.
    let replayed_attach = Command::new("ssh")
        .args(CONTROL)
        .args(["-oBatchMode=yes", "-tt", HOST, &attach_cmd])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(replayed_attach.status.code(), Some(6));
    let noise = format!(
        "{}{}",
        String::from_utf8_lossy(&replayed_attach.stdout),
        String::from_utf8_lossy(&replayed_attach.stderr)
    );
    assert!(noise.contains("already redeemed"), "{noise}");

    let mut detach_request = request(
        &client,
        protocol::methods::TMUX_CLIENT_DETACH,
        json!({
            "client_uid": plan.request_uid,
            "space_uid": created.space_uid,
            "group_ref": created.group_ref,
            "split_ref": created.split_ref,
        }),
    );
    detach_request.backend_instance_uid = Some(tmux_backend.backend_instance_uid);
    detach_request.server_epoch = Some(plan.server_epoch);
    let response = call(&client, &remote, &expectation, &detach_request).unwrap();
    let result: protocol::TmuxClientDetachResult =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(result.detached && !result.replayed);
    assert_eq!(remote.clients(), 0);
    let response = call(&client, &remote, &expectation, &detach_request).unwrap();
    let replayed: protocol::TmuxClientDetachResult =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.detached && replayed.replayed);
    wait_for(
        "the attach channel to close",
        Duration::from_secs(30),
        || matches!(attached.try_wait(), Ok(Some(_))),
    );

    // --- Remote rm with replay, then tombstone refusal. ---
    let rm_request = request(
        &client,
        protocol::methods::RM,
        json!({ "space_uid": created.space_uid }),
    );
    call(&client, &remote, &expectation, &rm_request).unwrap();
    assert!(!remote.sessions().contains(&"w5renamed".to_string()));
    let response = call(&client, &remote, &expectation, &rm_request).unwrap();
    let result: protocol::RmResult = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(result.replayed && result.removed);

    // --- Route fault matrix over REAL ssh (acceptance 21). ---
    // Dead endpoint first by priority: connection refused on 127.0.0.1:1
    // (pre-auth transport class) must fail over to the Archie route with
    // the identical request; outcomes recorded on both.
    {
        let mut registry = client.registry();
        registry
            .upsert_route(&RouteSpec {
                host_uid: owner_uid,
                transport: Transport::Openssh,
                endpoint: "ssh://127.0.0.1:1".into(),
                username: None,
                wez_domain: None,
                network_class: NetworkClass::Other,
                priority: 1,
                required_capability: None,
                trust_fingerprint: None,
                enabled: true,
            })
            .unwrap();
    }
    let hello_request = request(&client, protocol::methods::HELLO, json!({}));
    let response = call(&client, &remote, &expectation, &hello_request).unwrap();
    assert_eq!(response.host_uid, owner_uid);
    {
        let registry = client.registry();
        let routes = registry.routes_for(owner_uid).unwrap();
        let dead = routes
            .iter()
            .find(|r| r.endpoint == "ssh://127.0.0.1:1")
            .unwrap();
        assert_eq!(
            dead.last_outcome.as_deref(),
            Some(outcome::TRANSPORT_UNREACHABLE)
        );
        let good = routes.iter().find(|r| r.endpoint == HOST).unwrap();
        assert_eq!(good.last_outcome.as_deref(), Some(outcome::OK));
    }

    // Wrong-user AUTH failure first by priority: terminal, the healthy
    // Archie route must NOT be tried after it.
    {
        let mut registry = client.registry();
        let dead = registry
            .routes_for(owner_uid)
            .unwrap()
            .into_iter()
            .find(|r| r.endpoint == "ssh://127.0.0.1:1")
            .unwrap();
        registry.set_route_enabled(dead.route_id, false).unwrap();
        // Distinct endpoint spelling: routes are keyed on
        // (host, transport, endpoint) and must not clobber the good one.
        registry
            .upsert_route(&RouteSpec {
                host_uid: owner_uid,
                transport: Transport::Openssh,
                endpoint: "ssh://dmux-w5-no-such-user@archie:22".into(),
                username: None,
                wez_domain: None,
                network_class: NetworkClass::Usb,
                priority: 2,
                required_capability: None,
                trust_fingerprint: None,
                enabled: true,
            })
            .unwrap();
    }
    // Archie's sshd applies per-source penalties: while penalized it drops
    // the connection BEFORE authentication ("Connection closed by ..."),
    // which is genuinely the pre-auth transport class and correctly fails
    // over. Retry the leg with decay pauses until the wrong user reaches
    // the AUTH stage; that attempt must be terminal.
    let mut provoked_auth_failure = false;
    for attempt in 0..5 {
        if attempt > 0 {
            eprintln!("auth leg: penalty drop, waiting for sshd penalty decay");
            std::thread::sleep(Duration::from_secs(10));
        }
        let before_at = {
            let registry = client.registry();
            registry
                .routes_for(owner_uid)
                .unwrap()
                .into_iter()
                .find(|r| r.endpoint == HOST && r.username.is_none())
                .unwrap()
                .last_outcome_at
        };
        let hello_request = request(&client, protocol::methods::HELLO, json!({}));
        let result = call(&client, &remote, &expectation, &hello_request);
        let registry = client.registry();
        let routes = registry.routes_for(owner_uid).unwrap();
        let bad_user = routes
            .iter()
            .find(|r| r.endpoint.contains("no-such-user"))
            .unwrap();
        match bad_user.last_outcome.as_deref() {
            Some(t) if t == outcome::AUTH_FAILED => {
                let error = result.expect_err("auth failure must be terminal, not failover");
                assert_eq!(error.code, dmux::error::ErrorCode::AuthFailed);
                let good = routes
                    .iter()
                    .find(|r| r.endpoint == HOST && r.username.is_none())
                    .unwrap();
                assert_eq!(
                    good.last_outcome.as_deref(),
                    Some(outcome::OK),
                    "the healthy route keeps its PREVIOUS outcome"
                );
                assert_eq!(
                    good.last_outcome_at, before_at,
                    "the healthy route was NOT attempted after the auth failure"
                );
                provoked_auth_failure = true;
                break;
            }
            Some(t) if t == outcome::TRANSPORT_UNREACHABLE => continue,
            other => panic!("unexpected bad-user route outcome {other:?} ({result:?})"),
        }
    }
    assert!(
        provoked_auth_failure,
        "could not reach the ssh auth stage within the retry budget \
         (sshd per-source penalty never decayed)"
    );
}
