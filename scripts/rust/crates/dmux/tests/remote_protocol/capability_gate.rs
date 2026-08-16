//! P9 remote-Wez build/capability gate: positive owner hello report,
//! bounded no-server argv probes, exact controller/owner build equality,
//! and fail-closed legacy/missing/mismatch behavior without hiding explicit
//! tmux availability.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dmux::error::ErrorCode;
use dmux::model::{Backend, BackendInstanceUid, ServerEpoch};
use dmux::remote::protocol::{self, HelloInfo};
use dmux::remote::wez_compat::{
    CAP_ACTIVATE_EXISTING, CAP_ATTACH_NO_CREATE, CAP_TMUX, CAP_WEZ, RemoteWezRefusal,
    WEZ_BUILD_PREFIX, WEZ_PATH_PREFIX, assess_automatic_remote_wez, probe_wezterm_capabilities,
    reported_remote_wezterm_path,
};
use serde_json::json;
use uuid::Uuid;

use crate::util::{Scratch, envelope, error_code};

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn positive_probe(dir: &Path, build: &str) -> PathBuf {
    let path = dir.join("fake-wezterm");
    executable(
        &path,
        &format!(
            r#"#!/bin/sh
if test -n "${{WEZTERM_UNIX_SOCKET-}}" || test -n "${{WEZTERM_PANE-}}" || test -n "${{TMUX-}}" || test -n "${{TMUX_PANE-}}"; then
  echo ambient-mux-env-leaked >&2
  exit 91
fi
printf '%s\n' "$*" >> "$0.calls"
case "$*" in
  '--version')
    printf '%s\n' 'wezterm {build}'
    ;;
  '--skip-config start --help')
    printf '%s\n' '--always-new-process --domain DOMAIN --attach'
    ;;
  *)
    echo unexpected-argv >&2
    exit 92
    ;;
esac
"#
        ),
    );
    path
}

fn report(build: &str) -> Vec<String> {
    vec![
        CAP_WEZ.into(),
        format!("{WEZ_BUILD_PREFIX}{build}"),
        format!("{WEZ_PATH_PREFIX}/opt/dmux-test/wezterm"),
        CAP_ATTACH_NO_CREATE.into(),
        CAP_ACTIVATE_EXISTING.into(),
    ]
}

