use super::*;
use std::fs;

fn tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("notes.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(root.path().join(".secret"), "hidden\n").unwrap();
    fs::create_dir(root.path().join("assets")).unwrap();
    fs::write(root.path().join("assets/a.bin"), vec![0u8; 2500]).unwrap();
    fs::create_dir(root.path().join(".cache")).unwrap();
    fs::write(root.path().join(".cache/blob"), vec![0u8; 100]).unwrap();
    root
}

fn logical() -> Options {
    Options {
        apparent: true,
        ..Options::default()
    }
}

fn walk_all(root: &Path, lines: bool, all: bool, depth: usize) -> (Vec<Row>, Measure) {
    let options = Options {
        lines,
        all,
        display_depth: depth,
        ..logical()
    };
    let Walked {
        measure, mut rows, ..
    } = walk_directory(&options, root, Path::new(""), 0);
    sort_rows(&mut rows, lines);
    (rows, measure)
}

#[test]
fn totals_include_hidden_files_rows_do_not() {
    let root = tree();
    let (rows, total) = walk_all(root.path(), false, false, 1);
    assert_eq!(total.bytes, 14 + 7 + 2500 + 100);
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, vec!["notes.txt", "assets"]);
    let assets = rows.iter().find(|row| row.name == "assets").unwrap();
    assert_eq!(assets.measure.bytes, 2500);
}

#[test]
fn a_sparse_file_measures_what_it_holds() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("image.raw");
    let claimed = 64 * 1024 * 1024;
    fs::File::create(&path).unwrap().set_len(claimed).unwrap();
    let metadata = fs::symlink_metadata(&path).unwrap();

    assert_eq!(measure_file(&path, &metadata, &logical()).bytes, claimed);
    let on_disk = measure_file(&path, &metadata, &Options::default()).bytes;
    assert!(
        on_disk < claimed,
        "{on_disk} blocks for a hole of {claimed}"
    );
}

#[test]
fn disk_mode_counts_every_entry_the_tree_holds() {
    fn by_hand(directory: &Path) -> u64 {
        fs::read_dir(directory)
            .unwrap()
            .flatten()
            .map(|entry| {
                let metadata = entry.metadata().unwrap();
                let below = if metadata.is_dir() {
                    by_hand(&entry.path())
                } else {
                    0
                };
                allocated(&metadata) + below
            })
            .sum()
    }

    let root = tree();
    let options = Options {
        display_depth: usize::MAX,
        ..Options::default()
    };
    let walked = walk_directory(&options, root.path(), Path::new(""), 0);
    assert_eq!(walked.measure.bytes, by_hand(root.path()));
}

#[test]
fn one_file_system_keeps_the_targets_device_and_drops_the_rest() {
    let root = tree();
    let here = device(&fs::symlink_metadata(root.path()).unwrap());
    let walk = |device| {
        let options = Options {
            display_depth: usize::MAX,
            device,
            ..logical()
        };
        let walked = walk_directory(&options, root.path(), Path::new(""), 0);
        (walked.measure.bytes, walked.rows.len())
    };

    assert_eq!(walk(Some(here)), walk(None));
    assert_eq!(walk(Some(here.wrapping_add(1))), (0, 0));
}

#[test]
fn all_reveals_hidden_entries() {
    let root = tree();
    let (rows, _) = walk_all(root.path(), false, true, 1);
    assert!(rows.iter().any(|row| row.name == ".secret"));
    assert!(rows.iter().any(|row| row.name == ".cache"));
}

#[test]
fn depth_limits_the_listing_not_the_totals() {
    let root = tree();
    let (shallow, total) = walk_all(root.path(), false, false, 1);
    assert!(!shallow.iter().any(|row| row.name.contains("a.bin")));
    let (deep, deep_total) = walk_all(root.path(), false, false, usize::MAX);
    assert!(
        deep.iter()
            .any(|row| row.name == Path::new("assets").join("a.bin").to_string_lossy())
    );
    assert!(!deep.iter().any(|row| row.name.contains("blob")));
    assert_eq!(total.bytes, deep_total.bytes);
}

#[test]
fn line_mode_counts_newlines_recursively() {
    let root = tree();
    let (rows, total) = walk_all(root.path(), true, false, 1);
    assert_eq!(total.lines, 3 + 1);
    let notes = rows.iter().find(|row| row.name == "notes.txt").unwrap();
    assert_eq!(notes.measure.lines, 3);
}

#[test]
fn binary_files_count_zero_lines() {
    let root = tree();
    let mut noise = vec![0u8; 64];
    noise.extend_from_slice(b"\n\n\n");
    fs::write(root.path().join("blob.bin"), noise).unwrap();
    let (rows, _) = walk_all(root.path(), true, false, 1);
    let blob = rows.iter().find(|row| row.name == "blob.bin").unwrap();
    assert_eq!(blob.measure.lines, 0);
}

