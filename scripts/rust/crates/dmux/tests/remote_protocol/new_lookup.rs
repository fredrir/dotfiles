use std::path::Path;
use std::process::Command;

use dmux::error::ErrorCode;
use dmux::model::Backend;
use dmux::remote::protocol::{
    self, NewLookupBlockReason, NewLookupClass, NewLookupPayload, NewLookupResult,
};
use uuid::Uuid;

use super::util::{Scratch, envelope, error_code, stub_tmux};

fn lookup(name: &str) -> protocol::Envelope {
    envelope(
        protocol::methods::NEW_LOOKUP,
        Uuid::new_v4(),
        serde_json::to_value(NewLookupPayload { name: name.into() }).unwrap(),
    )
}

#[test]
fn owner_new_lookup_surfaces_unmanaged_exact_name_and_no_native_token() {
    let scratch = Scratch::with_tmux("new-lookup");
    let request = envelope(
        protocol::methods::NEW_LOOKUP,
        Uuid::new_v4(),
        serde_json::to_value(NewLookupPayload {
            name: "seed".into(),
        })
        .unwrap(),
    );
    let (status, response) = scratch.agent(&request);
    assert_eq!(status, 0, "{:?}", response.error);
    assert!(
        response
            .capabilities
            .iter()
            .any(|capability| capability == protocol::CAP_NEW_LOOKUP)
    );
    let payload = response.payload.unwrap();
    assert!(
        !payload.to_string().contains("native_token"),
        "NEW_LOOKUP must not expose provider-native IDs: {payload}"
    );
    let result: NewLookupResult = serde_json::from_value(payload).unwrap();
    assert_eq!(
        result.tmux,
        NewLookupClass::Blocking {
            reason: NewLookupBlockReason::UnmanagedSameName,
            space_uid: None,
        }
    );
    assert_eq!(result.wez, NewLookupClass::NoMatch);
}

#[test]
fn owner_new_lookup_rejects_instance_claims_because_it_spans_both_backends() {
    let scratch = Scratch::with_tmux("new-lookup-claim");
    let mut request = envelope(
        protocol::methods::NEW_LOOKUP,
        Uuid::new_v4(),
        serde_json::to_value(NewLookupPayload {
            name: "missing".into(),
        })
        .unwrap(),
    );
    request.backend_instance_uid = Some(dmux::model::BackendInstanceUid(Uuid::new_v4()));
    let (status, response) = scratch.agent(&request);
    assert_eq!(status, 2);
    assert_eq!(response.error.unwrap().code, ErrorCode::Usage);
}

