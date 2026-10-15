use std::io::{self, Write};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;

pub struct Session {
    screen: ui_terminal::Alternate,
    keyboard_flags: Option<KeyboardEnhancementFlags>,
    mouse_capture: ui_terminal::MouseCapture,
    editor_selection: bool,
}

impl Session {
    pub fn new(mouse: ui_terminal::MouseCapture) -> io::Result<Self> {
        let mut session = Self {
            screen: ui_terminal::Alternate::new(mouse)?,
            keyboard_flags: None,
            mouse_capture: mouse,
            editor_selection: false,
        };
        if mouse == ui_terminal::MouseCapture::Disabled {
            execute!(io::stdout(), DisableMouseCapture)?;
        }
        execute!(io::stdout(), EnableBracketedPaste, EnableFocusChange)?;
        session.set_tree_shortcuts(false)?;
        Ok(session)
    }

    pub fn set_tree_shortcuts(&mut self, enabled: bool) -> io::Result<()> {
        let mut flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        if enabled {
            flags |= KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
        }
        if self.keyboard_flags == Some(flags) {
            return Ok(());
        }
        if self.keyboard_flags.is_some() {
            execute!(io::stdout(), PopKeyboardEnhancementFlags)?;
            self.keyboard_flags = None;
        }
        execute!(io::stdout(), PushKeyboardEnhancementFlags(flags))?;
        self.keyboard_flags = Some(flags);
        Ok(())
    }

    pub fn terminal(&mut self) -> &mut ratatui::Terminal<ui_terminal::Backend<io::Stdout>> {
        self.screen.terminal()
    }

    pub fn set_mouse_capture(&mut self, mouse: ui_terminal::MouseCapture) -> io::Result<()> {
        if self.mouse_capture != mouse {
            match mouse {
                ui_terminal::MouseCapture::Enabled => {
                    execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;
                }
                ui_terminal::MouseCapture::Disabled => {
                    execute!(io::stdout(), DisableMouseCapture)?;
                }
            }
            self.mouse_capture = mouse;
        }
        Ok(())
    }

    pub fn set_editor_selection(&mut self, selected: bool) -> io::Result<()> {
        if self.editor_selection != selected {
            // A boolean only: terminal copy handlers can forward their usual
            // shortcut without receiving the draft or requiring enhanced keys.
            let value = if selected { "MQ==" } else { "MA==" };
            let mut output = io::stdout().lock();
            write!(output, "\x1b]1337;SetUserVar=APP_SELECTION={value}\x07")?;
            output.flush()?;
            self.editor_selection = selected;
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.set_editor_selection(false);
        if self.keyboard_flags.is_some() {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(io::stdout(), DisableBracketedPaste, DisableFocusChange);
    }
}
