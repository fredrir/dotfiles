//! P8b wez leg: the same remote hierarchy conformance against a scratch
//! STOCK wezterm-mux-server, driven through `_agent` with the owner-side
//! env seams (`DMUX_WEZ_BIN`/`DMUX_WEZ_CONFIG`/`DMUX_HELPER_BIN`).
//!
//! ADR 009 §4a pin: the wez mux server does NOT propagate server env into
//! panes, so the real helper is wrapped in a shim exporting
//! DMUX_RUNTIME_DIR (production is unaffected — the helper resolves the
//! runtime dir itself via confstr). The shim rides the owner-side
//! DMUX_HELPER_BIN seam; the client payloads never carry owner paths.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use dmux::backend::{InventoryOutcome, InventoryScope, Provider};
use dmux::model::{Backend, ChildKind, ServerEpoch};
use dmux::operations::{CreatedChild, CreatedSpace, SpaceHierarchy};
use dmux::remote::protocol::{self, SpacesInfo};
use serde_json::json;
use uuid::Uuid;

use crate::util::{Scratch, envelope, error_code};

const WEZ_MUX: &str = "/opt/homebrew/bin/wezterm-mux-server";
const WEZ_BIN: &str = "/opt/homebrew/bin/wezterm";

struct WezScratch {
    server: std::process::Child,
    socket: String,
    config: String,
    epoch: Uuid,
    dir: tempfile::TempDir,
}

impl WezScratch {
    /// Scratch stock mux server with a sentinel workspace (the pattern the
    /// P8a wez leg pinned): unix domain on a private socket, no auto-serve.
    fn start(tag: &str, runtime_dir: &Path) -> WezScratch {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let socket = dir.path().join("sock").display().to_string();
        let epoch = Uuid::new_v4();
        let config_path = dir.path().join("mux.lua");
        std::fs::write(
            &config_path,
            format!(
                r#"local wezterm = require 'wezterm'
local config = wezterm.config_builder and wezterm.config_builder() or {{}}
config.unix_domains = {{ {{ name = 'p8b{tag}', socket_path = os.getenv('DMUX_SOCKET'),
                            no_serve_automatically = true }} }}
config.default_prog = {{ '/bin/sh', '-c', 'echo DMUX-CANARY; sleep 600' }}
wezterm.on('mux-startup', function()
  wezterm.mux.spawn_window {{
    workspace = 'dmux:system:{epoch}',
    args = {{ '/bin/sh', '-c', 'while :; do sleep 3600; done' }},
  }}
end)
return config
"#
            ),
        )
        .unwrap();
        let server = Command::new(WEZ_MUX)
            .args(["--config-file", config_path.to_str().unwrap()])
            .env("DMUX_SOCKET", &socket)
            .env("DMUX_RUNTIME_DIR", runtime_dir)
            .env_remove("WEZTERM_UNIX_SOCKET")
            .spawn()
            .unwrap();
        WezScratch {
            server,
            socket,
            config: config_path.display().to_string(),
            epoch,
            dir,
        }
    }
}

impl Drop for WezScratch {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = std::fs::remove_dir_all(self.dir.path());
    }
}

fn marker_program(marker: &Path) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!(
            "printf %s \"$DMUX_GROUP_REF|$DMUX_SPLIT_REF|$DMUX_SPACE_UID\" > {} \
             && exec sleep 300",
            marker.display()
        ),
    ]
}

