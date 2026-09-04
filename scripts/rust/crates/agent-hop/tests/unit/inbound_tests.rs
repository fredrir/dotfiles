use super::*;

#[test]
fn equal_file_comparison_streams_content() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left");
    let right = directory.path().join("right");
    fs::write(&left, b"same").unwrap();
    fs::write(&right, b"same").unwrap();
    assert!(files_equal(&left, &right).unwrap());
    fs::write(&right, b"different").unwrap();
    assert!(!files_equal(&left, &right).unwrap());
}

#[test]
fn safe_directory_creation_rejects_a_file_in_the_path() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    fs::create_dir(&home).unwrap();
    fs::write(home.join("blocked"), "file").unwrap();
    let error = ensure_directory_tree(&home, &home.join("blocked/child")).unwrap_err();
    assert!(error.contains("unsafe non-directory"));
}

#[test]
fn new_transcripts_are_installed_without_overwriting() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    fs::write(&source, "{\"value\":\"one\"}\n").unwrap();
    install_new_file(&source, &destination).unwrap();
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "{\"value\":\"one\"}\n"
    );
    fs::write(&source, "{\"value\":\"two\"}\n").unwrap();
    assert!(install_new_file(&source, &destination).is_err());
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "{\"value\":\"one\"}\n"
    );
}

#[test]
fn companion_commit_is_atomic_when_destination_is_new() {
    let directory = tempfile::tempdir().unwrap();
    let staging = tempfile::tempdir_in(directory.path()).unwrap();
    let payload = staging.path().join("payload");
    fs::create_dir(&payload).unwrap();
    fs::write(payload.join("attachment.txt"), "content").unwrap();
    let destination = directory.path().join("session");
    commit_companion(staging, &payload, &destination).unwrap();
    assert_eq!(
        fs::read_to_string(destination.join("attachment.txt")).unwrap(),
        "content"
    );
}

#[test]
fn existing_companion_is_accepted_only_when_exactly_identical() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("session");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("attachment.txt"), "local").unwrap();

    let identical_staging = tempfile::tempdir_in(directory.path()).unwrap();
    let identical = identical_staging.path().join("payload");
    fs::create_dir(&identical).unwrap();
    fs::write(identical.join("attachment.txt"), "local").unwrap();
    commit_companion(identical_staging, &identical, &destination).unwrap();

    let staging = tempfile::tempdir_in(directory.path()).unwrap();
    let payload = staging.path().join("payload");
    fs::create_dir(&payload).unwrap();
    fs::write(payload.join("attachment.txt"), "remote").unwrap();
    let error = commit_companion(staging, &payload, &destination).unwrap_err();
    assert!(error.contains("different contents"));
    assert_eq!(
        fs::read_to_string(destination.join("attachment.txt")).unwrap(),
        "local"
    );
}

#[test]
fn existing_companion_with_stale_extra_files_is_not_modified() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("session");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("attachment.txt"), "same").unwrap();
    fs::write(destination.join("stale.txt"), "keep").unwrap();

    let staging = tempfile::tempdir_in(directory.path()).unwrap();
    let payload = staging.path().join("payload");
    fs::create_dir(&payload).unwrap();
    fs::write(payload.join("attachment.txt"), "same").unwrap();

    let error = commit_companion(staging, &payload, &destination).unwrap_err();
    assert!(error.contains("different contents"));
    assert_eq!(
        fs::read_to_string(destination.join("attachment.txt")).unwrap(),
        "same"
    );
    assert_eq!(
        fs::read_to_string(destination.join("stale.txt")).unwrap(),
        "keep"
    );
}
