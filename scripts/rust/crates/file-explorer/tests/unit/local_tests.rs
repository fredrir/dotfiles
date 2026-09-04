use super::*;
use tempfile::tempdir;

#[test]
fn present_directory_has_parent_label_and_sorted_typed_entries() {
    let temporary = tempdir().unwrap();
    fs::create_dir(temporary.path().join("z-dir")).unwrap();
    fs::create_dir(temporary.path().join("A-dir")).unwrap();
    fs::write(temporary.path().join("b-file"), "contents").unwrap();
    fs::write(temporary.path().join("A-file"), "contents").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("b-file", temporary.path().join("link")).unwrap();
        std::os::unix::fs::symlink("A-dir", temporary.path().join("dir-link")).unwrap();
    }

    let directory = LocalSource::without_home()
        .read_directory(&temporary.path().to_path_buf())
        .unwrap();
    assert_eq!(directory.status, DirectoryStatus::Present);
    assert_eq!(directory.location, temporary.path());
    assert_eq!(directory.parent.as_deref(), temporary.path().parent());
    assert_eq!(directory.label, temporary.path().to_string_lossy());
    let actual: Vec<_> = directory
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.kind))
        .collect();
    #[cfg(unix)]
    assert_eq!(
        actual,
        vec![
            ("A-dir", EntryKind::Directory),
            ("dir-link", EntryKind::SymlinkDirectory),
            ("z-dir", EntryKind::Directory),
            ("A-file", EntryKind::File),
            ("b-file", EntryKind::File),
            ("link", EntryKind::Symlink),
        ]
    );
    #[cfg(not(unix))]
    assert_eq!(
        actual,
        vec![
            ("A-dir", EntryKind::Directory),
            ("z-dir", EntryKind::Directory),
            ("A-file", EntryKind::File),
            ("b-file", EntryKind::File),
        ]
    );
}

#[test]
fn hidden_entries_are_included() {
    let temporary = tempdir().unwrap();
    fs::write(temporary.path().join(".secret"), "contents").unwrap();
    let directory = LocalSource::default()
        .read_directory(&temporary.path().to_path_buf())
        .unwrap();
    assert_eq!(directory.entries.len(), 1);
    assert_eq!(directory.entries[0].name, ".secret");
}

#[test]
fn missing_and_non_directory_locations_have_distinct_statuses() {
    let temporary = tempdir().unwrap();
    let missing_path = temporary.path().join("missing");
    let source = LocalSource::without_home();
    let missing = source.read_directory(&missing_path).unwrap();
    assert_eq!(missing.status, DirectoryStatus::Missing);
    assert_eq!(missing.location, missing_path);
    assert!(missing.entries.is_empty());

    let file_path = temporary.path().join("file");
    fs::write(&file_path, "contents").unwrap();
    let unreadable = source.read_directory(&file_path).unwrap();
    assert!(matches!(
        unreadable.status,
        DirectoryStatus::Unreadable(ref reason) if !reason.is_empty()
    ));
    assert!(unreadable.entries.is_empty());
}

