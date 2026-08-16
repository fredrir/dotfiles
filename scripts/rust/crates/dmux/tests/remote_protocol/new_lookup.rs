use dmux::error::ErrorCode;
use dmux::remote::protocol::{
    self, NewLookupBlockReason, NewLookupClass, NewLookupPayload, NewLookupResult,
};
use uuid::Uuid;

use super::util::{Scratch, envelope};

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
