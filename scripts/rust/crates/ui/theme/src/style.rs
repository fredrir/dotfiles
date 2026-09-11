use crate::{Color, ColorMode, Palette, Role, ThemeHandle};
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Style {
    colored: bool,
    palette: Arc<Palette>,
    prefixes: Arc<[String; Role::ALL.len()]>,
    reloadable: bool,
}

impl Style {
    pub fn for_stdout() -> Self {
        Self::for_mode(ColorMode::Auto, io::stdout().is_terminal())
    }

    pub fn for_stdout_with_color(colored: bool) -> Self {
        Self::for_mode(
            if colored {
                ColorMode::Always
            } else {
                ColorMode::Never
            },
            true,
        )
    }

    pub fn for_stderr() -> Self {
        Self::for_mode(ColorMode::Auto, io::stderr().is_terminal())
    }

    pub fn for_mode(mode: ColorMode, terminal: bool) -> Self {
        let mut style = Self::from_palette(Palette::current(), mode, terminal);
        style.reloadable = true;
        style
    }

    pub fn from_palette(palette: Arc<Palette>, mode: ColorMode, terminal: bool) -> Self {
        let colored = mode.enabled(terminal);
        let prefixes = Role::ALL.map(|role| {
            if !colored {
                return String::new();
            }
            let mut codes = Vec::with_capacity(3);
            if role.bold() {
                codes.push("1".to_string());
            }
            let foreground = palette.foreground(role);
            if foreground != Color::Terminal {
                codes.push(foreground.sgr(false));
            }
            let background = palette.background(role);
            if background != Color::Terminal {
                codes.push(background.sgr(true));
            } else if role == Role::Selection {
                codes.push("7".to_string());
            }
            if codes.is_empty() {
                String::new()
            } else {
                format!("\x1b[{}m", codes.join(";"))
            }
        });
        Self {
            colored,
            palette,
            prefixes: Arc::new(prefixes),
            reloadable: false,
        }
    }

    pub fn plain() -> Self {
        Self::from_palette(Arc::new(Palette::default()), ColorMode::Never, false)
    }

    pub fn palette(&self) -> &Arc<Palette> {
        &self.palette
    }

    pub fn colored(&self) -> bool {
        self.colored
    }

    pub fn paint(&self, role: Role, text: &str) -> String {
        self.wrap(&self.prefixes[role as usize], text)
    }

    pub fn role(&self, role: Role, text: &str) -> String {
        self.paint(role, text)
    }

    pub fn bold(&self, text: &str) -> String {
        self.wrap("\x1b[1m", text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint(Role::Muted, text)
    }

    pub fn green(&self, text: &str) -> String {
        self.paint(Role::Success, text)
    }

    pub fn red(&self, text: &str) -> String {
        self.paint(Role::Danger, text)
    }

    pub fn teal(&self, text: &str) -> String {
        self.paint(Role::Theirs, text)
    }

    pub fn code(&self, code: &str, text: &str) -> String {
        if !self.colored || text.is_empty() {
            return text.to_string();
        }
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    fn wrap(&self, prefix: &str, text: &str) -> String {
        if !self.colored || text.is_empty() || prefix.is_empty() {
            return text.to_string();
        }
        let mut output = String::with_capacity(prefix.len() + text.len() + 4);
        output.push_str(prefix);
        output.push_str(text);
        output.push_str("\x1b[0m");
        output
    }
}

#[derive(Clone, Debug)]
pub struct LiveStyle {
    style: Style,
    theme: Option<ThemeHandle>,
}

impl LiveStyle {
    pub fn new(style: &Style) -> Self {
        let mut session = Self {
            style: style.clone(),
            theme: style.reloadable.then(ThemeHandle::discover),
        };
        session.refresh();
        session
    }

    pub fn from_path(style: &Style, path: impl Into<PathBuf>) -> Self {
        let mut session = Self {
            style: style.clone(),
            theme: Some(ThemeHandle::from_path(path)),
        };
        session.refresh();
        session
    }

    pub fn style(&self) -> &Style {
        &self.style
    }

    pub fn poll(&mut self) -> bool {
        self.poll_at(Instant::now())
    }

    pub fn poll_at(&mut self, now: Instant) -> bool {
        if self.theme.as_mut().is_some_and(|theme| theme.poll_at(now)) {
            self.refresh();
            return true;
        }
        false
    }

    fn refresh(&mut self) {
        if let Some(theme) = &self.theme
            && theme.palette().as_ref() != self.style.palette.as_ref()
        {
            self.style = Style::from_palette(
                Arc::clone(theme.palette()),
                if self.style.colored {
                    ColorMode::Always
                } else {
                    ColorMode::Never
                },
                true,
            );
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/style.rs"]
mod tests;
