#![forbid(unsafe_code)]

use std::fs;

use dotfmt_core::file::replace;

#[test]
fn replaces_contents_with_arbitrary_bytes_or_an_empty_body() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("target");
    fs::write(&path, b"original contents").unwrap();

    for bytes in [b"\0\xff\n".as_slice(), b"".as_slice()] {
        replace(&path, bytes).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[cfg(unix)]
#[test]
fn replaces_files_with_names_at_the_filesystem_limit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("x".repeat(255));
    fs::write(&path, b"before").unwrap();

    replace(&path, b"after").unwrap();

    assert_eq!(fs::read(&path).unwrap(), b"after");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn preserves_permissions_when_replacing_the_file() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("target");
    fs::write(&path, b"before").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();

    replace(&path, b"after").unwrap();

    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o751
    );
    assert_eq!(fs::read(&path).unwrap(), b"after");
}

#[cfg(unix)]
#[test]
fn replaces_a_symlink_referent_and_keeps_the_link() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("real");
    fs::create_dir(&directory).unwrap();
    let target = directory.join("target");
    fs::write(&target, b"before").unwrap();
    let link = root.path().join("link");
    symlink("real/target", &link).unwrap();

    replace(&link, b"after").unwrap();

    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(
        fs::read_link(&link).unwrap(),
        std::path::Path::new("real/target")
    );
    assert_eq!(fs::read(&target).unwrap(), b"after");
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
}

#[test]
fn a_failed_replacement_keeps_the_target_and_removes_the_temporary_file() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("contents"), b"keep").unwrap();

    assert!(replace(&target, b"after").is_err());

    assert!(target.is_dir());
    assert_eq!(fs::read(target.join("contents")).unwrap(), b"keep");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}
