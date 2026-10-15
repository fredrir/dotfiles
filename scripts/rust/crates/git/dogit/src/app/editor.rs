use super::*;
use ui_widgets::{EditMotion as Motion, EditorBuffer};

pub(super) fn copy_key(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('c' | 'C') => {
            matches!(
                key.modifiers,
                KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::META
            ) || key.modifiers == KeyModifiers::CONTROL | KeyModifiers::SHIFT
        }
        KeyCode::Insert => key.modifiers == KeyModifiers::CONTROL,
        _ => false,
    }
}

pub(super) fn edit_key(editor: &mut EditorBuffer, key: KeyEvent, width: usize) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    let modifiers = key.modifiers;
    let control = modifiers.contains(KeyModifiers::CONTROL);
    let alt = modifiers.contains(KeyModifiers::ALT);
    let command = modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::META);
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    let motion = match key.code {
        KeyCode::Left if command => Some(Motion::LineStart),
        KeyCode::Right if command => Some(Motion::LineEnd),
        KeyCode::Left if control || alt => Some(Motion::WordLeft),
        KeyCode::Right if control || alt => Some(Motion::WordRight),
        KeyCode::Left => Some(Motion::Left),
        KeyCode::Right => Some(Motion::Right),
        KeyCode::Home if control || command => Some(Motion::Start),
        KeyCode::End if control || command => Some(Motion::End),
        KeyCode::Home => Some(Motion::LineStart),
        KeyCode::End => Some(Motion::LineEnd),
        KeyCode::Up if command => Some(Motion::Start),
        KeyCode::Down if command => Some(Motion::End),
        KeyCode::Up => Some(Motion::Up),
        KeyCode::Down => Some(Motion::Down),
        KeyCode::Char('a') if control && !alt => Some(Motion::LineStart),
        KeyCode::Char('e') if control && !alt => Some(Motion::LineEnd),
        KeyCode::Char('b') if control && !alt => Some(Motion::Left),
        KeyCode::Char('f') if control && !alt => Some(Motion::Right),
        KeyCode::Char('p') if control && !alt => Some(Motion::Up),
        KeyCode::Char('n') if control && !alt => Some(Motion::Down),
        KeyCode::Char('b') if alt && !control => Some(Motion::WordLeft),
        KeyCode::Char('f') if alt && !control => Some(Motion::WordRight),
        KeyCode::Char('<') if alt => Some(Motion::Start),
        KeyCode::Char('>') if alt => Some(Motion::End),
        _ => None,
    };
    if let Some(motion) = motion {
        editor.move_cursor(motion, shift, width);
        return;
    }
    match key.code {
        KeyCode::Backspace if command => editor.kill(Motion::LineStart),
        KeyCode::Backspace if control || alt => editor.kill(Motion::WordLeft),
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Delete if command => editor.kill(Motion::LineEnd),
        KeyCode::Delete if control || alt => editor.kill(Motion::WordRight),
        KeyCode::Delete => editor.delete(),
        KeyCode::Enter if !control && !command => editor.insert('\n'),
        KeyCode::Tab if modifiers.is_empty() => editor.insert('\t'),
        KeyCode::Char('j' | 'm') if control && !alt => editor.insert('\n'),
        KeyCode::Char('d') if control && !alt => editor.delete(),
        KeyCode::Char('d') if alt && !control => editor.kill(Motion::WordRight),
        KeyCode::Char('w') if control && !alt => editor.kill(Motion::WordLeft),
        KeyCode::Char('u') if control && !alt => editor.kill(Motion::LineStart),
        KeyCode::Char('k') if control && !alt => editor.kill(Motion::LineEnd),
        KeyCode::Char('y') if control && !alt => editor.yank(),
        KeyCode::Char('z' | 'Z') if (control || command) && shift => editor.redo(),
        KeyCode::Char('z' | '_' | '/' | '7') if (control || command) && !alt => editor.undo(),
        KeyCode::Char('_') if alt => editor.redo(),
        KeyCode::Char(character)
            if !command
                && !modifiers.contains(KeyModifiers::HYPER)
                && ((!control && (!alt || !character.is_ascii())) || (control && alt)) =>
        {
            editor.insert(character)
        }
        _ => {}
    }
}