/// Review finding #17 inverted (ADR 012 WS-A.5, `remote/agent.rs`
/// `owner_lookup_target`). A registered, addressable tmux instance whose
/// server epoch was never published — the row `_tmux-bootstrap` leaves
/// between registering and publishing — used to be scanned unpinned, so
/// `new_lookup` answered a determinate `no_match` ("name free") off whatever
/// server held the namespace. Now it is the epoch fault the client already
/// maps: no partition is answered, no provider command runs, and the client
/// can neither create nor connect on the owner's word. Publishing the
/// incarnation through the real bootstrap makes the identical rig answer
/// determinately again.
#[test]
fn owner_new_lookup_refuses_an_unpublished_instance_as_an_epoch_fault_never_name_free() {
    let mut scratch = Scratch::new("new-lookup-unpublished");
    let ns = format!("dmux-p7-nl-unpublished-{}", std::process::id());
    let out = Command::new("tmux")
        .args([
            "-L",
            &ns,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "seed",
        ])
        .env("DMUX_RUNTIME_DIR", scratch.locks.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "scratch tmux server: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    scratch.ns = Some(ns.clone());
    let instance = scratch
        .registry()
        .register_backend_instance(Backend::Tmux, Some(&ns), None)
        .unwrap();
    assert_eq!(
        scratch
            .registry()
            .backend_server(instance)
            .unwrap()
            .server_epoch,
        None,
        "registered and addressable, never published"
    );

    let (path, witness) = stub_tmux(&scratch);
    let (status, response) = scratch.agent_env(&lookup("free"), &[("PATH", path)]);
    assert_eq!(status, 1, "{response:?}");
    assert_eq!(error_code(&response), "backend_epoch_changed");
    let message = &response.error.as_ref().unwrap().message;
    assert!(
        message.contains("has published no server epoch"),
        "{response:?}"
    );
    assert!(message.contains(&instance.0.to_string()), "{response:?}");
    assert!(
        response.payload.is_none(),
        "neither partition may be answered: {response:?}"
    );
    assert!(
        !witness.exists(),
        "no provider command may run for an unpublished instance: {}",
        std::fs::read_to_string(&witness).unwrap_or_default()
    );

    // The same rig once the incarnation is published: determinate, and only
    // now is "free" free.
    scratch.bootstrap_tmux();
    let (status, response) = scratch.agent(&lookup("free"));
    assert_eq!(status, 0, "{response:?}");
    let result: NewLookupResult = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(result.tmux, NewLookupClass::NoMatch);
    assert_eq!(result.wez, NewLookupClass::NoMatch);
}

/// Review finding #20 (ADR 012 WS-A.12's first item): `owner_lookup_target`
/// used to build `Target { epoch: epoch.unwrap_or(ServerEpoch(Uuid::nil())) }`
/// when the registry had no epoch — inert only because `new_lookup` read
/// two other fields, while `Target.epoch` feeds every mutation in the same
/// file. `Target` is now built only by the two live-verified resolvers, and
/// nothing in the agent may fabricate a server epoch.
#[test]
fn the_agent_never_fabricates_a_server_epoch_from_the_nil_uuid() {
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/remote/agent.rs"))
            .unwrap();
    let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    for forbidden in [
        "ServerEpoch(Uuid::nil())",
        "unwrap_or(ServerEpoch(",
        "unwrap_or_else(||ServerEpoch(",
        "unwrap_or_default()",
    ] {
        assert!(
            !compact.contains(forbidden),
            "src/remote/agent.rs fabricates a server epoch: {forbidden}"
        );
    }
}

/// ADR 012 §10, A5-c's ratified deviation resolved through the
/// `operations::lookup_new_owner_fenced` seam: with only the OPPOSITE
/// instance unpublished, the owner answers the partition — the published
/// side scanned under its pin, the unpublished side `indeterminate`, no
/// provider command run for it — so the client's resolver can still select
/// an explicit backend's known live match (§8.2 step 7) while auto
/// selection refuses (case 7). A name the published side does not hold is
/// still the typed epoch fault, never "name free", and the create path
/// keeps refusing the unpublished opposite.
#[test]
fn owner_new_lookup_answers_the_partition_when_only_the_opposite_instance_is_unpublished() {
    use std::os::unix::fs::PermissionsExt;

    use dmux::resolve::{ClassSummary, NewLookup, lookup_for_new};

    let scratch = Scratch::with_tmux("new-lookup-opposite");
    let request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        serde_json::json!({ "name": "proj", "backend": "tmux", "program": ["sleep", "300"] }),
    );
    let (status, response) = scratch.agent(&request);
    assert_eq!(status, 0, "{response:?}");
    let created: dmux::operations::CreatedSpace =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    let sessions = scratch.session_names();

    // The opposite instance: registered and addressable, never published.
    // Its binary is a witness that must not run.
    let wez_instance = scratch
        .registry()
        .register_backend_instance(Backend::Wez, Some("/tmp/dmux-p7-never-a-wez.sock"), None)
        .unwrap();
    let stub_dir = scratch.data.path().join("wez-stub");
    std::fs::create_dir_all(&stub_dir).unwrap();
    let witness = scratch.data.path().join("wezterm-ran");
    let stub = stub_dir.join("wezterm");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexit 1\n",
            witness.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let seams = [("DMUX_WEZ_BIN", stub.display().to_string())];

    let (status, response) = scratch.agent_env(&lookup("proj"), &seams);
    assert_eq!(status, 0, "{response:?}");
    let result: NewLookupResult = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(
        result.tmux,
        NewLookupClass::Selectable {
            space_uid: created.space_uid,
            space_no: created.space_no.get(),
        }
    );
    assert_eq!(result.wez, NewLookupClass::Indeterminate);
    assert!(
        !witness.exists(),
        "no provider command may run for an unpublished instance: {}",
        std::fs::read_to_string(&witness).unwrap_or_default()
    );

    // The client's resolver on exactly that partition.
    let wez = ClassSummary::Indeterminate;
    let tmux = ClassSummary::Selectable {
        space: created.space_uid,
        no: created.space_no,
    };
    assert!(matches!(
        lookup_for_new(Some(Backend::Tmux), false, wez, tmux),
        NewLookup::Connect { backend: Backend::Tmux, space, .. } if space == created.space_uid
    ));
    assert!(matches!(
        lookup_for_new(None, false, wez, tmux),
        NewLookup::Indeterminate {
            backend: Backend::Wez
        }
    ));
    assert!(matches!(
        lookup_for_new(Some(Backend::Wez), false, wez, tmux),
        NewLookup::Indeterminate {
            backend: Backend::Wez
        }
    ));

    // A name the published side does not hold: the epoch fault, never
    // "name free".
    let (status, response) = scratch.agent_env(&lookup("free"), &seams);
    assert_eq!(status, 1, "{response:?}");
    assert_eq!(error_code(&response), "backend_epoch_changed");
    let message = &response.error.as_ref().unwrap().message;
    assert!(
        message.contains("has published no server epoch")
            && message.contains(&wez_instance.0.to_string()),
        "{response:?}"
    );
    assert!(response.payload.is_none(), "{response:?}");
    assert!(!witness.exists());

    // The create path still refuses the unpublished opposite: nothing is
    // created on the published side either.
    let request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        serde_json::json!({ "name": "free", "backend": "tmux", "program": ["sleep", "300"] }),
    );
    let (status, response) = scratch.agent_env(&request, &seams);
    assert_ne!(status, 0, "{response:?}");
    assert_eq!(scratch.session_names(), sessions, "a create reached tmux");
    assert_eq!(scratch.registry().spaces().unwrap().len(), 1);
}
