use super::*;

fn entry(kind: Kind, counts: Counts) -> Entry {
    Entry {
        path: "some/path".into(),
        label: "untracked",
        kind,
        fate: Fate::Delete,
        counts,
        head: None,
        worktree: Some(EntryKind::Blob),
    }
}

#[test]
fn a_conflict_outranks_the_rest() {
    let change = Change {
        conflict: true,
        deleted: true,
        ..Change::default()
    };
    assert_eq!(change.label(), "unmerged");
}

#[test]
fn the_index_is_asked_before_the_disk() {
    let change = Change {
        added: true,
        typechange: true,
        ..Change::default()
    };
    assert_eq!(change.label(), "added");
    let change = Change {
        deleted: true,
        typechange: true,
        ..Change::default()
    };
    assert_eq!(change.label(), "deleted");
}

#[test]
fn an_executable_bit_is_not_a_change_of_kind() {
    let file = gix::index::entry::Mode::FILE;
    let executable = gix::index::entry::Mode::FILE_EXECUTABLE;
    let link = gix::index::entry::Mode::SYMLINK;
    assert_eq!(family(file), family(executable));
    assert_ne!(family(file), family(link));
}

#[test]
fn directories_are_shown_with_their_slash() {
    assert_eq!(
        entry(Kind::Directory, Counts::Files(2)).shown(),
        "some/path/"
    );
    assert_eq!(entry(Kind::File, Counts::None).shown(), "some/path");
}

#[test]
fn notes_say_what_the_counts_cannot() {
    assert_eq!(
        entry(Kind::Directory, Counts::Files(1)).note().as_deref(),
        Some("1 file")
    );
    assert_eq!(
        entry(Kind::Directory, Counts::Files(9)).note().as_deref(),
        Some("9 files")
    );
    assert_eq!(
        entry(Kind::Repository, Counts::None).note().as_deref(),
        Some("nested repository")
    );
    assert_eq!(
        entry(
            Kind::File,
            Counts::Lines {
                added: 1,
                removed: 0
            }
        )
        .note()
        .as_deref(),
        None
    );
}

#[test]
fn unprintable_names_cannot_forge_a_line() {
    assert_eq!(visible(&"plain".into()), "plain");
    assert_eq!(visible(&"two\nlines".into()), "two^Jlines");
}

#[test]
fn totals_add_up_every_line_not_only_the_shown_ones() {
    let survey = Survey {
        root: PathBuf::new(),
        entries: vec![
            entry(
                Kind::File,
                Counts::Lines {
                    added: 3,
                    removed: 1,
                },
            ),
            entry(Kind::File, Counts::Binary),
            entry(
                Kind::File,
                Counts::Lines {
                    added: 4,
                    removed: 0,
                },
            ),
        ],
    };
    assert_eq!(survey.totals(), (7, 1));
}
