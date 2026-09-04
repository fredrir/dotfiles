use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use super::*;

fn listed(path: &str, names: &[(&str, bool)]) -> Listing {
    Listing {
        path: path.to_string(),
        home: "/home/fredrir".into(),
        entries: names
            .iter()
            .map(|(name, directory)| Entry {
                name: (*name).to_string(),
                directory: *directory,
            })
            .collect(),
        missing: false,
    }
}

#[test]
fn a_listing_separates_the_directories_from_the_files() {
    let listing = parse(
        "HOME\0/home/f\0ASK\0/home/f/projects\0DIR\0/home/f/projects\0\
             ENTRY\0D\0my-app\0ENTRY\0F\0notes.md\0ENTRY\0D\0src\0DONE\0",
    )
    .expect("a listing");
    assert_eq!(listing.home, "/home/f");
    assert_eq!(listing.path, "/home/f/projects");
    assert!(!listing.missing);
    assert_eq!(
        listing.entries,
        vec![
            Entry {
                name: "my-app".into(),
                directory: true
            },
            Entry {
                name: "src".into(),
                directory: true
            },
            Entry {
                name: "notes.md".into(),
                directory: false
            },
        ]
    );
}

#[test]
fn directories_come_first_and_then_case_folded_order() {
    let listing = parse(
        "HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0ENTRY\0F\0b.txt\0\
             ENTRY\0D\0Alpha\0ENTRY\0F\0a.txt\0ENTRY\0D\0zeta\0DONE\0",
    )
    .unwrap();
    let names: Vec<&str> = listing
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["Alpha", "zeta", "a.txt", "b.txt"]);
}

#[test]
fn a_directory_that_is_not_there_keeps_the_requested_path() {
    let listing = parse("HOME\0/home/f\0ASK\0/home/f/new/place\0GONE\0").unwrap();
    assert!(listing.missing);
    assert_eq!(listing.home, "/home/f");
    assert_eq!(listing.path, "/home/f/new/place");
    assert!(listing.entries.is_empty());
}

#[test]
fn an_empty_directory_is_not_a_missing_one() {
    let listing = parse("HOME\0/home/f\0ASK\0/home/f/empty\0DIR\0/home/f/empty\0DONE\0").unwrap();
    assert!(!listing.missing);
    assert!(listing.entries.is_empty());
}

#[test]
fn a_reply_that_says_nothing_useful_is_an_error() {
    assert!(parse("").is_err());
    assert!(parse("ASK\0/home/f\0DIR\0/home/f\0DONE\0").is_err());
    assert!(parse("HOME\0/home/f\0ASK\0/home/f\0").is_err());
    assert!(parse("HOME\0/home/f\0DIR\0/home/f\0DONE\0").is_err());
}

#[test]
fn whitespace_in_names_survives_the_listing() {
    let listing = parse(
        "HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0ENTRY\0D\0my\nnotes\0\
             ENTRY\0F\0a\tfile.txt\0DONE\0",
    )
    .unwrap();
    assert_eq!(listing.entries[0].name, "my\nnotes");
    assert!(listing.entries[0].directory);
    assert_eq!(listing.entries[1].name, "a\tfile.txt");
}

#[test]
fn a_failed_listing_is_not_a_missing_directory() {
    let error = parse("HOME\0/home/f\0ASK\0/home/f/private\0FAIL\0directory could not be read\0")
        .unwrap_err();
    assert!(error.contains("could not list"));
    assert!(error.contains("could not be read"));
}

#[test]
fn malformed_records_are_rejected() {
    assert!(parse("HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0ENTRY\0X\0name\0DONE\0").is_err());
    assert!(parse("HOME\0/home/f\0ASK\0/home/f\0WHAT\0").is_err());
    assert!(parse("HOME\0/home/f\0ASK\0/home/f\0DIR").is_err());
    assert!(parse("HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0").is_err());
    assert!(parse("HOME\0/home/f\0ASK\0/home/f\0ENTRY\0F\0name\0GONE\0").is_err());
    assert!(parse("HOME\0/home/f\0ASK\0/home/f\0GONE\0ENTRY\0F\0name\0").is_err());
}

#[test]
fn non_utf8_identities_have_a_clear_error() {
    let error = parse_reply(
        "archie",
        b"HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0ENTRY\0F\0bad\xff\0DONE\0",
    )
    .unwrap_err();
    assert_eq!(error, "archie: replied with non-UTF-8 names");
}

