use std::io;

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;

pub struct Session {
    screen: ui_terminal::Alternate,
    keyboard_pushed: bool,
}

impl Session {
    pub fn new() -> io::Result<Self> {
        let mut session = Self {
            screen: ui_terminal::Alternate::new(ui_terminal::MouseCapture::Enabled)?,
            keyboard_pushed: false,
        };
        execute!(
            io::stdout(),
            EnableBracketedPaste,
            EnableFocusChange,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        session.keyboard_pushed = true;
        Ok(session)
    }

    pub fn terminal(&mut self) -> &mut ratatui::Terminal<ui_terminal::Backend<io::Stdout>> {
        self.screen.terminal()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.keyboard_pushed {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(io::stdout(), DisableBracketedPaste, DisableFocusChange);
    }
}
