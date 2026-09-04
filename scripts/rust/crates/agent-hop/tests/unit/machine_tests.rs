use super::*;

#[test]
fn only_the_current_protocol_is_accepted() {
    assert!(require_protocol(MACHINE_PROTOCOL_VERSION).is_ok());
    assert!(require_protocol(MACHINE_PROTOCOL_VERSION + 1).is_err());
}