#[test]
fn line_mode_sorts_by_lines() {
    let root = tree();
    fs::write(root.path().join("tiny.txt"), "a\nb\nc\nd\ne\n").unwrap();
    let (rows, _) = walk_all(root.path(), true, false, 1);
    assert_eq!(rows.last().unwrap().name, "tiny.txt");
}

#[test]
fn rows_sort_smallest_first_whatever_their_kind() {
    let root = tree();
    fs::write(root.path().join("big.txt"), vec![b'x'; 9000]).unwrap();
    let (rows, _) = walk_all(root.path(), false, false, 1);
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, vec!["notes.txt", "assets", "big.txt"]);
}

#[test]
fn human_sizes_match_the_house_style() {
    assert_eq!(human_size(999), "999 B");
    assert_eq!(human_size(3000), "3.0 kB");
    assert_eq!(human_size(1_400), "1.4 kB");
    assert_eq!(human_size(31_000_000), "31 MB");
    assert_eq!(human_size(45_300_000), "45 MB");
}

#[test]
fn unique_prefix_resolves() {
    let root = tree();
    let target = root.path().join("note");
    assert_eq!(resolve(target).unwrap(), root.path().join("notes.txt"));
    assert!(resolve(root.path().join("nothing")).is_err());
}

#[test]
fn grouped_thousands() {
    assert_eq!(grouped(58), "58");
    assert_eq!(grouped(12345), "12,345");
}

fn row(name: &str, kind: &'static str) -> Row {
    Row {
        name: name.to_string(),
        kind,
        executable: false,
        measure: Measure::default(),
    }
}

#[test]
fn the_palette_reads_the_themes_entries() {
    let palette = Palette::parse("reset:fi=38;2;186:di=38;2;61:ex=38;2;124:ln=38;2;26");
    assert_eq!(
        palette.color(&row("src", "directory"), "src"),
        Some("38;2;61")
    );
    assert_eq!(palette.color(&row("link", "link"), "link"), Some("38;2;26"));
    assert_eq!(
        palette.color(&row("notes.txt", "file"), "notes.txt"),
        Some("38;2;186")
    );
    let mut runnable = row("build.sh", "file");
    runnable.executable = true;
    assert_eq!(palette.color(&runnable, "build.sh"), Some("38;2;124"));
}

#[test]
fn an_empty_table_falls_back_to_the_gnu_defaults() {
    let palette = Palette::parse("");
    assert_eq!(
        palette.color(&row("src", "directory"), "src"),
        Some("01;34")
    );
    assert_eq!(palette.color(&row("notes.txt", "file"), "notes.txt"), None);
}

#[test]
fn a_longer_key_is_not_mistaken_for_a_shorter_one() {
    let palette = Palette::parse("dirty=1:link=2");
    assert_eq!(
        palette.color(&row("src", "directory"), "src"),
        Some("01;34")
    );
    assert_eq!(palette.color(&row("l", "link"), "l"), Some("01;36"));
}

fn linked() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("apples")).unwrap();
    fs::create_dir(root.path().join("bananas")).unwrap();
    fs::write(root.path().join("apples/one.txt"), "hello\n").unwrap();
    fs::hard_link(
        root.path().join("apples/one.txt"),
        root.path().join("bananas/two.txt"),
    )
    .unwrap();
    fs::write(root.path().join("solo.txt"), "abc").unwrap();
    root
}

fn walk_and_dedupe(options: &Options, root: &Path) -> (Measure, Vec<Row>) {
    let Walked {
        measure: mut total,
        mut rows,
        links,
    } = walk_directory(options, root, Path::new(""), 0);
    dedupe(&mut total, &mut rows, links);
    (total, rows)
}

#[test]
fn a_file_with_two_names_counts_once() {
    let root = linked();
    let options = Options {
        display_depth: usize::MAX,
        ..logical()
    };
    let (total, _) = walk_and_dedupe(&options, root.path());
    // 6 for the linked file, counted once, and 3 for the other.
    assert_eq!(total.bytes, 6 + 3);
}

#[test]
fn the_lowest_path_is_the_one_that_counts() {
    let root = linked();
    let options = Options {
        display_depth: usize::MAX,
        ..logical()
    };
    for _ in 0..8 {
        let (_, rows) = walk_and_dedupe(&options, root.path());
        let bytes = |name: &str| {
            rows.iter()
                .find(|row| row.name == name)
                .unwrap_or_else(|| panic!("no row for {name}"))
                .measure
                .bytes
        };
        assert_eq!(bytes("apples/one.txt"), 6);
        assert_eq!(bytes("bananas/two.txt"), 0);
        // The directory the repeat sat in loses the bytes with it, so the
        // rows still agree with the total.
        assert_eq!(bytes("apples"), 6);
        assert_eq!(bytes("bananas"), 0);
    }
}

#[test]
fn a_link_from_outside_the_tree_still_counts() {
    let root = linked();
    let options = Options {
        display_depth: usize::MAX,
        ..logical()
    };
    let (total, _) = walk_and_dedupe(&options, &root.path().join("apples"));
    assert_eq!(total.bytes, 6);
}

