use super::*;

#[test]
fn scripted_terminal_replays_keys_and_records_frames() {
    let mut terminal = ScriptedTerminal::new(
        Size {
            width: 31,
            height: 9,
        },
        [Key::Down, Key::Enter],
    );

    terminal.draw(&["first".into(), "second".into()]).unwrap();

    assert_eq!(
        terminal.size(),
        Size {
            width: 31,
            height: 9
        }
    );
    assert_eq!(terminal.read_key().unwrap(), Key::Down);
    assert_eq!(terminal.read_key().unwrap(), Key::Enter);
    assert_eq!(terminal.read_key().unwrap(), Key::Interrupt);
    assert_eq!(terminal.frames, vec![vec!["first", "second"]]);
}

#[test]
fn scripted_terminal_records_every_clear() {
    let mut terminal = ScriptedTerminal::new(Size::default(), []);

    terminal.clear().unwrap();
    terminal.clear().unwrap();

    assert_eq!(terminal.clears, 2);
}