fn wait_marker(marker: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(text) = std::fs::read_to_string(marker)
            && !text.is_empty()
        {
            return text;
        }
        assert!(Instant::now() < deadline, "helper never exec'd the program");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn wez_remote_hierarchy_full_cycle_through_the_agent() {
    if !Path::new(WEZ_MUX).exists() {
        eprintln!("SKIP wez_agent: stock wezterm-mux-server not installed at {WEZ_MUX}");
        return;
    }
    // Keep a real managed tmux instance live as the opposite provider. This
    // makes every Wez NEW below exercise the owner-fenced cross-backend seam
    // (the owner scans both; the controller never guesses).
    let scratch = Scratch::with_tmux("wez-agent");
    let wez = WezScratch::start("a", scratch.locks.path());

    // Owner-side setup: register the wez instance with the scratch socket
    // (the registry row is where the agent resolves the endpoint from).
    let mut registry = scratch.registry();
    let instance = registry
        .register_backend_instance(Backend::Wez, Some(&wez.socket), None)
        .unwrap();
    drop(registry);

    // Helper shim exporting the runtime-dir seam (ADR 009 §4a).
    let shim = scratch.data.path().join("helper-shim.sh");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nexport DMUX_RUNTIME_DIR={}\nexec {} \"$@\"\n",
            scratch.locks.path().display(),
            env!("CARGO_BIN_EXE_pane-bootstrap"),
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let seams: Vec<(&str, String)> = vec![
        ("DMUX_WEZ_BIN", WEZ_BIN.to_string()),
        ("DMUX_WEZ_CONFIG", wez.config.clone()),
        ("DMUX_HELPER_BIN", shim.display().to_string()),
    ];

    // Wait for the sentinel-epoched server to answer a complete scan.
    let provider = dmux::backend::wez::WezProvider::new(WEZ_BIN, wez.config.clone());
    let scope = InventoryScope {
        backend: Backend::Wez,
        endpoint: wez.socket.clone(),
        expected_epoch: None,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let InventoryOutcome::Complete(inv) = provider.inventory(&scope)
            && inv.server_epoch == Some(ServerEpoch(wez.epoch))
        {
            break;
        }
        assert!(Instant::now() < deadline, "mux server never became ready");
        std::thread::sleep(Duration::from_millis(100));
    }

    // P10 fail-fast identity seam: production learns this exact ready
    // descriptor from the service coordinator. The scratch stock server
    // cannot write dmux's descriptor itself, so publish the same identity
    // explicitly before any owner RPC is allowed to probe it.
    let mut registry = scratch.registry();
    registry
        .publish_backend_server(
            instance,
            ServerEpoch(wez.epoch),
            Some(wez.server.id().into()),
            Some("wez-agent-scratch"),
            None,
            None,
        )
        .unwrap();
    drop(registry);
    let descriptor = scratch.locks.path().join("wez-dmux.json");
    std::fs::write(
        &descriptor,
        serde_json::to_vec(&json!({
            "descriptor_version": 1,
            "state": "ready",
            "epoch": wez.epoch,
            "pid": wez.server.id(),
            "socket": wez.socket,
            "start_token": "wez-agent-scratch",
            "boot_nonce": Uuid::new_v4(),
            "backend_instance_uid": instance,
        }))
        .unwrap(),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&descriptor, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    // An unmanaged exact-name row on the opposite provider refuses before
    // Wez identity reservation or native allocation.
    let native = scratch.tmux(&["new-session", "-d", "-s", "cross-collision"]);
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    let (code, response) = scratch.agent_env(
        &envelope(
            protocol::methods::NEW,
            Uuid::new_v4(),
            json!({ "name": "cross-collision", "backend": "wez" }),
        ),
        &seams,
    );
    assert_eq!(code, 4, "{response:?}");
    assert_eq!(error_code(&response), "name_conflict");
    assert!(scratch.registry().spaces().unwrap().is_empty());

    // --- `new` with backend=wez through the agent (P8b lifts the v1
    // refusal): opaque workspace key, real helper, marker proven.
    let mark = scratch.data.path().join("wm");
    let (code, response) = scratch.agent_env(
        &envelope(
            protocol::methods::NEW,
            Uuid::new_v4(),
            json!({ "name": "proj", "backend": "wez", "program": marker_program(&mark) }),
        ),
        &seams,
    );
    assert_eq!(code, 0, "{response:?}");
    let created: CreatedSpace = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(created.native_token.starts_with("dmux:"));
    let stamped = wait_marker(&mark);
    assert!(
        stamped.ends_with(&created.space_uid.0.to_string()),
        "{stamped}"
    );

    // --- spaces scan reports the wez backend as complete.
    let (code, response) = scratch.agent_env(
        &envelope(protocol::methods::SPACES, Uuid::new_v4(), json!({})),
        &seams,
    );
    assert_eq!(code, 0, "{response:?}");
    let info: SpacesInfo = serde_json::from_value(response.payload.unwrap()).unwrap();
    let scan = info
        .scans
        .iter()
        .find(|s| s.backend == Backend::Wez)
        .expect("wez scan present");
    assert_eq!(scan.outcome, "complete", "{scan:?}");
    assert!(scan.server_epoch.is_some());

    // --- hierarchy, group_new (real helper), split_new with placement.
    let (code, response) = scratch.agent_env(
        &envelope(
            protocol::methods::HIERARCHY,
            Uuid::new_v4(),
            json!({ "space_uid": created.space_uid }),
        ),
        &seams,
    );
    assert_eq!(code, 0, "{response:?}");
    let tree: SpaceHierarchy = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(tree.groups.len(), 1);

    let gmark = scratch.data.path().join("wg");
    let group_request = envelope(
        protocol::methods::GROUP_NEW,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "program": marker_program(&gmark) }),
    );
    let (code, response) = scratch.agent_env(&group_request, &seams);
    assert_eq!(code, 0, "{response:?}");
    let group: CreatedChild = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(group.kind, ChildKind::Group);
    assert_eq!(
        wait_marker(&gmark),
        format!(
            "{}|{}|{}",
            group.group_ref, group.split_ref, created.space_uid.0
        ),
        "wez marker propagation through the remote protocol"
    );

    // Envelope replay: no second tab.
    let (code, response) = scratch.agent_env(&group_request, &seams);
    assert_eq!(code, 0, "{response:?}");
    let replayed: CreatedChild = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.group_ref, group.group_ref);

    let smark = scratch.data.path().join("ws");
    let (code, response) = scratch.agent_env(
        &envelope(
            protocol::methods::SPLIT_NEW,
            Uuid::new_v4(),
            json!({
                "space_uid": created.space_uid,
                "group_ref": group.group_ref,
                "direction": "right",
                "percent": 40,
                "program": marker_program(&smark),
            }),
        ),
        &seams,
    );
    assert_eq!(code, 0, "{response:?}");
    let split: CreatedChild = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(split.group_ref, group.group_ref);
    wait_marker(&smark);

    let (code, response) = scratch.agent_env(
        &envelope(
            protocol::methods::HIERARCHY,
            Uuid::new_v4(),
            json!({ "space_uid": created.space_uid }),
        ),
        &seams,
    );
    assert_eq!(code, 0, "{response:?}");
    let tree: SpaceHierarchy = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(tree.groups.len(), 2);
    assert_eq!(tree.groups.iter().map(|g| g.splits.len()).sum::<usize>(), 3);

    // --- removes and the last-group cascade refusal.
    let (code, _) = scratch.agent_env(
        &envelope(
            protocol::methods::SPLIT_RM,
            Uuid::new_v4(),
            json!({ "space_uid": created.space_uid, "split_ref": split.split_ref }),
        ),
        &seams,
    );
    assert_eq!(code, 0);
    let (code, _) = scratch.agent_env(
        &envelope(
            protocol::methods::GROUP_RM,
            Uuid::new_v4(),
            json!({ "space_uid": created.space_uid, "group_ref": group.group_ref }),
        ),
        &seams,
    );
    assert_eq!(code, 0);
    let (code, response) = scratch.agent_env(
        &envelope(
            protocol::methods::HIERARCHY,
            Uuid::new_v4(),
            json!({ "space_uid": created.space_uid }),
        ),
        &seams,
    );
    assert_eq!(code, 0, "{response:?}");
    let tree: SpaceHierarchy = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(tree.groups.len(), 1, "back to the root group");
    let (code, response) = scratch.agent_env(
        &envelope(
            protocol::methods::GROUP_RM,
            Uuid::new_v4(),
            json!({ "space_uid": created.space_uid, "group_ref": tree.groups[0].group_ref }),
        ),
        &seams,
    );
    assert_eq!(code, 4, "{response:?}");
    assert_eq!(error_code(&response), "repair_required");
}
