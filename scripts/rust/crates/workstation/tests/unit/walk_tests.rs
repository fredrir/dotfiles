use std::collections::BTreeSet;
use std::sync::Mutex;

use super::*;

fn tree(paths: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for path in paths {
        let full = root.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, "").unwrap();
    }
    root
}

fn shown(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

fn every_file(root: &Path, policy: &Policy) -> Walked<PathBuf> {
    walk(root, policy, |_, entries| {
        entries
            .iter()
            .filter(|entry| entry.is_file())
            .map(|entry| entry.relative.clone())
            .collect()
    })
}

fn every_entry(root: &Path, policy: &Policy) -> Walked<()> {
    walk(root, policy, |_, entries| {
        entries.iter().map(|_| ()).collect()
    })
}

#[test]
fn the_skip_list_is_the_one_dotfile_format_and_dotfmt_agree_on() {
    assert_eq!(SKIP.len(), 22);
    for name in [
        ".git",
        ".jj",
        ".hg",
        "node_modules",
        "target",
        "__pycache__",
        ".direnv",
        ".cache",
    ] {
        assert!(SKIP.contains(&name), "{name}");
    }
}

fn count_tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let at = |name: &str| root.path().join(name);
    fs::write(at("a"), "").unwrap();
    fs::write(at(".b"), "").unwrap();
    fs::create_dir(at("sub")).unwrap();
    fs::write(at("sub/c"), "").unwrap();
    fs::create_dir(at("sub/.hid")).unwrap();
    fs::write(at("sub/.hid/d"), "").unwrap();
    fs::create_dir(at(".hidden")).unwrap();
    fs::write(at(".hidden/e"), "").unwrap();
    root
}

fn counting(no_hidden: bool) -> Policy {
    Policy::new().skipping(&[]).skip_hidden(no_hidden)
}

#[test]
fn a_directory_counts_as_an_entry_of_its_own() {
    let root = count_tree();
    assert_eq!(every_entry(root.path(), &counting(false)).items.len(), 8);
}

#[test]
fn a_hidden_name_takes_its_subtree_with_it() {
    let root = count_tree();
    assert_eq!(every_entry(root.path(), &counting(true)).items.len(), 3);
}

#[test]
fn an_empty_directory_offers_nothing() {
    let root = tempfile::tempdir().unwrap();
    let walked = every_entry(root.path(), &counting(false));
    assert!(walked.items.is_empty());
    assert_eq!(walked.unreadable, 0);
}

#[test]
fn a_directory_that_is_not_there_is_counted_unreadable() {
    let root = count_tree();
    let walked = every_entry(&root.path().join("missing"), &counting(false));
    assert!(walked.items.is_empty());
    assert_eq!(walked.unreadable, 1);
}

#[test]
fn the_listing_helper_stops_at_the_direct_children() {
    let root = count_tree();
    assert_eq!(list(root.path(), &counting(false)).unwrap().len(), 4);
    assert_eq!(list(root.path(), &counting(true)).unwrap().len(), 2);
}

#[test]
fn the_listing_helper_names_the_directory_it_could_not_read() {
    let root = count_tree();
    let error = list(&root.path().join("missing"), &counting(false)).unwrap_err();
    assert!(error.contains("missing"), "{error}");
}

#[test]
fn the_listing_helper_names_the_file_it_was_handed_in_place_of_a_directory() {
    let root = count_tree();
    let file = root.path().join("a");
    let error = list(&file, &counting(false)).unwrap_err();
    assert!(error.contains(&file.display().to_string()), "{error}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_name_that_is_not_utf8_reaches_the_caller_with_its_bytes_intact() {
    use std::os::unix::ffi::OsStrExt;

    let root = tempfile::tempdir().unwrap();
    let name = OsStr::from_bytes(b"bad\xffname");
    fs::write(root.path().join(name), "").unwrap();
    let entries = list(root.path(), &counting(false)).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name.as_bytes(), b"bad\xffname");
    assert_eq!(entries[0].kind, Kind::File);
}

#[cfg(unix)]
#[test]
fn a_linked_directory_is_one_entry_and_is_not_followed() {
    let root = count_tree();
    std::os::unix::fs::symlink(root.path().join("sub"), root.path().join("link")).unwrap();
    assert_eq!(every_entry(root.path(), &counting(false)).items.len(), 9);
    assert_eq!(list(root.path(), &counting(false)).unwrap().len(), 5);
}

const AVOIDED: &[&str] = &["node_modules", "target", "vendor"];

fn avoiding() -> Policy {
    Policy::new()
        .skipping(AVOIDED)
        .skip_hidden(true)
        .symlinks(Symlinks::Drop)
}

