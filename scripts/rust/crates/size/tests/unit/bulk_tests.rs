use super::*;
use std::fs;

type Shape = (String, &'static str, bool, u64, u64);

type Linked = ((u64, u64), String, u64, u64);

type Answer = (u64, u64, usize, Vec<Shape>, Vec<Linked>);

fn shape(walked: Walked) -> Answer {
    let mut rows: Vec<Shape> = walked
        .rows
        .into_iter()
        .map(|row| {
            (
                row.name,
                row.kind,
                row.executable,
                row.measure.bytes,
                row.measure.lines,
            )
        })
        .collect();
    rows.sort();
    let mut links: Vec<Linked> = walked
        .links
        .into_iter()
        .map(|link| (link.file, link.path, link.bytes, link.lines))
        .collect();
    links.sort();
    (
        walked.measure.bytes,
        walked.measure.lines,
        walked.measure.unreadable,
        rows,
        links,
    )
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let at = |path: &str| root.path().join(path);
    fs::create_dir_all(at("visible/deeper")).unwrap();
    fs::create_dir_all(at(".hidden/deeper")).unwrap();
    fs::write(at("notes.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(at("no-newline.txt"), "trailing").unwrap();
    fs::write(at("binary.bin"), b"\0\0\n\n\n").unwrap();
    fs::write(at(".secret"), "hidden\n").unwrap();
    fs::write(at("visible/inner.txt"), "a\nb\n").unwrap();
    fs::write(at("visible/deeper/deep.txt"), "deep\n").unwrap();
    fs::write(at(".hidden/deeper/buried.txt"), "buried\n").unwrap();
    fs::write(at("spaced name.txt"), "space\n").unwrap();
    fs::write(at("üni—code.txt"), "unicode\n").unwrap();
    let runnable = at("run.sh");
    fs::write(&runnable, "#!/bin/sh\n").unwrap();
    let mut mode = fs::metadata(&runnable).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    fs::set_permissions(&runnable, mode).unwrap();
    fs::hard_link(at("notes.txt"), at("visible/same-file.txt")).unwrap();
    std::os::unix::fs::symlink("notes.txt", at("link")).unwrap();
    std::os::unix::fs::symlink("/nowhere", at("broken")).unwrap();
    std::os::unix::fs::symlink("visible", at("dirlink")).unwrap();
    root
}

#[test]
fn the_bulk_walk_agrees_with_the_portable_one() {
    let root = fixture();
    for lines in [false, true] {
        for all in [false, true] {
            for apparent in [false, true] {
                for display_depth in [1, 2, usize::MAX] {
                    let options = Options {
                        lines,
                        all,
                        apparent,
                        display_depth,
                        ..Options::default()
                    };
                    let bulk = shape(walk(&options, root.path()).expect("bulk walk runs"));
                    let portable = shape(super::super::walk_directory(
                        &options,
                        root.path(),
                        Path::new(""),
                        0,
                    ));
                    assert!(!bulk.4.is_empty(), "the fixture has a hardlink");
                    assert_eq!(
                        bulk, portable,
                        "lines={lines} all={all} apparent={apparent} depth={display_depth}"
                    );
                }
            }
        }
    }
}

#[test]
fn ignored_entries_never_reach_the_totals() {
    let root = fixture();
    let plain = Options {
        display_depth: 1,
        ..Options::default()
    };
    let whole = walk(&plain, root.path()).expect("bulk walk runs");
    let options = Options {
        display_depth: 1,
        ignore: super::super::Ignore::new(&["visible".to_string()]),
        ..Options::default()
    };
    let trimmed = walk(&options, root.path()).expect("bulk walk runs");
    assert!(trimmed.measure.bytes < whole.measure.bytes);
    assert!(!trimmed.rows.iter().any(|row| row.name == "visible"));
    let portable = super::super::walk_directory(&options, root.path(), Path::new(""), 0);
    assert_eq!(trimmed.measure.bytes, portable.measure.bytes);
}

#[test]
fn one_file_system_agrees_with_the_portable_walk() {
    let root = fixture();
    let here = super::super::device(&fs::symlink_metadata(root.path()).unwrap());
    for device in [Some(here), Some(here.wrapping_add(1)), None] {
        let options = Options {
            display_depth: usize::MAX,
            device,
            ..Options::default()
        };
        let bulk = shape(walk(&options, root.path()).expect("bulk walk runs"));
        let portable = shape(super::super::walk_directory(
            &options,
            root.path(),
            Path::new(""),
            0,
        ));
        assert_eq!(bulk, portable, "device={device:?}");
    }
    // The fixture is all on one device, so only a device it is not on can
    // show that anything is being dropped at all.
    let elsewhere = Options {
        display_depth: usize::MAX,
        device: Some(here.wrapping_add(1)),
        ..Options::default()
    };
    let walked = walk(&elsewhere, root.path()).expect("bulk walk runs");
    assert_eq!(walked.measure.bytes, 0);
    assert!(walked.rows.is_empty());
}

#[test]
fn directory_symlinks_are_not_followed() {
    let root = fixture();
    let options = Options {
        lines: false,
        all: false,
        display_depth: usize::MAX,
        ..Options::default()
    };
    let walked = walk(&options, root.path()).expect("bulk walk runs");
    let rows = &walked.rows;
    let dirlink = rows.iter().find(|row| row.name == "dirlink").unwrap();
    assert_eq!(dirlink.kind, "link");
    assert!(!rows.iter().any(|row| row.name.starts_with("dirlink/")));
}