fn peer_knowing(path: &str, names: &[(&str, bool)]) -> Peer {
    let peer = Peer::new("archie");
    peer.remember(Some(path), &listed(path, names));
    peer
}

#[test]
fn a_listed_directory_does_not_have_to_be_asked_about_again() {
    let peer = peer_knowing("/home/fredrir/projects", &[("my-app", true)]);
    assert!(peer.knows_directory("/home/fredrir/projects"));
    assert!(!peer.knows_directory("/home/fredrir/elsewhere"));
}

#[test]
fn an_entry_of_a_listed_directory_is_known_to_be_there() {
    let peer = peer_knowing(
        "/home/fredrir/projects",
        &[("my-app", true), ("a.txt", false)],
    );
    assert!(peer.knows_entry("/home/fredrir/projects/my-app"));
    assert!(peer.knows_entry("/home/fredrir/projects/a.txt"));
    assert!(!peer.knows_entry("/home/fredrir/projects/gone"));
    assert!(!peer.knows_entry("/home/fredrir/elsewhere/my-app"));
}

#[test]
fn a_directory_that_was_not_there_is_never_remembered_as_being_there() {
    let peer = Peer::new("archie");
    peer.remember(
        Some("/home/fredrir/new"),
        &Listing {
            path: "/home/fredrir/new".into(),
            home: "/home/fredrir".into(),
            entries: Vec::new(),
            missing: true,
        },
    );
    assert!(!peer.knows_directory("/home/fredrir/new"));
}

#[test]
fn canonical_and_requested_paths_share_one_cache_entry() {
    let peer = Peer::new("archie");
    peer.remember(
        Some("/home/fredrir/projects/../notes"),
        &listed("/home/fredrir/notes", &[("old", false)]),
    );
    peer.remember(
        Some("/home/fredrir/notes"),
        &listed("/home/fredrir/notes", &[("new", false)]),
    );

    let alias = peer.cached("/home/fredrir/projects/../notes").unwrap();
    assert_eq!(alias.entries[0].name, "new");
    assert_eq!(alias.path, "/home/fredrir/notes");
}

#[test]
fn refreshing_bypasses_and_updates_the_cache() {
    let peer = peer_knowing("/home/fredrir/projects", &[("old", false)]);
    let calls = AtomicUsize::new(0);
    let refreshed = peer
        .load_with(
            &Target::Absolute("/home/fredrir/projects".into()),
            false,
            |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(listed("/home/fredrir/projects", &[("new", false)]))
            },
        )
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(refreshed.entries[0].name, "new");
    assert_eq!(
        peer.cached("/home/fredrir/projects").unwrap().entries[0].name,
        "new"
    );
}

#[test]
fn a_missing_refresh_invalidates_every_alias() {
    let peer = Peer::new("archie");
    peer.remember(
        Some("/home/fredrir/projects/../notes"),
        &listed("/home/fredrir/notes", &[]),
    );
    let missing = Listing {
        path: "/home/fredrir/projects/../notes".into(),
        home: "/home/fredrir".into(),
        entries: Vec::new(),
        missing: true,
    };
    peer.load_with(
        &Target::Absolute("/home/fredrir/projects/../notes".into()),
        false,
        |_, _| Ok(missing),
    )
    .unwrap();

    assert!(peer.cached("/home/fredrir/projects/../notes").is_none());
    assert!(peer.cached("/home/fredrir/notes").is_none());
}

#[test]
fn foreground_requests_share_an_inflight_fetch() {
    let peer = Peer::new("archie");
    let calls = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first_peer = peer.clone();
    let first_calls = calls.clone();
    let first = std::thread::spawn(move || {
        first_peer.load_with(
            &Target::Absolute("/home/fredrir/projects".into()),
            true,
            |_, _| {
                first_calls.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(listed("/home/fredrir/projects", &[("shared", true)]))
            },
        )
    });
    started_rx.recv().unwrap();
    let second_peer = peer.clone();
    let second_calls = calls.clone();
    let second = std::thread::spawn(move || {
        second_peer.load_with(
            &Target::Absolute("/home/fredrir/projects".into()),
            true,
            |_, _| {
                second_calls.fetch_add(1, Ordering::SeqCst);
                Ok(listed("/home/fredrir/projects", &[("duplicate", true)]))
            },
        )
    });
    release_tx.send(()).unwrap();

    let first = first.join().unwrap().unwrap();
    let second = second.join().unwrap().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(first, second);
}

