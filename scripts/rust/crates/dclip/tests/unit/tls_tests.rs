use super::*;

#[test]
fn the_server_name_is_the_dns_san_of_each_host() {
    assert_eq!(server_name(Host::Macie).unwrap().to_str(), "macie");
    assert_eq!(server_name(Host::Archie).unwrap().to_str(), "archie");
}

#[test]
fn garbage_has_no_common_name() {
    assert_eq!(common_name(b"not a certificate"), None);
    assert_eq!(common_name(&[]), None);
}

#[test]
fn some_user_name_is_found() {
    assert!(!user().unwrap().is_empty());
}
