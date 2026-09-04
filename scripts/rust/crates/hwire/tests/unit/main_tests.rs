use super::*;

#[test]
fn the_banner_says_where_to_connect() {
    assert_eq!(
        address_of("hwire serve 10.77.77.2 54321\n"),
        Some(SocketAddrV4::new(Ipv4Addr::new(10, 77, 77, 2), 54321))
    );
}

#[test]
fn anything_else_on_stdout_is_not_a_banner() {
    assert_eq!(address_of(""), None);
    assert_eq!(address_of("Last login: Tue Aug 18\n"), None);
    assert_eq!(address_of("hwire serve 10.77.77.2\n"), None);
    assert_eq!(address_of("hwire serve nowhere 54321\n"), None);
}

#[test]
fn the_remote_command_survives_a_minimal_login_path() {
    let command = format!("{REMOTE_PATH}hwire serve");
    assert!(command.starts_with("PATH=\"$HOME/.local/bin:"));
    assert!(command.ends_with("hwire serve"));
}
