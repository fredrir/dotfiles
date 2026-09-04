use super::*;

fn session(home: &Path) -> Session {
    Session {
        this: Host::Macie,
        peer: Peer::new("archie"),
        home: home.to_path_buf(),
        style: Style::plain(),
        route: None,
        remote_home: "/home/fredrir".to_string(),
    }
}

fn listing(entries: Vec<crate::remote::Entry>) -> Listing {
    Listing {
        path: "/home/fredrir".to_string(),
        home: "/home/fredrir".to_string(),
        entries,
        missing: false,
    }
}

fn request(direction: Direction, path: &str) -> Request {
    Request {
        direction,
        path: Some(path.to_string()),
        remote: None,
        dry_run: false,
        checksum: false,
        all: false,
        yes: false,
        verbose: false,
    }
}

#[test]
fn a_push_needs_its_source_to_be_there() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let missing = home.join("not-here").to_string_lossy().into_owned();

    let error = anchor(&request(Direction::Push, &missing), &home).unwrap_err();
    assert!(error.starts_with("local source does not exist"));
    assert!(error.contains("not-here"));
}

#[test]
fn a_pull_does_not_need_the_path_to_exist_here_first() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let missing = home.join("not-here").to_string_lossy().into_owned();

    let anchored = anchor(&request(Direction::Pull, &missing), &home)
        .unwrap()
        .expect("a named path is always an anchor");
    assert_eq!(anchored.relative, "not-here");
}

#[test]
fn a_push_accepts_a_symlink_whose_target_is_gone() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    std::os::unix::fs::symlink("/nowhere-at-all", home.join("dangling")).unwrap();
    let path = home.join("dangling").to_string_lossy().into_owned();

    assert!(anchor(&request(Direction::Push, &path), &home).is_ok());
}

#[test]
fn a_directory_push_opens_the_mirrored_directory_itself() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let directory = home.join("dotfiles");
    std::fs::create_dir(&directory).unwrap();
    let local = Local {
        absolute: directory,
        relative: "dotfiles".to_string(),
        name: "dotfiles".to_string(),
    };
    let remote = listing(vec![crate::remote::Entry {
        name: "dotfiles".to_string(),
        directory: true,
    }]);

    let start = session(&home).start(Direction::Push, Some(&local), &remote);

    assert_eq!(start.directory, "/home/fredrir/dotfiles");
    assert_eq!(start.name.as_deref(), Some("dotfiles"));
    assert_eq!(start.mirror.as_deref(), Some("/home/fredrir/dotfiles"));

    let missing = session(&home).start(Direction::Push, Some(&local), &listing(Vec::new()));
    assert_eq!(missing.directory, "/home/fredrir/dotfiles");
    assert_eq!(missing.mirror.as_deref(), Some("/home/fredrir/dotfiles"));
}

#[test]
fn file_pushes_conflicts_and_pulls_keep_the_mirror_parent() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let file = home.join("notes.md");
    std::fs::write(&file, []).unwrap();
    let local_file = Local {
        absolute: file,
        relative: "notes.md".to_string(),
        name: "notes.md".to_string(),
    };
    let directory = home.join("dotfiles");
    std::fs::create_dir(&directory).unwrap();
    let local_directory = Local {
        absolute: directory,
        relative: "dotfiles".to_string(),
        name: "dotfiles".to_string(),
    };
    let listing = listing(vec![crate::remote::Entry {
        name: "dotfiles".to_string(),
        directory: false,
    }]);
    let session = session(&home);

    assert_eq!(
        session
            .start(Direction::Push, Some(&local_file), &listing)
            .directory,
        "/home/fredrir"
    );
    assert_eq!(
        session
            .start(Direction::Push, Some(&local_directory), &listing)
            .directory,
        "/home/fredrir"
    );
    assert_eq!(
        session
            .start(Direction::Pull, Some(&local_directory), &listing)
            .directory,
        "/home/fredrir"
    );
}
