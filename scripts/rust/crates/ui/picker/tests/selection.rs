use ui_picker::{Column, Item, Mode, Outcome, Picker, SelectionState, cascade_in};
use ui_terminal::{Event, Key, ScriptedSurface};
use ui_theme::Style;

#[test]
fn multiple_selection_survives_filter_changes_and_rejects_disabled_rows() {
    let mut state = SelectionState::new(
        vec![
            Item::new(1, "one"),
            Item::new(2, "two"),
            Item::new(3, "three").disabled(true),
        ],
        Mode::Multiple,
    );
    state.toggle();
    state.set_query("two");
    state.toggle();
    state.set_query("three");
    state.toggle();
    assert_eq!(state.selected(), [1, 2]);
    assert_eq!(state.selected_count(), 2);
    state.set_query("missing");
    assert_eq!(state.selected(), [1, 2]);
}

#[test]
fn clearing_a_zero_match_filter_restores_the_original_focus() {
    let mut state =
        SelectionState::new(vec![Item::new(1, "one"), Item::new(2, "two")], Mode::Single);
    state.move_by(1);
    state.set_query("missing");
    assert!(state.focused().is_none());
    state.set_query("");
    assert_eq!(state.focused().unwrap().id, 2);
}

#[test]
fn picker_handles_resize_and_cleans_up_on_accept_and_interrupt() {
    let style = Style::plain();
    let mut picker = Picker::new("choose", [Item::new(1, "one"), Item::new(2, "two")], &style)
        .mode(Mode::Multiple);
    let mut terminal = ScriptedSurface::new(
        (40, 10),
        [
            Event::Key(Key::Tab),
            Event::Resize {
                width: 12,
                height: 4,
            },
            Event::Key(Key::Down),
            Event::Key(Key::Tab),
            Event::Key(Key::Enter),
        ],
    );
    assert_eq!(
        picker.run_in(&mut terminal).unwrap(),
        Outcome::Selected(vec![1, 2])
    );
    assert_eq!(terminal.clears, 1);
    assert!(terminal.frames.last().unwrap().len() <= 4);
    let mut terminal = ScriptedSurface::keys((40, 10), [Key::Interrupt]);
    assert_eq!(picker.run_in(&mut terminal).unwrap(), Outcome::Interrupted);
    assert_eq!(terminal.clears, 1);
}

#[test]
fn cascade_backtracking_changes_only_the_selected_branch() {
    let expand = |picks: &[ui_picker::Pick]| match picks.len() {
        0 => Some(Column::new(
            "parent",
            "parent",
            vec![("one".into(), "".into()), ("two".into(), "".into())],
        )),
        1 => Some(Column::new(
            "child",
            "child",
            vec![(picks[0].option.clone(), "detail".into())],
        )),
        _ => None,
    };
    let mut terminal = ScriptedSurface::keys(
        (25, 8),
        [Key::Right, Key::Left, Key::Down, Key::Right, Key::Enter],
    );
    let Outcome::Selected(picks) =
        cascade_in("choose", expand, &Style::plain(), &mut terminal).unwrap()
    else {
        panic!("selection expected");
    };
    assert_eq!(picks[0].option, "two");
    assert_eq!(picks[1].option, "two");
    assert_eq!(terminal.clears, 1);
}

#[test]
fn cascade_handles_wide_unicode_and_small_terminals_without_overflow() {
    let columns = vec![Column {
        kind: "run".into(),
        title: "choose".into(),
        options: vec![("界面 configuration".into(), "some details".into()); 30],
        index: 29,
    }];
    let frame = ui_picker::cascade_frame("bench", &columns, 20, 10, &ui_theme::Style::plain());
    assert!(
        frame
            .iter()
            .all(|line| ui_terminal::text::width(line) <= 19)
    );
    assert!(frame.len() <= 10);
}