#[test]
fn globs_match_the_way_the_shell_would() {
    assert!(matches("bin", "bin"));
    assert!(!matches("bin", "binary"));
    assert!(matches("*.log", "server.log"));
    assert!(!matches("*.log", "server.log.gz"));
    assert!(matches("*.log*", "server.log.gz"));
    assert!(matches("node_*", "node_modules"));
    assert!(matches("?ar", "tar"));
    assert!(!matches("?ar", "star"));
    assert!(matches("*", "anything"));
    assert!(matches("*", ""));
    assert!(matches("a*b*c", "axxbyyc"));
    assert!(!matches("a*b*c", "axxbyy"));
}

#[test]
fn ignored_entries_leave_the_totals_as_well_as_the_rows() {
    let root = tree();
    let ignore = Ignore::new(&["assets".to_string()]);
    let options = Options {
        display_depth: 1,
        ignore,
        ..logical()
    };
    let walked = walk_directory(&options, root.path(), Path::new(""), 0);
    // assets/a.bin is 2500 of the tree's bytes, and goes with it.
    assert_eq!(walked.measure.bytes, 14 + 7 + 100);
    assert!(!walked.rows.iter().any(|row| row.name == "assets"));
}

#[test]
fn a_trailing_slash_on_a_pattern_is_ignored() {
    let root = tree();
    for pattern in ["assets", "assets/"] {
        let options = Options {
            display_depth: 1,
            ignore: Ignore::new(&[pattern.to_string()]),
            ..logical()
        };
        let walked = walk_directory(&options, root.path(), Path::new(""), 0);
        assert_eq!(walked.measure.bytes, 14 + 7 + 100, "pattern {pattern}");
    }
}

#[test]
fn a_pattern_with_a_slash_matches_the_relative_path() {
    let root = tree();
    let by_path = Options {
        display_depth: usize::MAX,
        ignore: Ignore::new(&["assets/a.bin".to_string()]),
        ..logical()
    };
    let walked = walk_directory(&by_path, root.path(), Path::new(""), 0);
    assert_eq!(walked.measure.bytes, 14 + 7 + 100);

    // The same text as a bare name matches nothing, since no entry is
    // called "assets/a.bin".
    let by_name = Options {
        display_depth: usize::MAX,
        ignore: Ignore::new(&["a.bin".to_string()]),
        ..logical()
    };
    let walked = walk_directory(&by_name, root.path(), Path::new(""), 0);
    assert_eq!(walked.measure.bytes, 14 + 7 + 100);
}

#[test]
fn extension_colours_come_from_the_table() {
    let palette = Palette::parse("fi=plain:*.rs=rust:*.MD=doc");
    assert_eq!(
        palette.color(&row("main.rs", "file"), "main.rs"),
        Some("rust")
    );
    // The table's case should not decide whether a file matches.
    assert_eq!(
        palette.color(&row("README.md", "file"), "readme.md"),
        Some("doc")
    );
    assert_eq!(
        palette.color(&row("notes.txt", "file"), "notes.txt"),
        Some("plain")
    );
}

#[test]
fn kind_outranks_extension() {
    let palette = Palette::parse("di=dir:ex=exec:ln=link:*.rs=rust");
    assert_eq!(
        palette.color(&row("src.rs", "directory"), "src.rs"),
        Some("dir")
    );
    assert_eq!(palette.color(&row("to.rs", "link"), "to.rs"), Some("link"));
    let mut runnable = row("build.rs", "file");
    runnable.executable = true;
    assert_eq!(palette.color(&runnable, "build.rs"), Some("exec"));
}

#[test]
fn a_later_entry_overrides_an_earlier_one() {
    let palette = Palette::parse("*.toml=first:*.toml=second:fi=one:fi=two");
    assert_eq!(
        palette.color(&row("Cargo.toml", "file"), "cargo.toml"),
        Some("second")
    );
    assert_eq!(palette.color(&row("plain", "file"), "plain"), Some("two"));
}

#[test]
fn icons_follow_the_eza_table() {
    assert_eq!(
        icon_for(&row("src", "directory"), &basename("src")),
        '\u{f115}'
    );
    assert_eq!(
        icon_for(&row("main.rs", "file"), &basename("main.rs")),
        '\u{e68b}'
    );
    assert_eq!(
        icon_for(
            &row("deep/path/notes.md", "file"),
            &basename("deep/path/notes.md")
        ),
        '\u{f48a}'
    );
    assert_eq!(
        icon_for(&row("README.md", "file"), &basename("README.md")),
        '\u{f00ba}'
    );
    assert_eq!(
        icon_for(&row(".gitignore", "file"), &basename(".gitignore")),
        '\u{f02a2}'
    );
    assert_eq!(
        icon_for(&row("mystery", "file"), &basename("mystery")),
        '\u{f086f}'
    );
}
