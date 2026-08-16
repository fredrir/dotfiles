//! Black-box tests for the internal pane-bootstrap helper binary against the
//! frozen broker contract in `dmux::bootstrap` (plan §11.1, §13.1; ADR 004,
//! ADR 005 §2). The test process plays the broker with the real broker fns
//! (`prepare` / `send_result` / `read_pane_env` / `read_ack`) over a scratch
//! `DMUX_RUNTIME_DIR`.

use std::num::NonZeroU64;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use dmux::bootstrap::{
    self, BootstrapPaths, BootstrapResult, HelperAck, MarkerContext, PaneEnvRecord,
};
use dmux::model::{Backend, HostUid, ServerEpoch, SpaceNo, SpaceUid};
use uuid::Uuid;

const EXIT_PROTOCOL: i32 = 40;
const EXIT_USAGE: i32 = 2;

fn helper_command(runtime_dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pane-bootstrap"));
    command
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("WEZTERM_PANE")
        .env_remove("DMUX_BOOTSTRAP_TIMEOUT_SECS")
        .env("DMUX_RUNTIME_DIR", runtime_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn context() -> MarkerContext {
    MarkerContext {
        host_uid: HostUid(Uuid::from_u128(0x11)),
        space_uid: SpaceUid(Uuid::from_u128(0x22)),
        space_no: SpaceNo(NonZeroU64::new(7).unwrap()),
        backend: Backend::Wez,
        domain: None,
        server_epoch: ServerEpoch(Uuid::from_u128(0x33)),
        group_ref: "g00000000-0000-0000-0000-000000000033.wz-3".into(),
        split_ref: "p00000000-0000-0000-0000-000000000033.wz-4".into(),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn read_timeout_marker(paths: &BootstrapPaths) -> serde_json::Value {
    let text = std::fs::read_to_string(&paths.timeout_marker).expect("timeout marker exists");
    serde_json::from_str(&text).expect("timeout marker is JSON")
}

#[test]
fn happy_path_emits_markers_and_execs_the_program() {
    let runtime = tempfile::tempdir().unwrap();
    let uid = Uuid::new_v4();
    let paths = bootstrap::prepare(runtime.path(), uid).unwrap();

    let child = helper_command(runtime.path())
        .env("WEZTERM_PANE", "42")
        .arg(uid.to_string())
        .arg("--")
        .args([
            "sh",
            "-c",
            r#"printf "%s|%s" "$DMUX_SPACE_UID" "$DMUX_GROUP_REF""#,
        ])
        .spawn()
        .unwrap();
    let helper_pid = child.id();

    let result = BootstrapResult {
        request_uid: uid,
        context: context(),
    };
    // send_result retries on ENXIO until the helper's O_RDWR open appears.
    bootstrap::send_result(&paths, &result, Duration::from_secs(10)).unwrap();

    let output: Output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = &output.stdout;

    // Reserved title, unwrapped (no $TMUX).
    let reserved = format!("\x1b]2;dmux-bootstrap:{uid}\x07");
    assert!(contains(stdout, reserved.as_bytes()));

    // SetUserVar markers: lowercase names, base64 values. Known encodings:
    // "1" = MQ==, "wez" = d2V6; DMUX_DOMAIN=None emits an empty value.
    assert!(contains(
        stdout,
        b"\x1b]1337;SetUserVar=dmux_context_version=MQ==\x07"
    ));
    assert!(contains(
        stdout,
        b"\x1b]1337;SetUserVar=dmux_backend=d2V6\x07"
    ));
    assert!(contains(stdout, b"\x1b]1337;SetUserVar=dmux_domain=\x07"));
    for name in [
        "dmux_host_uid",
        "dmux_space_uid",
        "dmux_space_no",
        "dmux_server_epoch",
        "dmux_group_ref",
        "dmux_split_ref",
    ] {
        let prefix = format!("\x1b]1337;SetUserVar={name}=");
        assert!(contains(stdout, prefix.as_bytes()), "missing marker {name}");
    }

    // Final run title.
    let run = format!("\x1b]2;dmux-run:{uid}\x07");
    assert!(contains(stdout, run.as_bytes()));

    // The exec'd program saw the exported marker env.
    let proof = format!(
        "{}|{}",
        result.context.space_uid.0, result.context.group_ref
    );
    assert!(
        contains(stdout, proof.as_bytes()),
        "program output missing from {:?}",
        String::from_utf8_lossy(stdout)
    );

    // Pane-env record carries the helper's real identity (exec preserves the
    // PID, so the spawned child id is the helper pid).
    let record: PaneEnvRecord = bootstrap::read_pane_env(&paths, Duration::from_millis(100))
        .unwrap()
        .expect("pane-env written");
    assert_eq!(record.request_uid, uid);
    assert_eq!(record.wezterm_pane.as_deref(), Some("42"));
    assert_eq!(record.tmux_pane, None);
    assert_eq!(record.helper_pid, helper_pid);

    // Ack present; no timeout marker.
    assert_eq!(
        bootstrap::read_ack(&paths, Duration::from_millis(100)).unwrap(),
        Some(HelperAck { request_uid: uid })
    );
    assert!(!paths.timeout_marker.exists());
}

#[test]
fn timeout_writes_marker_exits_41_and_never_runs_the_program() {
    let runtime = tempfile::tempdir().unwrap();
    let uid = Uuid::new_v4();
    let paths = bootstrap::prepare(runtime.path(), uid).unwrap();

    let output = helper_command(runtime.path())
        .env("DMUX_BOOTSTRAP_TIMEOUT_SECS", "0.3")
        .arg(uid.to_string())
        .arg("--")
        .args(["sh", "-c", "echo SHOULD_NOT_RUN"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(bootstrap::EXIT_TIMEOUT));
    assert!(!contains(&output.stdout, b"SHOULD_NOT_RUN"));
    // Reserved title was still emitted before the wait.
    let reserved = format!("\x1b]2;dmux-bootstrap:{uid}\x07");
    assert!(contains(&output.stdout, reserved.as_bytes()));
    // No run title, no ack on a non-success path.
    assert!(!contains(&output.stdout, b"dmux-run:"));
    assert!(!paths.ack.exists());
    // Visible stderr line and the JSON marker.
    assert!(!output.stderr.is_empty());
    let marker = read_timeout_marker(&paths);
    assert_eq!(marker["uid"], serde_json::json!(uid.to_string()));
    assert!(marker["reason"].as_str().unwrap().contains("timed out"));
}

#[test]
fn uid_mismatch_is_a_protocol_violation_exit_40() {
    let runtime = tempfile::tempdir().unwrap();
    let uid = Uuid::new_v4();
    let other = Uuid::new_v4();
    let paths = bootstrap::prepare(runtime.path(), uid).unwrap();

    let child = helper_command(runtime.path())
        .arg(uid.to_string())
        .arg("--")
        .args(["sh", "-c", "echo SHOULD_NOT_RUN"])
        .spawn()
        .unwrap();

    let result = BootstrapResult {
        request_uid: other,
        context: context(),
    };
    bootstrap::send_result(&paths, &result, Duration::from_secs(10)).unwrap();

    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(EXIT_PROTOCOL));
    assert!(!contains(&output.stdout, b"SHOULD_NOT_RUN"));
    assert!(!contains(&output.stdout, b"dmux-run:"));
    assert!(!paths.ack.exists());
    let marker = read_timeout_marker(&paths);
    assert_eq!(marker["uid"], serde_json::json!(uid.to_string()));
    assert!(marker["reason"].as_str().unwrap().contains("mismatch"));
}

#[test]
fn missing_fifo_is_a_protocol_violation_exit_40() {
    let runtime = tempfile::tempdir().unwrap();
    let uid = Uuid::new_v4();
    // No prepare(): the broker never created the FIFO.

    let output = helper_command(runtime.path())
        .arg(uid.to_string())
        .arg("--")
        .args(["sh", "-c", "echo SHOULD_NOT_RUN"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(EXIT_PROTOCOL));
    assert!(!contains(&output.stdout, b"SHOULD_NOT_RUN"));
    let paths = BootstrapPaths::new(runtime.path(), uid);
    let marker = read_timeout_marker(&paths);
    assert!(marker["reason"].as_str().unwrap().contains("FIFO missing"));
}

#[test]
fn inside_tmux_the_title_is_dcs_passthrough_wrapped() {
    let runtime = tempfile::tempdir().unwrap();
    let uid = Uuid::new_v4();
    // Missing-FIFO path: exits fast, but the reserved title is emitted first.

    let output = helper_command(runtime.path())
        .env("TMUX", "fake")
        .arg(uid.to_string())
        .arg("--")
        .args(["sh", "-c", "true"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(EXIT_PROTOCOL));
    // ADR 005 §2 recipe: DCS tmux; introducer, ESC-doubled OSC 2 payload,
    // BEL, ST terminator.
    assert!(contains(&output.stdout, b"\x1bPtmux;"));
    let wrapped = format!("\x1bPtmux;\x1b\x1b]2;dmux-bootstrap:{uid}\x07\x1b\\");
    assert!(
        contains(&output.stdout, wrapped.as_bytes()),
        "stdout: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn bad_argv_exits_2_with_usage() {
    let runtime = tempfile::tempdir().unwrap();
    let uid = Uuid::new_v4();

    // No arguments at all.
    let output = helper_command(runtime.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    assert!(contains(&output.stderr, b"usage"));
    assert!(output.stdout.is_empty());

    // Missing the `--` separator.
    let output = helper_command(runtime.path())
        .args([uid.to_string(), "sh".into()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    assert!(contains(&output.stderr, b"usage"));

    // Bad uuid token.
    let output = helper_command(runtime.path())
        .args(["not-a-uuid", "--", "sh"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));

    // Non-canonical uuid spelling is rejected (broker always sends the
    // canonical lowercase hyphenated form via Uuid::to_string).
    let output = helper_command(runtime.path())
        .args([uid.to_string().to_uppercase(), "--".into(), "sh".into()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));

    // Empty program after `--`.
    let output = helper_command(runtime.path())
        .args([uid.to_string(), "--".into()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
}
