#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use dotfmt_core::walk::{Explicit, Symlinks, gather};

fn matches(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "fmt")
}

#[test]
fn directory_selection_is_sorted_and_uses_the_shared_skip_policy() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        "z.fmt",
        "deep/b.fmt",
        "a.fmt",
        "not-selected.txt",
        "target/ignored.fmt",
        "node_modules/ignored.fmt",
    ] {
        let path = root.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "contents").unwrap();
    }

    let found = gather(root.path(), Symlinks::Drop, Explicit::Regular, matches).unwrap();

    assert_eq!(
        found.files,
        ["a.fmt", "deep/b.fmt", "z.fmt"].map(|name| root.path().join(name))
    );
    assert_eq!(found.unreadable, 0);
}

#[test]
fn explicit_files_bypass_filename_selection() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join(".configuration");
    fs::write(&target, "contents").unwrap();

    for policy in [Explicit::Any, Explicit::Regular] {
        let found = gather(&target, Symlinks::Drop, policy, matches).unwrap();
        assert_eq!(found.files, std::slice::from_ref(&target));
    }
}

#[test]
fn missing_targets_are_errors_instead_of_empty_selections() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("absent.fmt");

    let error = gather(&target, Symlinks::Drop, Explicit::Regular, matches).unwrap_err();

    assert!(error.contains(&target.display().to_string()), "{error}");
}

#[cfg(unix)]
#[test]
fn discovered_symlinks_follow_the_requested_policy_without_recursing() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let real = root.path().join("real.fmt");
    fs::write(&real, "contents").unwrap();
    fs::write(outside.path().join("nested.fmt"), "contents").unwrap();
    for (source, name) in [
        (real.as_path(), "link.fmt"),
        (outside.path(), "directory.fmt"),
        (Path::new("missing"), "broken.fmt"),
    ] {
        std::os::unix::fs::symlink(source, root.path().join(name)).unwrap();
    }

    let dropped = gather(root.path(), Symlinks::Drop, Explicit::Any, matches).unwrap();
    assert_eq!(dropped.files, [real]);

    let reported = gather(root.path(), Symlinks::Report, Explicit::Any, matches).unwrap();
    assert_eq!(
        reported.files,
        ["broken.fmt", "directory.fmt", "link.fmt", "real.fmt"].map(|name| root.path().join(name))
    );
}

#[cfg(unix)]
#[test]
fn explicit_symlinks_keep_the_supplied_path_even_when_discovery_drops_links() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real.fmt");
    let link = root.path().join("link");
    fs::write(&real, "contents").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let found = gather(&link, Symlinks::Drop, Explicit::Regular, matches).unwrap();

    assert_eq!(found.files, [link]);
}

#[cfg(unix)]
#[test]
fn explicit_nonregular_targets_are_rejected_only_when_requested() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("socket");
    let _socket = std::os::unix::net::UnixListener::bind(&target).unwrap();

    let error = gather(&target, Symlinks::Drop, Explicit::Regular, matches).unwrap_err();
    assert!(error.contains("not a regular file or directory"), "{error}");

    let found = gather(&target, Symlinks::Drop, Explicit::Any, matches).unwrap();
    assert_eq!(found.files, [target]);
}
