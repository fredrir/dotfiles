use super::*;

fn source<'a>(peer: &'a Peer) -> RemoteSource<'a> {
    RemoteSource {
        peer,
        home: "/home/fredrir",
    }
}

fn browser<'a>(direction: Direction, peer: &'a Peer, style: &'a Style) -> Browser<'a> {
    Browser {
        direction,
        peer,
        style,
        this: "macie",
        route: Some(Route::Cable),
        name: Some("my-app".into()),
        local_display: "~/projects/my-app".into(),
        start: "/home/fredrir/projects".into(),
        mirror: Some("/home/fredrir/projects/my-app".into()),
        remote_home: "/home/fredrir".into(),
        home: PathBuf::from("/Users/fredrir"),
        here: PathBuf::from("/Users/fredrir/projects"),
    }
}

fn directory(entries: Vec<Entry<String>>, status: DirectoryStatus) -> Directory<String> {
    Directory {
        location: "/home/fredrir/projects".into(),
        parent: Some("/home/fredrir".into()),
        label: "archie:~/projects".into(),
        entries,
        status,
    }
}

fn context<'a>(
    directory: &'a Directory<String>,
    selection: Option<&'a file_explorer::Selection<String>>,
) -> ViewContext<'a, String> {
    ViewContext {
        directory,
        focused: directory.entries.first(),
        selection,
        prompt: None,
        error: None,
    }
}

#[test]
fn remote_listing_maps_to_opaque_full_locations() {
    let peer = Peer::new("archie");
    let directory = source(&peer)
        .directory(
            "/home/fredrir/projects",
            crate::remote::Listing {
                path: "/home/fredrir/projects".into(),
                home: "/home/fredrir".into(),
                entries: vec![crate::remote::Entry {
                    name: "my-app".into(),
                    directory: true,
                }],
                missing: false,
            },
        )
        .unwrap();

    assert_eq!(directory.location, "/home/fredrir/projects");
    assert_eq!(directory.parent.as_deref(), Some("/home/fredrir"));
    assert_eq!(directory.label, "archie:~/projects");
    assert_eq!(
        directory.entries[0].location,
        "/home/fredrir/projects/my-app"
    );
    assert_eq!(directory.entries[0].kind, EntryKind::Directory);
}

#[test]
fn remote_alias_identity_survives_canonical_listings_and_missing_refreshes() {
    let peer = Peer::new("archie");
    let source = source(&peer);
    let requested = "/home/fredrir/dotfiles";
    let present = source
        .directory(
            requested,
            crate::remote::Listing {
                path: "/srv/homes/fredrir/dotfiles".into(),
                home: "/home/fredrir".into(),
                entries: vec![crate::remote::Entry {
                    name: "scripts".into(),
                    directory: true,
                }],
                missing: false,
            },
        )
        .unwrap();
    assert_eq!(present.location, requested);
    assert_eq!(present.parent.as_deref(), Some("/home/fredrir"));
    assert_eq!(
        present.entries[0].location,
        "/home/fredrir/dotfiles/scripts"
    );

    let missing = source
        .directory(
            requested,
            crate::remote::Listing {
                path: requested.into(),
                home: "/home/fredrir".into(),
                entries: Vec::new(),
                missing: true,
            },
        )
        .unwrap();
    assert_eq!(missing.location, requested);
    assert_eq!(missing.status, DirectoryStatus::Missing);

    let resolved = source
        .directory(
            "/home/fredrir/projects/../dotfiles",
            crate::remote::Listing {
                path: "/home/fredrir/dotfiles".into(),
                home: "/home/fredrir".into(),
                entries: Vec::new(),
                missing: false,
            },
        )
        .unwrap();
    assert_eq!(resolved.location, "/home/fredrir/dotfiles");
}

#[test]
fn a_missing_listing_never_loses_the_requested_destination() {
    let peer = Peer::new("archie");
    let directory = source(&peer)
        .directory(
            "/home/fredrir/new/place",
            crate::remote::Listing {
                path: "/home/fredrir/new/place".into(),
                home: "/home/fredrir".into(),
                entries: Vec::new(),
                missing: true,
            },
        )
        .unwrap();

    assert_eq!(directory.location, "/home/fredrir/new/place");
    assert_eq!(directory.status, DirectoryStatus::Missing);
}

#[test]
fn hcopy_view_keeps_push_and_pull_selection_semantics() {
    let peer = Peer::new("archie");
    let style = Style::plain();
    let item = Entry {
        location: "/home/fredrir/projects/notes.md".into(),
        name: "notes.md".into(),
        kind: EntryKind::File,
    };
    let directory = directory(vec![item.clone()], DirectoryStatus::Present);
    let selection = file_explorer::Selection {
        location: item.location,
        kind: item.kind,
        label: item.name,
    };

    let push = browser(Direction::Push, &peer, &style);
    let pull = browser(Direction::Pull, &peer, &style);
    assert_eq!(
        HcopyView { browser: &push }.chosen(&context(&directory, Some(&selection))),
        Some("/home/fredrir/projects/my-app".to_string())
    );
    assert_eq!(
        HcopyView { browser: &pull }.chosen(&context(&directory, Some(&selection))),
        Some("/home/fredrir/projects/notes.md".to_string())
    );
    assert_eq!(
        HcopyView { browser: &pull }.chosen(&context(&directory, None)),
        None
    );
    let mirror = Directory {
        location: "/home/fredrir/projects/my-app".to_string(),
        parent: Some("/home/fredrir/projects".to_string()),
        label: "archie:~/projects/my-app".to_string(),
        entries: Vec::new(),
        status: DirectoryStatus::Present,
    };
    assert_eq!(
        HcopyView { browser: &push }.chosen(&context(&mirror, None)),
        Some("/home/fredrir/projects/my-app".to_string())
    );
}

#[test]
fn missing_state_wording_is_direction_specific() {
    let peer = Peer::new("archie");
    let style = Style::plain();
    let directory = directory(Vec::new(), DirectoryStatus::Missing);
    let context = context(&directory, None);
    let push = browser(Direction::Push, &peer, &style);
    let pull = browser(Direction::Pull, &peer, &style);
    assert_eq!(
        HcopyView { browser: &push }.state_label(&context, false),
        Some("(not there yet, it will be created)".into())
    );
    assert_eq!(
        HcopyView { browser: &pull }.state_label(&context, false),
        Some("(not found)".into())
    );
}

#[test]
fn mirror_and_replacement_badges_compare_full_locations() {
    let peer = Peer::new("archie");
    let style = Style::plain();
    let browser = browser(Direction::Push, &peer, &style);
    let view = HcopyView { browser: &browser };
    let directory = directory(Vec::new(), DirectoryStatus::Present);
    let context = context(&directory, None);
    let mirror = Entry {
        location: "/home/fredrir/projects/my-app".into(),
        name: "my-app".into(),
        kind: EntryKind::Directory,
    };
    let elsewhere = Entry {
        location: "/home/fredrir/scratch/my-app".into(),
        ..mirror.clone()
    };
    assert_eq!(
        view.badge(&context, &mirror),
        Some(Line::styled("mirror", Role::Muted))
    );
    assert_eq!(
        view.badge(&context, &elsewhere),
        Some(Line::styled("replaces", Role::Muted))
    );
}
