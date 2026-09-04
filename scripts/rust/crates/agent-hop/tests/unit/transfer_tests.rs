use super::*;
use std::fs;
use std::path::PathBuf;

#[test]
fn jsonl_requires_one_complete_object_on_every_line() {
    assert!(valid_jsonl(
        br#"{"type":"one"}
{"type":"two","payload":{"id":1}}
"#
    ));
    assert!(valid_jsonl(br#"{"type":"one"}"#));
    assert!(!valid_jsonl(b""));
    assert!(!valid_jsonl(b"\n"));
    assert!(!valid_jsonl(b"{}\n\n"));
    assert!(!valid_jsonl(b"{}\n{\n"));
    assert!(!valid_jsonl(b"[]\n"));
    assert!(!valid_jsonl(&[0xff, b'\n']));
}

#[test]
fn a_snapshot_is_an_exact_independent_copy() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("session.jsonl");
    let body = b"{\"type\":\"one\"}\n{\"type\":\"two\"}\n";
    fs::write(&source, body).unwrap();
    let snapshot = Snapshot::create(&source).unwrap();
    assert_eq!(fs::read(snapshot.path()).unwrap(), body);
    fs::write(&source, "{\"changed\":true}\n").unwrap();
    assert_eq!(fs::read(snapshot.path()).unwrap(), body);
}

#[test]
fn invalid_jsonl_is_rejected_after_the_retry_budget() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("session.jsonl");
    fs::write(&source, "{\"unfinished\":").unwrap();
    let error = Snapshot::create(&source).err().unwrap();
    assert!(error.contains("after 3 attempts"));
    assert!(error.contains("not valid JSONL"));
}

#[test]
fn file_transfer_arguments_keep_each_path_one_process_argument() {
    assert_eq!(
        file_arguments(
            Host::Archie,
            Path::new("/tmp/agent hop.snapshot"),
            Path::new("/home/fred rir/.codex/a'b.jsonl"),
        )
        .unwrap(),
        [
            "-a",
            "-e",
            "ssh -o ConnectTimeout=8 -o LogLevel=ERROR",
            "--",
            "/tmp/agent hop.snapshot",
            "archie:'/home/fred rir/.codex/a'\\''b.jsonl'",
        ]
        .map(OsString::from)
    );
}

#[test]
fn attachment_transfer_copies_directory_contents() {
    assert_eq!(
        directory_arguments(
            Host::Macie,
            Path::new("/tmp/attachments"),
            Path::new("/Users/fredrir/.claude/project/id"),
        )
        .unwrap(),
        [
            "-a",
            "-e",
            "ssh -o ConnectTimeout=8 -o LogLevel=ERROR",
            "--",
            "/tmp/attachments/",
            "macie:'/Users/fredrir/.claude/project/id'/",
        ]
        .map(OsString::from)
    );
}

#[test]
fn transcript_and_companion_builders_are_independent_operations() {
    let transcript = file_arguments(
        Host::Archie,
        Path::new("/tmp/session.jsonl"),
        Path::new("/home/fredrir/session.jsonl"),
    )
    .unwrap();
    let companion = directory_arguments(
        Host::Archie,
        Path::new("/tmp/session"),
        Path::new("/home/fredrir/session"),
    )
    .unwrap();
    assert_eq!(transcript[4], "/tmp/session.jsonl");
    assert_eq!(transcript[5], "archie:'/home/fredrir/session.jsonl'");
    assert_eq!(companion[4], "/tmp/session/");
    assert_eq!(companion[5], "archie:'/home/fredrir/session'/");
}

#[test]
fn a_disappearing_companion_is_reported_before_transfer() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let error =
        copy_companion(Host::Archie, &missing, Path::new("/home/fredrir/session")).unwrap_err();
    assert!(error.contains("could not inspect"));
}

#[test]
fn snapshot_paths_can_be_passed_directly_to_the_file_builder() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("session.jsonl");
    fs::write(&source, "{}\n").unwrap();
    let snapshot = Snapshot::create(&source).unwrap();
    let arguments = file_arguments(
        Host::Archie,
        snapshot.path(),
        Path::new("/home/fredrir/session.jsonl"),
    )
    .unwrap();
    assert_eq!(arguments[4], snapshot.path().as_os_str());
}

#[test]
fn remote_targets_follow_the_named_peer() {
    let local = PathBuf::from("/tmp/session");
    let destination = PathBuf::from("/Users/fredrir/session");
    assert_eq!(
        file_arguments(Host::Macie, &local, &destination).unwrap()[5],
        "macie:'/Users/fredrir/session'"
    );
    assert_eq!(
        file_arguments(Host::Archie, &local, &destination).unwrap()[5],
        "archie:'/Users/fredrir/session'"
    );
}

#[test]
fn immutable_install_reuses_exact_objects_and_refuses_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.jsonl");
    let destination = directory.path().join("destination.jsonl");
    fs::write(&source, "{\"value\":1}\n").unwrap();
    assert!(!install_immutable_file(&source, &destination).unwrap());
    assert!(install_immutable_file(&source, &destination).unwrap());
    fs::write(&source, "{\"value\":2}\n").unwrap();
    assert!(
        install_immutable_file(&source, &destination)
            .unwrap_err()
            .contains("different contents")
    );
    assert_eq!(fs::read_to_string(destination).unwrap(), "{\"value\":1}\n");
}
