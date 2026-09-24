use super::*;

const ARCH_HOSTS: &str = "# Static table lookup for hostnames.\n127.0.0.1        localhost\n::1              localhost\n";

#[test]
fn an_unmapped_peer_is_detected() {
    assert!(!maps(ARCH_HOSTS, "macie"));
}

#[test]
fn a_mapping_among_other_names_counts() {
    assert!(maps("127.0.0.2 macie macie.local\n", "macie"));
    assert!(maps("127.0.0.2\tfoo macie # hport\n", "macie"));
}

#[test]
fn a_commented_or_different_mapping_does_not_count() {
    assert!(!maps("# 127.0.0.2 macie\n", "macie"));
    assert!(!maps("127.0.0.3 macie\n", "macie"));
    assert!(!maps("127.0.0.2 maciex\n", "macie"));
}

#[test]
fn the_appended_line_starts_on_its_own_line() {
    assert_eq!(append(ARCH_HOSTS, "macie"), "127.0.0.2 macie\n");
    assert_eq!(
        append("127.0.0.1 localhost", "archie"),
        "\n127.0.0.2 archie\n"
    );
    assert_eq!(append("", "archie"), "127.0.0.2 archie\n");
}

#[test]
fn the_alias_daemon_brings_up_127_0_0_2_at_boot() {
    assert!(ALIAS_PLIST_TEXT.contains(&format!("<string>{ALIAS_LABEL}</string>")));
    assert!(ALIAS_PLIST_TEXT.contains("<string>127.0.0.2</string>"));
    assert!(ALIAS_PLIST_TEXT.contains("<key>RunAtLoad</key>"));
}

#[test]
fn a_missing_runtime_dir_falls_back_to_the_users_run_directory() {
    assert_eq!(runtime_dir(None, 1000), "/run/user/1000");
    assert_eq!(runtime_dir(Some(OsString::new()), 1000), "/run/user/1000");
}

#[test]
fn an_existing_runtime_dir_is_kept() {
    assert_eq!(
        runtime_dir(Some(OsString::from("/run/user/42")), 1000),
        "/run/user/42"
    );
}