fn write_descriptor(
    scratch: &Scratch,
    state: &str,
    instance: BackendInstanceUid,
    epoch: ServerEpoch,
    socket: &str,
) {
    let path = scratch.locks.path().join("wez-dmux.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "descriptor_version": 1,
            "state": state,
            "epoch": epoch,
            "pid": 4242,
            "socket": socket,
            "start_token": "test-process-witness",
            "boot_nonce": Uuid::new_v4(),
            "backend_instance_uid": instance,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn never_contacted_bin(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let marker = scratch.data.path().join("provider-was-contacted");
    let bin = scratch.data.path().join("must-not-run-wezterm");
    executable(
        &bin,
        &format!(
            "#!/bin/sh\nprintf contacted > {}\nexit 93\n",
            marker.display()
        ),
    );
    (bin, marker)
}

fn wez_new_request() -> dmux::remote::protocol::Envelope {
    envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        json!({
            "name": "must-not-create",
            "backend": "wez",
            "program": ["sleep", "30"],
        }),
    )
}

#[test]
fn positive_probe_is_exact_bounded_read_only_argv() {
    let scratch = tempfile::tempdir().unwrap();
    let bin = positive_probe(scratch.path(), "20260816-143635-72f3fd75");
    let probed = probe_wezterm_capabilities(bin.to_str().unwrap(), Duration::from_secs(5)).unwrap();

    assert_eq!(probed.build, "20260816-143635-72f3fd75");
    assert_eq!(
        probed.wezterm_path,
        fs::canonicalize(&bin).unwrap().display().to_string()
    );
    assert_eq!(
        probed.capabilities,
        vec![
            CAP_WEZ.into(),
            format!("{WEZ_BUILD_PREFIX}{}", probed.build),
            format!("{WEZ_PATH_PREFIX}{}", probed.wezterm_path),
            CAP_ATTACH_NO_CREATE.into(),
            CAP_ACTIVATE_EXISTING.into(),
        ]
    );
    assert_eq!(
        fs::read_to_string(bin.with_extension("calls")).unwrap(),
        "--version\n--skip-config start --help\n"
    );
}

#[test]
fn probe_timeout_kills_the_fixed_child() {
    let scratch = tempfile::tempdir().unwrap();
    let bin = scratch.path().join("hung-wezterm");
    executable(&bin, "#!/bin/sh\nexec sleep 5\n");
    let started = Instant::now();
    let error =
        probe_wezterm_capabilities(bin.to_str().unwrap(), Duration::from_millis(30)).unwrap_err();
    assert!(error.to_string().contains("deadline"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn probe_bound_includes_descendants_that_inherit_capture_pipes() {
    let scratch = tempfile::tempdir().unwrap();
    let bin = scratch.path().join("descendant-wezterm");
    executable(
        &bin,
        "#!/bin/sh\n(sleep 5) &\ncase \"$*\" in\n  '--version') echo 'wezterm build-a' ;;\n  *) echo '--always-new-process --domain DOMAIN --attach' ;;\nesac\n",
    );
    let started = Instant::now();
    let report =
        probe_wezterm_capabilities(bin.to_str().unwrap(), Duration::from_millis(500)).unwrap();
    assert_eq!(report.build, "build-a");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "inherited capture pipes escaped the bounded probe"
    );
}

#[test]
fn owner_hello_reports_exact_build_and_presentation_tokens_once() {
    let scratch = Scratch::new("wez-build-hello");
    scratch
        .registry()
        .register_backend_instance(Backend::Wez, Some("/not-contacted.sock"), None)
        .unwrap();
    let bin = positive_probe(scratch.data.path(), "20260816-143635-72f3fd75");
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let (code, response) =
        scratch.agent_env(&request, &[("DMUX_WEZ_BIN", bin.display().to_string())]);
    assert_eq!(code, 0, "{response:?}");
    let hello: HelloInfo = serde_json::from_value(response.payload.clone().unwrap()).unwrap();
    let expected = vec![
        CAP_WEZ.into(),
        format!("{WEZ_BUILD_PREFIX}20260816-143635-72f3fd75"),
        format!(
            "{WEZ_PATH_PREFIX}{}",
            fs::canonicalize(&bin).unwrap().display()
        ),
        CAP_ATTACH_NO_CREATE.into(),
        CAP_ACTIVATE_EXISTING.into(),
    ];
    for capability in &expected {
        assert!(response.capabilities.contains(capability), "{response:?}");
        assert!(hello.capabilities.contains(capability), "{hello:?}");
    }
    assert_eq!(response.capabilities, hello.capabilities);
    assert_eq!(
        fs::read_to_string(bin.with_extension("calls")).unwrap(),
        "--version\n--skip-config start --help\n",
        "one agent request probes once; envelope and hello reuse the report"
    );
}

#[test]
fn ordinary_rpc_keeps_coarse_caps_without_spawning_probe_children() {
    let scratch = Scratch::new("wez-build-non-hello");
    scratch
        .registry()
        .register_backend_instance(Backend::Wez, Some("/not-contacted.sock"), None)
        .unwrap();
    let bin = positive_probe(scratch.data.path(), "build-a");
    let request = envelope("frobnicate", Uuid::new_v4(), json!({}));
    let (code, response) =
        scratch.agent_env(&request, &[("DMUX_WEZ_BIN", bin.display().to_string())]);
    assert_eq!(code, 2, "{response:?}");
    assert!(response.capabilities.iter().any(|cap| cap == CAP_WEZ));
    assert!(
        !response
            .capabilities
            .iter()
            .any(|cap| cap.starts_with(WEZ_BUILD_PREFIX))
    );
    assert!(
        !bin.with_extension("calls").exists(),
        "only the hello handshake may run the positive Wez probes"
    );
}

#[test]
fn failed_positive_probe_does_not_claim_wez_support() {
    let scratch = Scratch::new("wez-build-negative");
    scratch
        .registry()
        .register_backend_instance(Backend::Wez, Some("/not-contacted.sock"), None)
        .unwrap();
    let bin = scratch.data.path().join("fake-wezterm-missing-attach");
    executable(
        &bin,
        "#!/bin/sh\ncase \"$*\" in\n  '--version') echo 'wezterm build-a' ;;\n  *) echo '--domain DOMAIN' ;;\nesac\n",
    );
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let (code, response) =
        scratch.agent_env(&request, &[("DMUX_WEZ_BIN", bin.display().to_string())]);
    assert_eq!(code, 0, "{response:?}");
    assert!(!response.capabilities.iter().any(|cap| cap == CAP_WEZ));
    assert!(
        !response
            .capabilities
            .iter()
            .any(|cap| cap.starts_with(WEZ_BUILD_PREFIX))
    );
    assert!(!response.capabilities.contains(&CAP_ATTACH_NO_CREATE.into()));
}

#[test]
fn automatic_remote_wez_requires_exact_build_and_all_tokens() {
    let controller = report("build-a");
    let mut owner = report("build-a");
    owner.push(CAP_TMUX.into());
    let accepted = assess_automatic_remote_wez(&controller, &owner);
    assert!(accepted.is_eligible(), "{accepted:?}");
    assert_eq!(accepted.build.as_deref(), Some("build-a"));
    assert!(accepted.explicit_tmux_available);
    assert!(accepted.typed_error().is_none());

    let mut mismatched = report("build-b");
    mismatched.push(CAP_TMUX.into());
    let mismatch = assess_automatic_remote_wez(&controller, &mismatched);
    assert_eq!(
        mismatch.refusal,
        Some(RemoteWezRefusal::BuildMismatch {
            controller: "build-a".into(),
            owner: "build-b".into(),
        })
    );
    assert!(mismatch.explicit_tmux_available);
    let error = mismatch.typed_error().unwrap();
    assert_eq!(error.code, ErrorCode::VersionMismatch);
    assert!(error.message.contains("automatic fallback is forbidden"));
    assert!(error.message.contains("--backend tmux"));

    let legacy = vec![CAP_WEZ.into(), CAP_TMUX.into()];
    let legacy = assess_automatic_remote_wez(&controller, &legacy);
    assert_eq!(legacy.refusal, Some(RemoteWezRefusal::OwnerBuildMissing));
    assert_eq!(
        legacy.typed_error().unwrap().code,
        ErrorCode::VersionMismatch
    );
    assert!(legacy.explicit_tmux_available);

    let mut missing = report("build-a");
    missing.retain(|cap| cap != CAP_ACTIVATE_EXISTING);
    missing.push(CAP_TMUX.into());
    let missing = assess_automatic_remote_wez(&controller, &missing);
    assert_eq!(
        missing.refusal,
        Some(RemoteWezRefusal::OwnerCapabilityMissing(
            CAP_ACTIVATE_EXISTING.into()
        ))
    );
    assert_eq!(
        missing.typed_error().unwrap().code,
        ErrorCode::ProviderUnavailable
    );
    assert!(missing.explicit_tmux_available);
}

#[test]
fn duplicate_or_malformed_build_reports_fail_closed() {
    let controller = report("build-a");
    let mut duplicate = report("build-a");
    duplicate.push(format!("{WEZ_BUILD_PREFIX}build-a"));
    duplicate.push(CAP_TMUX.into());
    let duplicate = assess_automatic_remote_wez(&controller, &duplicate);
    assert!(matches!(
        duplicate.refusal,
        Some(RemoteWezRefusal::OwnerBuildMalformed(_))
    ));
    assert_eq!(
        duplicate.typed_error().unwrap().code,
        ErrorCode::VersionMismatch
    );

    let mut malformed = report("build-a");
    malformed.retain(|cap| !cap.starts_with(WEZ_BUILD_PREFIX));
    malformed.push(format!("{WEZ_BUILD_PREFIX}bad build"));
    let malformed = assess_automatic_remote_wez(&controller, &malformed);
    assert!(matches!(
        malformed.refusal,
        Some(RemoteWezRefusal::OwnerBuildMalformed(_))
    ));
    assert!(!malformed.explicit_tmux_available);
}

#[test]
fn remote_wezterm_path_is_exactly_one_strict_absolute_fact() {
    let caps = report("build-a");
    assert_eq!(
        reported_remote_wezterm_path(&caps).unwrap().as_deref(),
        Some("/opt/dmux-test/wezterm")
    );

    for bad in [
        "wez:path:wezterm",
        "wez:path:/opt/../usr/bin/wezterm",
        "wez:path:/usr//bin/wezterm",
        "wez:path:/usr/bin/wezterm\nforged",
    ] {
        assert!(
            reported_remote_wezterm_path(&[bad.into()]).is_err(),
            "accepted {bad:?}"
        );
    }
    let duplicate = vec![
        "wez:path:/usr/bin/wezterm".into(),
        "wez:path:/opt/bin/wezterm".into(),
    ];
    assert!(reported_remote_wezterm_path(&duplicate).is_err());

    let mut missing = report("build-a");
    missing.retain(|cap| !cap.starts_with(WEZ_PATH_PREFIX));
    let refusal = assess_automatic_remote_wez(&report("build-a"), &missing);
    assert_eq!(refusal.refusal, Some(RemoteWezRefusal::OwnerPathMissing));
    assert_eq!(
        refusal.typed_error().unwrap().code,
        ErrorCode::ProviderUnavailable
    );
}

#[test]
fn absent_or_starting_descriptor_refuses_before_any_wez_provider_probe() {
    let scratch = Scratch::new("wez-descriptor-not-ready");
    let socket = scratch.data.path().join("expected.sock");
    let socket = socket.to_str().unwrap();
    let epoch = ServerEpoch(Uuid::new_v4());
    let mut registry = scratch.registry();
    let instance = registry
        .register_backend_instance(Backend::Wez, Some(socket), None)
        .unwrap();
    registry
        .publish_backend_server(
            instance,
            epoch,
            Some(4242),
            Some("test-process-witness"),
            None,
            None,
        )
        .unwrap();
    drop(registry);
    let (bin, marker) = never_contacted_bin(&scratch);
    let seams = [("DMUX_WEZ_BIN", bin.display().to_string())];

    let (code, response) = scratch.agent_env(&wez_new_request(), &seams);
    assert_eq!(code, 6, "{response:?}");
    assert_eq!(error_code(&response), "provider_unavailable");
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("descriptor is absent")
    );
    assert!(
        !marker.exists(),
        "an absent descriptor must fail before argv"
    );

    write_descriptor(&scratch, "starting", instance, epoch, socket);
    let (code, response) = scratch.agent_env(&wez_new_request(), &seams);
    assert_eq!(code, 6, "{response:?}");
    assert_eq!(error_code(&response), "provider_unavailable");
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("recovering/not ready")
    );
    assert!(
        !marker.exists(),
        "a starting descriptor must fail before any provider probe"
    );
}

#[test]
fn descriptor_identity_mismatches_fail_before_any_wez_provider_probe() {
    for mismatch in ["instance", "socket", "epoch"] {
        let scratch = Scratch::new(mismatch);
        let socket_path = scratch.data.path().join("expected.sock");
        let socket = socket_path.to_str().unwrap();
        let published_epoch = ServerEpoch(Uuid::new_v4());
        let mut registry = scratch.registry();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some(socket), None)
            .unwrap();
        registry
            .publish_backend_server(
                instance,
                published_epoch,
                Some(4242),
                Some("test-process-witness"),
                None,
                None,
            )
            .unwrap();
        drop(registry);

        let descriptor_instance = if mismatch == "instance" {
            BackendInstanceUid(Uuid::new_v4())
        } else {
            instance
        };
        let descriptor_epoch = if mismatch == "epoch" {
            ServerEpoch(Uuid::new_v4())
        } else {
            published_epoch
        };
        let descriptor_socket = if mismatch == "socket" {
            scratch.data.path().join("other.sock")
        } else {
            socket_path.clone()
        };
        write_descriptor(
            &scratch,
            "ready",
            descriptor_instance,
            descriptor_epoch,
            descriptor_socket.to_str().unwrap(),
        );
        let (bin, marker) = never_contacted_bin(&scratch);
        let (code, response) = scratch.agent_env(
            &wez_new_request(),
            &[("DMUX_WEZ_BIN", bin.display().to_string())],
        );
        assert_eq!(code, 1, "{mismatch}: {response:?}");
        let expected = if mismatch == "epoch" {
            "backend_epoch_changed"
        } else {
            "wrong_backend_instance"
        };
        assert_eq!(error_code(&response), expected, "{mismatch}");
        assert!(
            !marker.exists(),
            "{mismatch} mismatch reached the Wez provider"
        );
    }
}
