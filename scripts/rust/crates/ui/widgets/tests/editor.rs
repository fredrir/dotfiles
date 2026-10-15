use ui_widgets::{EditMotion as Motion, EditorBuffer, EditorLayout};

#[test]
fn edits_at_cursor_replaces_selection_and_undoes_complete_typing_runs() {
    let mut editor = EditorBuffer::new("ac");
    editor.move_cursor(Motion::Left, false, 80);
    editor.insert('b');
    assert_eq!(editor.as_str(), "abc");
    editor.move_cursor(Motion::Left, true, 80);
    editor.paste("東京");
    assert_eq!(editor.as_str(), "a東京c");
    editor.undo();
    assert_eq!(editor.as_str(), "abc");
    editor.redo();
    assert_eq!(editor.as_str(), "a東京c");
    editor.delete();
    editor.backspace();
    assert_eq!(editor.as_str(), "a東");
    let mut editor = EditorBuffer::default();
    for c in "typed text".chars() {
        editor.insert(c);
    }
    editor.undo();
    assert_eq!(editor.as_str(), "");
    editor.redo();
    assert_eq!(editor.as_str(), "typed text");
}

#[test]
fn unicode_navigation_and_erasure_keep_combining_marks_and_emoji_together() {
    let mut editor = EditorBuffer::new("åe\u{301}👩‍💻界");
    editor.backspace();
    assert_eq!(editor.as_str(), "åe\u{301}👩‍💻");
    editor.move_cursor(Motion::Left, false, 80);
    assert_eq!(&editor.as_str()[editor.cursor()..], "👩‍💻");
    editor.backspace();
    assert_eq!(editor.as_str(), "å👩‍💻");
    editor.delete();
    assert_eq!(editor.as_str(), "å");
    editor.backspace();
    assert_eq!(editor.as_str(), "");
    let mut editor = EditorBuffer::new("a\n\u{301}b");
    editor.move_cursor(Motion::Start, false, 80);
    editor.move_cursor(Motion::Right, false, 80);
    editor.delete();
    assert_eq!(editor.cursor(), "a\u{301}".len());
    editor.backspace();
    assert_eq!(editor.as_str(), "b");
}

#[test]
fn line_and_word_commands_kill_and_yank_without_touching_other_lines() {
    let mut editor = EditorBuffer::new("subject\nfirst second\nlast");
    editor.move_cursor(Motion::Up, false, 80);
    editor.move_cursor(Motion::LineEnd, false, 80);
    editor.kill(Motion::WordLeft);
    assert_eq!(editor.as_str(), "subject\nfirst \nlast");
    editor.yank();
    assert_eq!(editor.as_str(), "subject\nfirst second\nlast");
    editor.kill(Motion::LineStart);
    assert_eq!(editor.as_str(), "subject\n\nlast");
    editor.yank();
    editor.kill(Motion::LineEnd);
    assert_eq!(editor.as_str(), "subject\nfirst secondlast");
    editor.undo();
    assert_eq!(editor.as_str(), "subject\nfirst second\nlast");
}

#[test]
fn vertical_movement_uses_wrapped_cells_and_preserves_preferred_column() {
    let mut editor = EditorBuffer::new("abcdef\nx\nabcdef");
    editor.move_cursor(Motion::Up, false, 20);
    assert_eq!(editor.cursor(), "abcdef\nx".len());
    editor.move_cursor(Motion::Up, false, 20);
    assert_eq!(editor.cursor(), 6);
    let mut editor = EditorBuffer::new("ab界cdEF");
    editor.move_cursor(Motion::Up, false, 4);
    assert_eq!(editor.cursor(), "ab界".len()); // Start of the preceding visible row.
    let layout = EditorLayout::new(editor.as_str(), 4);
    let (row, column) = layout.position(editor.as_str(), editor.cursor());
    assert_eq!(layout.index(editor.as_str(), row, column), editor.cursor());
}

#[test]
fn pasted_line_endings_are_normalized_and_terminal_controls_are_not_inserted() {
    let mut editor = EditorBuffer::new("end");
    editor.move_cursor(Motion::Start, false, 80);
    editor.paste("å\r\n界\r\t\x1b\x07");
    assert_eq!(editor.as_str(), "å\n界\n\tend");
    editor.undo();
    assert_eq!(editor.as_str(), "end");
}
