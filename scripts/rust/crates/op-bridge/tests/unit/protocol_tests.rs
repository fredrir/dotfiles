use super::*;

#[test]
fn the_vault_is_the_first_path_segment() {
    assert_eq!(vault("op://Dev/pi/credential"), Some("Dev"));
    assert_eq!(vault("op://Dev/My Item/section/field"), Some("Dev"));
}

#[test]
fn a_reference_needs_a_vault_and_an_item() {
    assert_eq!(vault("op://Dev"), None);
    assert_eq!(vault("op://Dev/"), None);
    assert_eq!(vault("op:///pi/credential"), None);
    assert_eq!(vault("Dev/pi/credential"), None);
}

#[test]
fn control_characters_and_oversized_references_are_refused() {
    assert!(validate("op://Dev/pi/credential\n").is_err());
    let long = format!("op://Dev/{}", "a".repeat(600));
    assert!(validate(&long).is_err());
    assert_eq!(validate("op://Dev/pi/credential"), Ok("Dev"));
}

#[test]
fn a_value_survives_a_round_trip_over_a_socket_pair() {
    let (left, right) = UnixStream::pair().unwrap();
    send(&left, &Response::Value(Zeroizing::new("s3cret".to_string()))).unwrap();
    match receive::<Response>(&right).unwrap() {
        Response::Value(value) => assert_eq!(value.as_str(), "s3cret"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn responses_are_tagged_by_outcome() {
    let text = serde_json::to_string(&Response::Refused("no".to_string())).unwrap();
    assert_eq!(text, r#"{"refused":"no"}"#);
}

#[test]
fn a_connection_closed_mid_message_is_an_error() {
    let (mut left, right) = UnixStream::pair().unwrap();
    left.write_all(br#"{"read":"op://Dev"#).unwrap();
    drop(left);
    assert!(receive::<Request>(&right).is_err());
}
