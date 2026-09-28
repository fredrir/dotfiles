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