#[test]
fn detached_prefetch_work_is_deduplicated_and_bounded() {
    let peer = Peer::new("archie");
    assert!(peer.begin_prefetch("/one"));
    assert!(!peer.begin_prefetch("/one"));
    assert!(peer.begin_prefetch("/two"));
    assert!(!peer.begin_prefetch("/three"));

    peer.finish("/one", true);
    assert!(peer.begin_prefetch("/three"));
    peer.finish("/two", true);
    peer.finish("/three", true);
}

#[test]
fn the_listing_uses_nul_records_and_reports_listing_failures() {
    let text = script(&Target::Absolute("/home/f".into()));
    assert!(text.contains("printf 'HOME\\000%s\\000ASK\\000%s\\000'"));
    assert!(text.contains("printf \"ENTRY\\000%s\\000%s\\000\""));
    assert!(text.contains("find . ! -name . -prune"));
    assert!(text.contains("FAIL\\000directory could not be read\\000"));
    assert!(text.contains("target='/home/f'"));
    assert!(text.trim_end().ends_with("exit 0"));
}

#[test]
fn a_home_target_is_expanded_by_the_other_shell_rather_than_this_one() {
    assert_eq!(Target::Home(String::new()).expression(), "\"$HOME\"");
    assert_eq!(
        Target::Home("projects".into()).expression(),
        "\"$HOME\"/'projects'"
    );
    assert_eq!(
        Target::Absolute("/etc/ssh".into()).expression(),
        "'/etc/ssh'"
    );
}

#[cfg(unix)]
#[test]
fn the_shell_protocol_preserves_hidden_and_whitespace_names_and_directory_links() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("folder")).unwrap();
    std::fs::create_dir(root.path().join(".hidden-dir")).unwrap();
    std::fs::write(root.path().join(".hidden-file"), []).unwrap();
    std::fs::write(root.path().join("line\nbreak\tfile"), []).unwrap();
    symlink("folder", root.path().join("linked-folder")).unwrap();
    let target = Target::Absolute(root.path().to_string_lossy().into_owned());
    let output = Command::new("sh")
        .args(["-c", &script(&target)])
        .output()
        .unwrap();
    assert!(output.status.success());
    let listing = parse_reply("local", &output.stdout).unwrap();

    assert!(
        listing
            .entries
            .iter()
            .any(|entry| entry.name == ".hidden-dir" && entry.directory)
    );
    assert!(
        listing
            .entries
            .iter()
            .any(|entry| entry.name == ".hidden-file" && !entry.directory)
    );
    assert!(
        listing
            .entries
            .iter()
            .any(|entry| entry.name == "line\nbreak\tfile" && !entry.directory)
    );
    assert!(
        listing
            .entries
            .iter()
            .any(|entry| entry.name == "linked-folder" && entry.directory)
    );
}

#[cfg(unix)]
#[test]
fn the_shell_protocol_keeps_the_exact_missing_request_and_rejects_files() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing\nplace");
    let missing = missing.to_string_lossy().into_owned();
    let output = Command::new("sh")
        .args(["-c", &script(&Target::Absolute(missing.clone()))])
        .output()
        .unwrap();
    let listing = parse_reply("local", &output.stdout).unwrap();
    assert!(listing.missing);
    assert_eq!(listing.path, missing);

    let file = root.path().join("a-file");
    std::fs::write(&file, []).unwrap();
    let output = Command::new("sh")
        .args([
            "-c",
            &script(&Target::Absolute(file.to_string_lossy().into_owned())),
        ])
        .output()
        .unwrap();
    let error = parse_reply("local", &output.stdout).unwrap_err();
    assert!(error.contains("could not be opened"));

    let beneath_file = file.join("child");
    let output = Command::new("sh")
        .args([
            "-c",
            &script(&Target::Absolute(
                beneath_file.to_string_lossy().into_owned(),
            )),
        ])
        .output()
        .unwrap();
    let error = parse_reply("local", &output.stdout).unwrap_err();
    assert!(error.contains("could not be opened"));
}

#[cfg(target_os = "linux")]
#[test]
fn the_shell_protocol_reports_non_utf8_names_without_loss() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(OsString::from_vec(b"bad\xff".to_vec())),
        [],
    )
    .unwrap();
    let target = Target::Absolute(root.path().to_string_lossy().into_owned());
    let output = Command::new("sh")
        .args(["-c", &script(&target)])
        .output()
        .unwrap();
    let error = parse_reply("local", &output.stdout).unwrap_err();
    assert_eq!(error, "local: replied with non-UTF-8 names");
}