#[test]
fn a_replaced_skip_list_stands_in_for_the_default() {
    let root = tree(&[
        "a.rs",
        "node_modules/x.rs",
        "vendor/y.rs",
        "sub/b.rs",
        ".git/c.rs",
    ]);
    assert_eq!(
        shown(&every_file(root.path(), &avoiding()).items),
        ["a.rs", "sub/b.rs"]
    );
}

#[cfg(unix)]
#[test]
fn a_dropped_symlink_never_reaches_the_visitor() {
    let root = tree(&["here/a.rs", "elsewhere/b.rs"]);
    let here = root.path().join("here");
    std::os::unix::fs::symlink(root.path().join("elsewhere"), here.join("link")).unwrap();
    std::os::unix::fs::symlink(root.path().join("elsewhere/b.rs"), here.join("link.rs")).unwrap();
    let seen = walk(&here, &avoiding(), |_, entries| {
        entries
            .iter()
            .map(|entry| entry.relative.clone())
            .collect::<Vec<PathBuf>>()
    });
    assert_eq!(shown(&seen.items), ["a.rs"]);
}

#[test]
fn the_order_is_each_directory_by_name_then_the_directories_below_it() {
    let root = tree(&["z.rs", "b.rs", "a/inner.rs", "a/deeper/last.rs"]);
    assert_eq!(
        shown(&every_file(root.path(), &avoiding()).items),
        ["b.rs", "z.rs", "a/inner.rs", "a/deeper/last.rs"]
    );
}

#[test]
fn the_skip_list_is_not_walked() {
    let root = tree(&[
        "keep.py",
        "node_modules/pkg/index.js",
        "target/debug/build.rs",
        ".git/hooks/pre-commit.sh",
        ".venv/lib/x.py",
        "__pycache__/y.py",
    ]);
    assert_eq!(
        shown(&every_file(root.path(), &Policy::new()).items),
        ["keep.py"]
    );
}

#[test]
fn the_skip_list_refuses_directories_and_leaves_files_of_the_same_name() {
    let root = tree(&["target/x.py", "vendor"]);
    assert_eq!(
        shown(&every_file(root.path(), &Policy::new()).items),
        ["vendor"]
    );
}

#[test]
fn a_hidden_name_is_offered_unless_the_policy_refuses_it() {
    let root = tree(&[".gitignore", "kept.py"]);
    assert_eq!(
        shown(&every_file(root.path(), &Policy::new()).items),
        [".gitignore", "kept.py"]
    );
}

#[test]
fn an_entry_carries_both_the_full_path_and_the_path_from_the_root() {
    let root = tree(&["apps/web/deep/app.json"]);
    let walked = walk(root.path(), &Policy::new(), |_, entries| {
        entries
            .iter()
            .filter(|entry| entry.is_file())
            .map(|entry| (entry.relative.clone(), entry.path.clone()))
            .collect::<Vec<(PathBuf, PathBuf)>>()
    });
    let (relative, path) = walked.items.first().unwrap();
    assert_eq!(relative, Path::new("apps/web/deep/app.json"));
    assert_eq!(path, &root.path().join("apps/web/deep/app.json"));
    assert_eq!(walked.items.len(), 1);
}

#[test]
fn a_directory_past_the_depth_cap_is_counted_and_left_alone() {
    let root = tree(&["d0/shallow.py", "d0/d1/d2/buried.py", "e0/e1/also.py"]);
    let walked = every_file(root.path(), &Policy::new().max_depth(1));
    assert_eq!(shown(&walked.items), ["d0/shallow.py"]);
    assert_eq!(walked.deep, 2);
}

#[test]
fn a_depth_cap_of_zero_reads_the_root_and_nothing_below_it() {
    let root = tree(&["top.py", "one/a.py", "two/b.py"]);
    let walked = every_file(root.path(), &Policy::new().max_depth(0));
    assert_eq!(shown(&walked.items), ["top.py"]);
    assert_eq!(walked.deep, 2);
}

#[test]
fn more_files_than_the_cap_allows_sets_the_capped_flag() {
    let root = tree(&["a.py", "b.py", "c.py", "d.py", "e.py"]);
    let walked = every_file(root.path(), &Policy::new().max_files(3));
    assert_eq!(walked.items.len(), 3);
    assert!(walked.capped);
}

#[test]
fn a_walk_that_fits_under_the_cap_is_not_capped() {
    let root = tree(&["a.py", "b.py", "sub/c.py"]);
    let walked = every_file(root.path(), &Policy::new().max_files(3));
    assert_eq!(walked.items.len(), 3);
    assert!(!walked.capped);
}

