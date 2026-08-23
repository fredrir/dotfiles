//! Acceptance case 29 (plan §13.3, §20.2), the Rust half: `_gui space-new`
//! from a pane whose marker names a REMOTE owner's Wez Space selects the
//! marker's backend and dispatches the create as a remote `NEW` to the
//! marker's owner — here a scratch owner reached through the real `_agent`
//! binary over a local script transport, the way `tests/remote_protocol`
//! reaches it — and writes nothing into the controller's own registry. The
//! Lua half (`tests/actions_mac_keys.lua`) proves the CMD|SHIFT t chord hands
//! that marker to the controller byte for byte. Live two-host evidence stays
//! with WS-G.5.
//!
//! The owner here publishes a Wez instance row but runs no mux server, so
//! the owner's own verification matrix refuses the create after it has
//! arrived; what the test pins is the dispatch — which owner, which backend,
//! which incarnation — and that nothing is minted on either side.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use dmux::bootstrap::MarkerContext;
use dmux::error::ErrorCode;
use dmux::gui::{BridgeDomainState, BridgeHeartbeat, BridgePane, BridgeSelection, GuiCliOrigin};
use dmux::gui_cli::{BoundGuiOrigin, GuiAuthority, GuiCommand, ProductionGuiAuthority};
use dmux::model::{Backend, ServerEpoch, SpaceNo, SpaceUid};
use dmux::operations::{HierarchyGroup, HierarchySplit, OperationEnv, SpaceHierarchy};
use dmux::registry::{NetworkClass, Registry, RegistryConfig, RouteRow, RouteSpec, Transport};
use dmux::remote::client::{AgentInvocation, RouteInvoker};
use serde_json::Value;
use uuid::Uuid;

const WEZTERM: &str = "/opt/homebrew/bin/wezterm";
const DMUX_BIN: &str = env!("CARGO_BIN_EXE_dmux");

struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Scratch {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("locks")).unwrap();
        std::fs::set_permissions(
            dir.path().join("locks"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        Scratch { dir }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    fn locks(&self) -> PathBuf {
        self.dir.path().join("locks")
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.data().join("registry.sqlite3"),
            lock_dir: self.locks(),
        }
    }

    fn registry(&self) -> Registry {
        let env = self.env();
        Registry::open(RegistryConfig::new(env.db_path, env.lock_dir)).unwrap()
    }

    fn space_no_counter(&self) -> i64 {
        rusqlite::Connection::open_with_flags(
            self.env().db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT space_no_counter FROM meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }
}

/// The local agent invoker: every route attempt runs one script that copies
/// the request document into a per-method capture file and forwards it to
/// the REAL `dmux _agent` of the scratch owner, with the owner's WezTerm
/// probe pointed at the installed binary (the owner-side seam production
/// never sets).
struct OwnerInvoker {
    script: PathBuf,
}

impl OwnerInvoker {
    fn new(owner: &Scratch, captures: &Path) -> OwnerInvoker {
        let script = owner.data().join("owner-agent.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nmethod=\"$1\"\ntee -a '{captures}/'\"$method\" | \
                 DMUX_WEZ_BIN='{WEZTERM}' exec '{DMUX_BIN}' _agent --protocol 1 \"$method\" \
                 --data-dir '{data}' --lock-dir '{locks}'\n",
                captures = captures.display(),
                data = owner.data().display(),
                locks = owner.locks().display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        OwnerInvoker { script }
    }
}

impl RouteInvoker for OwnerInvoker {
    fn argv_for(&self, _route: &RouteRow, invocation: &AgentInvocation) -> Vec<String> {
        vec![self.script.display().to_string(), invocation.method.clone()]
    }
}

fn captured(captures: &Path, method: &str) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(captures.join(method)) else {
        return Vec::new();
    };
    serde_json::Deserializer::from_str(&text)
        .into_iter::<Value>()
        .map(|document| document.expect("one request envelope per call"))
        .collect()
}

#[test]
fn space_new_from_a_remote_wez_marker_dispatches_a_wez_new_to_the_markers_owner() {
    if !Path::new(WEZTERM).exists() {
        eprintln!(
            "skipping: {WEZTERM} is not installed (the remote Wez compatibility gate probes it)"
        );
        return;
    }

    // The owner (Archie): a published Wez instance row, no server.
    let owner = Scratch::new();
    let owner_uid;
    let instance;
    let epoch = ServerEpoch(Uuid::new_v4());
    {
        let mut registry = owner.registry();
        owner_uid = registry.identity().unwrap().host_uid;
        let socket = owner.locks().join("dmux").join("wez-dmux.sock");
        instance = registry
            .register_backend_instance(Backend::Wez, Some(socket.to_str().unwrap()), None)
            .unwrap();
        registry
            .publish_backend_server(
                instance,
                epoch,
                Some(i64::from(std::process::id())),
                Some("owner-start-token"),
                None,
                None,
            )
            .unwrap();
    }
    let owner_head = owner.registry().authority_head().unwrap();

    // The controller (Macie): the owner enrolled, one Tailscale route carrying
    // the Wez domain name the GUI imported.
    let client = Scratch::new();
    let captures = client.data().join("captures");
    std::fs::create_dir_all(&captures).unwrap();
    let (alias, label) = {
        let mut registry = client.registry();
        let enrolled = registry.enroll_host(owner_uid, Some("archie")).unwrap();
        registry
            .upsert_route(&RouteSpec {
                host_uid: owner_uid,
                transport: Transport::Openssh,
                endpoint: "archie.tailnet".into(),
                username: Some("fredrir".into()),
                wez_domain: Some("dmux-b-tailscale".into()),
                network_class: NetworkClass::Tailscale,
                priority: 20,
                required_capability: None,
                trust_fingerprint: None,
                enabled: true,
            })
            .unwrap();
        (enrolled.alias, "archie".to_string())
    };
    let client_head = client.registry().authority_head().unwrap();
    let client_counter = client.space_no_counter();

    // The pane marker: an Archie Wez Space, attached through the Tailscale
    // domain, exactly as the Lua half hands it over.
    let marker = MarkerContext {
        host_uid: owner_uid,
        space_uid: SpaceUid(Uuid::new_v4()),
        space_no: SpaceNo(NonZeroU64::new(2).unwrap()),
        backend: Backend::Wez,
        domain: Some("dmux-b-tailscale".into()),
        server_epoch: epoch,
        group_ref: format!("g{}.wz-7", epoch.0),
        split_ref: format!("p{}.wz-9", epoch.0),
    };
    let origin = GuiCliOrigin {
        protocol_version: 1,
        gui_instance: "gui-test-1".into(),
        pane_id: 51,
        domain: "dmux-b-tailscale".into(),
        tmux_client_uid: None,
        marker: marker.clone(),
    };
    let mut domains = BTreeMap::new();
    domains.insert(
        "dmux-b-tailscale".to_string(),
        BridgeDomainState {
            state: "Attached".into(),
            has_any_panes: true,
            backend_instance_uid: Some(instance),
            pane_count: 1,
            valid_marker_pane_count: 1,
            system_pane_count: 0,
            system_workspace: None,
            system_epoch: None,
        },
    );
    let bound = BoundGuiOrigin::remote_wez_for_test(
        origin,
        BridgeSelection {
            gui_instance: "gui-test-1".into(),
            pid: std::process::id(),
            process_start_token: "test-token".into(),
            pane_id: 51,
            domain: "dmux-b-tailscale".into(),
        },
        BridgeHeartbeat {
            protocol_version: 1,
            gui_instance: "gui-test-1".into(),
            pid: std::process::id(),
            process_start_token: "test-token".into(),
            updated_at: 1,
            panes: vec![BridgePane {
                pane_id: 51,
                domain: "dmux-b-tailscale".into(),
                tmux_client_uid: None,
                context: marker.clone(),
            }],
            domains,
        },
        instance,
        "project".into(),
        SpaceHierarchy {
            space_uid: marker.space_uid,
            server_epoch: epoch,
            groups: vec![HierarchyGroup {
                group_ref: marker.group_ref.clone(),
                title: Some("editor".into()),
                splits: vec![HierarchySplit {
                    split_ref: marker.split_ref.clone(),
                    title: None,
                    cwd: None,
                }],
            }],
        },
        alias,
        label,
        "dmux-b-tailscale".into(),
    );

    let runtime = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut production = ProductionGuiAuthority::with_dependencies(
        client.env(),
        runtime.path().to_path_buf(),
        state.path().to_path_buf(),
        WEZTERM.into(),
        "/dev/null".into(),
        PathBuf::from("/dev/null"),
        "/bin/false".into(),
        OwnerInvoker::new(&owner, &captures),
    );
    let result = production.execute_bound(
        &bound,
        &GuiCommand::SpaceNew {
            name: "proj".into(),
            dir: None,
            tmux_client_uid: None,
        },
    );

    // The create was dispatched to the marker's owner as a Wez NEW carrying
    // the marker's exact incarnation, after the owner's compatibility hello.
    let hellos = captured(&captures, "hello");
    assert!(
        !hellos.is_empty(),
        "the remote domain manifest is built from a fresh owner hello"
    );
    let news = captured(&captures, "new");
    assert_eq!(news.len(), 1, "exactly one NEW reached the owner: {news:?}");
    let new = &news[0];
    assert_eq!(new["method"], "new");
    assert_eq!(new["payload"]["backend"], "wez", "{new}");
    assert_eq!(new["payload"]["name"], "proj", "{new}");
    assert_eq!(new["payload"]["allow_name_collision"], false, "{new}");
    assert_eq!(new["backend_instance_uid"], instance.0.to_string(), "{new}");
    assert_eq!(new["server_epoch"], epoch.0.to_string(), "{new}");
    for method in ["tmux_client_status", "tmux_client_refresh", "attach_plan"] {
        assert!(
            captured(&captures, method).is_empty(),
            "a tmux path was taken for a Wez marker: {method}"
        );
    }

    // The owner runs no mux server, so its own verification matrix refuses
    // the create AFTER it arrived: a typed refusal, never a tmux fallback
    // and never a create on the controller.
    let error = match result {
        Ok(value) => panic!("the owner has no Wez server to create on: {value}"),
        Err(error) => error,
    };
    assert_eq!(
        error.code,
        ErrorCode::ProviderUnavailable,
        "the owner's verified_wez_target refuses a missing descriptor: {}",
        error.message
    );
    assert!(
        error.message.contains("managed Wez descriptor is absent"),
        "the refusal is the owner's own Wez verification, not a transport or GUI fault: {}",
        error.message
    );
    assert!(
        production.take_partial_result().is_none(),
        "nothing durable was created, so no partial result may be retained"
    );

    // Neither registry minted anything: the controller's stays as enrolled,
    // the owner's refused before any reservation.
    let registry = client.registry();
    assert!(registry.spaces().unwrap().is_empty());
    assert_eq!(registry.authority_head().unwrap(), client_head);
    assert_eq!(client.space_no_counter(), client_counter);
    let registry = owner.registry();
    assert!(registry.spaces().unwrap().is_empty());
    assert_eq!(registry.authority_head().unwrap(), owner_head);
}
