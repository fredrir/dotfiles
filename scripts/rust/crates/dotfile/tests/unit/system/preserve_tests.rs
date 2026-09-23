use std::path::PathBuf;

use super::{carry, load};
use crate::secret::vault::{SecretEntry, SecretKind};

fn tracked(destination: &str) -> Vec<SecretEntry> {
    vec![SecretEntry {
        source: PathBuf::from("repo/etc/lact/config.yaml"),
        destination: PathBuf::from(destination),
        kind: SecretKind::Plain,
    }]
}

fn marker(text: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(".system");
    std::fs::write(&path, text).unwrap();
    (directory, path)
}

fn prefixes() -> Vec<String> {
    vec!["current_profile:".into()]
}

#[test]
fn installed_owned_line_replaces_the_tracked_one() {
    let carried = carry(
        b"version: 7\ncurrent_profile: null\nauto: false\n",
        b"version: 6\ncurrent_profile: comfort\n",
        &prefixes(),
    );
    assert_eq!(
        carried,
        b"version: 7\ncurrent_profile: comfort\nauto: false\n"
    );
}

#[test]
fn tracked_line_stays_when_installed_file_lacks_it() {
    let wanted = b"current_profile: null\nauto: false";
    assert_eq!(carry(wanted, b"auto: true\n", &prefixes()), wanted);
}

#[test]
fn indented_lines_are_not_owned() {
    let wanted = b"profiles:\n  current_profile: x\n";
    assert_eq!(
        carry(wanted, b"current_profile: comfort\n", &prefixes()),
        wanted
    );
}

#[test]
fn marker_without_content_preserves_nothing() {
    let (_directory, path) = marker("");
    assert!(
        load(&path, &tracked("/etc/lact/config.yaml"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn marker_lists_prefixes_per_destination() {
    let (_directory, path) = marker("preserve {\n  /etc/lact/config.yaml = current_profile:\n}\n");
    let preserved = load(&path, &tracked("/etc/lact/config.yaml")).unwrap();
    assert_eq!(
        preserved[&PathBuf::from("/etc/lact/config.yaml")],
        prefixes()
    );
}

#[test]
fn marker_rejects_untracked_destinations_and_unknown_blocks() {
    let (_directory, path) = marker("preserve {\n  /etc/other.yaml = key:\n}\n");
    let error = load(&path, &tracked("/etc/lact/config.yaml")).unwrap_err();
    assert!(error.contains("not tracked"), "{error}");
    let (_directory, path) = marker("keep {\n  /etc/lact/config.yaml = key:\n}\n");
    let error = load(&path, &tracked("/etc/lact/config.yaml")).unwrap_err();
    assert!(error.contains("unknown block"), "{error}");
    let (_directory, path) = marker("preserve {\n  etc/lact/config.yaml = key:\n}\n");
    assert!(load(&path, &tracked("/etc/lact/config.yaml")).is_err());
}