#[test]
fn what_the_visitor_sets_aside_is_not_charged_against_the_cap() {
    const LOCKFILES: &[&str] = &["Cargo.lock", "package-lock.json"];
    let root = tree(&["Cargo.lock", "package-lock.json", "a.json", "b.json"]);
    let locked = Mutex::new(Vec::new());
    let walked = walk(root.path(), &Policy::new().max_files(2), |_, entries| {
        let mut files = Vec::new();
        for entry in entries.iter().filter(|entry| entry.is_file()) {
            if LOCKFILES.iter().any(|lock| OsStr::new(lock) == entry.name) {
                locked.lock().unwrap().push(entry.relative.clone());
                continue;
            }
            files.push(entry.relative.clone());
        }
        files
    });
    assert_eq!(shown(&walked.items), ["a.json", "b.json"]);
    assert!(!walked.capped);
    let mut set_aside: Vec<PathBuf> = locked.into_inner().unwrap();
    set_aside.sort();
    assert_eq!(shown(&set_aside), ["Cargo.lock", "package-lock.json"]);
}

#[cfg(unix)]
#[test]
fn a_directory_that_cannot_be_read_is_counted_rather_than_dropped() {
    use std::os::unix::fs::PermissionsExt;

    let root = tree(&["a.py", "locked/b.py"]);
    let locked = root.path().join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let walked = every_file(root.path(), &Policy::new());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(shown(&walked.items), ["a.py"]);
    assert_eq!(walked.unreadable, 1);
}

#[cfg(unix)]
#[test]
fn a_link_that_points_at_its_own_root_is_reported_once_and_not_followed() {
    let root = tree(&["a.py"]);
    std::os::unix::fs::symlink(root.path(), root.path().join("loop")).unwrap();
    let walked = walk(root.path(), &Policy::new(), |_, entries| {
        entries
            .iter()
            .map(|entry| (entry.relative.clone(), entry.kind))
            .collect::<Vec<(PathBuf, Kind)>>()
    });
    assert_eq!(walked.items.len(), 2);
    assert_eq!(walked.items[0], (PathBuf::from("a.py"), Kind::File));
    assert_eq!(walked.items[1], (PathBuf::from("loop"), Kind::Symlink));
}

#[test]
fn a_directory_reaches_the_visitor_once_with_all_of_its_entries() {
    let root = tree(&["a/x.conf", "a/z.conf", "b/y.conf", "top.conf"]);
    let visits = Mutex::new(Vec::new());
    let walked = walk(root.path(), &Policy::new(), |directory, entries| {
        let files: Vec<PathBuf> = entries
            .iter()
            .filter(|entry| !entry.is_dir())
            .map(|entry| entry.relative.clone())
            .collect();
        if !files.is_empty() {
            visits.lock().unwrap().push(directory.to_path_buf());
        }
        files
    });
    let visited: Vec<PathBuf> = visits.into_inner().unwrap();
    let once: BTreeSet<PathBuf> = visited.iter().cloned().collect();
    assert_eq!(visited.len(), 3);
    assert_eq!(once.len(), 3);
    assert!(once.contains(&root.path().join("a")));
    assert!(once.contains(&root.path().join("b")));
    assert_eq!(
        shown(&walked.items),
        ["top.conf", "a/x.conf", "a/z.conf", "b/y.conf"]
    );
}

#[cfg(unix)]
#[test]
fn a_reported_symlink_is_not_a_directory_and_is_not_descended() {
    let root = tree(&["here/a.conf", "elsewhere/b.conf"]);
    let here = root.path().join("here");
    std::os::unix::fs::symlink(root.path().join("elsewhere"), here.join("link")).unwrap();
    let walked = walk(&here, &Policy::new(), |_, entries| {
        entries
            .iter()
            .filter(|entry| !entry.is_dir())
            .map(|entry| entry.relative.clone())
            .collect::<Vec<PathBuf>>()
    });
    assert_eq!(shown(&walked.items), ["a.conf", "link"]);
}

#[cfg(unix)]
#[test]
fn each_entry_carries_the_kind_the_listing_reported() {
    let root = tree(&["file.txt", "dir/inner.txt"]);
    std::os::unix::fs::symlink(root.path().join("dir"), root.path().join("link")).unwrap();
    let named: Vec<(String, Kind)> = list(root.path(), &Policy::new().skipping(&[]))
        .unwrap()
        .iter()
        .map(|entry| (entry.name.to_string_lossy().into_owned(), entry.kind))
        .collect();
    assert_eq!(
        named,
        [
            ("dir".to_string(), Kind::Directory),
            ("file.txt".to_string(), Kind::File),
            ("link".to_string(), Kind::Symlink),
        ]
    );
}
