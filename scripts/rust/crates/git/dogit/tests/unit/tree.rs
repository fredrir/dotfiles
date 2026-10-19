use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn app() -> (tempfile::TempDir, App) {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new("git")
        .args(["init", "--quiet"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let mut app = App::new(Repo::open(directory.path()).unwrap(), Start::Changes);
    app.snapshot.status.entries = ["a.ts", "src/components/ui/app.tsx", "src/init.lua"]
        .into_iter()
        .map(|path| crate::git::StatusEntry {
            path: path.into(),
            original_path: None,
            index: 'M',
            worktree: 'M',
            unmerged: false,
            untracked: false,
            submodule: None,
        })
        .collect();
    app.rebuild();
    (directory, app)
}

fn draw(app: &mut App) {
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|frame| {
            app.geometry = Some(view::render(
                frame,
                &mut app.view,
                &RenderData {
                    items: &app.nav_items,
                    ..Default::default()
                },
            ));
        })
        .unwrap();
}

#[test]
fn nested_directories_are_independent_and_sort_before_files() {
    let (_directory, mut app) = app();
    let paths: Vec<_> = app
        .entries
        .iter()
        .map(|entry| entry.target.clone())
        .collect();
    assert_eq!(
        paths,
        vec![
            Target::Path("".into(), true),
            Target::Path("src".into(), true),
            Target::Path("src/components".into(), true),
            Target::Path("src/components/ui".into(), true),
            Target::Path("src/components/ui/app.tsx".into(), false),
            Target::Path("src/init.lua".into(), false),
            Target::Path("a.ts".into(), false),
        ]
    );
    app.view.selected = 2;
    app.toggle_folder();
    assert_eq!(app.entries.len(), 5);
    assert!(!app.entries[2].item.file_state.unwrap().expanded);
    assert_eq!(app.nav_summary.items, 3);
    assert_eq!((app.nav_summary.staged, app.nav_summary.unstaged), (3, 3));
    assert!(app.expand_selected());
    assert_eq!(app.entries.len(), 7);
    assert!(app.collapse_selected());
    assert_eq!(app.entries.len(), 5);
}

#[test]
fn first_chevron_click_toggles_unselected_directory_and_hover_does_not_select() {
    let (_directory, mut app) = app();
    draw(&mut app);
    let hit = app
        .geometry
        .as_ref()
        .unwrap()
        .nav_disclosures
        .iter()
        .find(|(row, _)| *row == 2)
        .unwrap()
        .1;
    let event = MouseEvent {
        kind: MouseEventKind::Moved,
        column: hit.x,
        row: hit.y,
        modifiers: KeyModifiers::NONE,
    };
    app.mouse(event);
    assert_eq!(app.view.nav_hovered, Some(2));
    assert_eq!(app.view.selected, 0);
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        ..event
    });
    assert_eq!(app.view.selected, 2);
    assert!(app.collapsed.contains(Path::new("src/components")));
    assert_eq!(app.entries.len(), 5);
    draw(&mut app);
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        ..event
    });
    assert!(!app.collapsed.contains(Path::new("src/components")));
    assert_eq!(app.entries.len(), 7);
}

#[test]
fn folder_animation_follows_path_and_finishes_without_idle_redraws() {
    let (_directory, mut app) = app();
    app.view.selected = 2;
    app.toggle_folder();
    let started = app.folder_animation.as_ref().unwrap().started;
    assert!(app.animate_tree(started + Duration::from_millis(40), false));
    assert_eq!(app.view.folder_animation.unwrap().row, 2);

    // A refresh can insert a directory before the animated row.
    let mut added = app.snapshot.status.entries[0].clone();
    added.path = "other/file.rs".into();
    app.snapshot.status.entries.push(added);
    app.rebuild();
    assert!(app.animate_tree(started + Duration::from_millis(80), false));
    let row = app.view.folder_animation.unwrap().row;
    assert_ne!(row, 2);
    assert_eq!(
        app.entries[row].target,
        Target::Path("src/components".into(), true)
    );

    app.dirty = false;
    assert!(!app.animate_tree(started + view::FolderAnimation::DURATION, false));
    assert!(app.dirty);
    assert!(app.view.folder_animation.is_none());
    assert!(app.folder_animation.is_none());
    app.dirty = false;
    assert!(!app.animate_tree(started + Duration::from_secs(1), false));
    assert!(!app.dirty);

    app.toggle_folder();
    assert!(app.folder_animation.is_some());
    assert!(!app.animate_tree(Instant::now(), true));
    assert!(app.view.folder_animation.is_none());
    assert!(app.folder_animation.is_none());
}