#[test]
fn entry_identity_is_the_native_path_not_the_display_label() {
    let temporary = tempdir().unwrap();
    let name = "line\nbreak\u{1b}[31m";
    let path = temporary.path().join(name);
    fs::write(&path, "contents").unwrap();
    let directory = LocalSource::default()
        .read_directory(&temporary.path().to_path_buf())
        .unwrap();
    assert_eq!(directory.entries[0].location, path);
    assert_eq!(directory.entries[0].name, "line\\nbreak\\e[31m");
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_names_keep_lossless_path_identity_and_a_lossy_label() {
    use std::os::unix::ffi::OsStringExt;

    let temporary = tempdir().unwrap();
    let raw_name = std::ffi::OsString::from_vec(vec![b'n', 0x80, b'm', b'e']);
    let path = temporary.path().join(&raw_name);
    fs::write(&path, "contents").unwrap();
    let directory = LocalSource::default()
        .read_directory(&temporary.path().to_path_buf())
        .unwrap();
    assert_eq!(directory.entries[0].location, path);
    assert_eq!(directory.entries[0].name, "n\u{fffd}me");
}

#[cfg(unix)]
#[test]
fn non_utf8_names_have_a_safe_lossy_label() {
    use std::os::unix::ffi::OsStringExt;

    let raw_name = std::ffi::OsString::from_vec(vec![b'n', 0x80, b'm', b'e']);
    assert_eq!(display_os_str(&raw_name), "n\u{fffd}me");
}

#[cfg(unix)]
#[test]
fn unix_socket_is_classified_as_other() {
    use std::os::unix::net::UnixListener;

    let temporary = tempdir().unwrap();
    let socket = temporary.path().join("service.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    let directory = LocalSource::default()
        .read_directory(&temporary.path().to_path_buf())
        .unwrap();
    assert_eq!(directory.entries[0].kind, EntryKind::Other);
}

#[test]
fn input_kind_distinguishes_names_from_path_forms() {
    let source = LocalSource::default();
    assert_eq!(source.input_kind("notes"), InputKind::Search);
    assert_eq!(source.input_kind(".secret"), InputKind::Search);
    assert_eq!(source.input_kind(""), InputKind::Search);
    assert_eq!(source.input_kind("/etc"), InputKind::Location);
    assert_eq!(source.input_kind("~/work"), InputKind::Location);
    assert_eq!(source.input_kind("../work"), InputKind::Location);
    assert_eq!(source.input_kind("work/src"), InputKind::Location);
    #[cfg(unix)]
    assert_eq!(source.input_kind(r"notes\draft"), InputKind::Search);
}

#[test]
fn resolves_path_forms_without_erasing_filesystem_components() {
    let source = LocalSource::with_home("/users/alice");
    let current = PathBuf::from("/work/project");
    assert_eq!(source.resolve_input(&current, "").unwrap(), current);
    assert_eq!(
        source.resolve_input(&current, "/var/./log").unwrap(),
        PathBuf::from("/var/./log")
    );
    assert_eq!(
        source.resolve_input(&current, "src/../tests").unwrap(),
        PathBuf::from("/work/project/src/../tests")
    );
    assert_eq!(
        source.resolve_input(&current, "..").unwrap(),
        PathBuf::from("/work/project/..")
    );
    assert_eq!(
        source.resolve_input(&current, "~").unwrap(),
        PathBuf::from("/users/alice")
    );
    assert_eq!(
        source.resolve_input(&current, "~/src/../bin").unwrap(),
        PathBuf::from("/users/alice/src/../bin")
    );
}

#[test]
fn an_opened_parent_component_uses_the_filesystem_identity() {
    let temporary = tempdir().unwrap();
    let project = temporary.path().join("workspace/project");
    fs::create_dir_all(&project).unwrap();
    let requested = project.join("..");

    let directory = LocalSource::without_home()
        .read_directory(&requested)
        .unwrap();
    let expected = fs::canonicalize(temporary.path().join("workspace")).unwrap();

    assert_eq!(directory.location, expected);
    assert_eq!(directory.parent, expected.parent().map(Path::to_path_buf));
}

#[cfg(unix)]
#[test]
fn parent_components_follow_symlinks_but_direct_symlink_navigation_keeps_the_alias() {
    let temporary = tempdir().unwrap();
    let actual = temporary.path().join("actual");
    let nested = actual.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("entry"), []).unwrap();
    let alias = temporary.path().join("alias");
    std::os::unix::fs::symlink(&nested, &alias).unwrap();
    let source = LocalSource::without_home();

    let resolved = source.read_directory(&alias.join("..")).unwrap();
    assert_eq!(resolved.location, fs::canonicalize(&actual).unwrap());

    let direct = source.read_directory(&alias).unwrap();
    assert_eq!(direct.location, alias);
    assert_eq!(direct.entries[0].location, alias.join("entry"));
}

#[test]
fn home_errors_are_typed_and_explanatory() {
    let source = LocalSource::without_home();
    assert!(matches!(
        source.resolve_input(&PathBuf::from("/work"), "~"),
        Err(LocalError::HomeUnavailable)
    ));
    let source = LocalSource::with_home("/users/alice");
    assert!(matches!(
        source.resolve_input(&PathBuf::from("/work"), "~bob/src"),
        Err(LocalError::UnsupportedHome(value)) if value == "~bob/src"
    ));
    assert_eq!(
        LocalError::HomeUnavailable.to_string(),
        "home directory is unavailable"
    );
}

#[test]
fn root_has_no_parent() {
    let directory = LocalSource::default()
        .read_directory(&PathBuf::from("/"))
        .unwrap();
    assert_eq!(directory.parent, None);
}

#[test]
fn unicode_labels_remain_readable() {
    assert_eq!(sanitize("blåbær"), "blåbær");
    assert_eq!(sanitize("a\tb\rc"), "a\\tb\\rc");
}
