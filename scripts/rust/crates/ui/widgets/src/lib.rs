#![forbid(unsafe_code)]

mod search;
mod text;
mod viewport;

pub use search::{MatchMode, SearchIndex, SearchText};
pub use text::{Line, PromptBuffer, Span};
pub use ui_theme::Role;
pub use viewport::{Navigation, Viewport};

pub struct KeyHint<'a> {
    pub key: &'a str,
    pub label: &'a str,
}

impl<'a> KeyHint<'a> {
    pub const fn new(key: &'a str, label: &'a str) -> Self {
        Self { key, label }
    }
}

pub fn hints(hints: &[KeyHint<'_>]) -> Line {
    let mut spans = Vec::with_capacity(hints.len() * 3);
    for (index, hint) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::new("   ", Role::Plain));
        }
        spans.push(Span::new(hint.key, Role::Accent));
        spans.push(Span::new(format!(" {}", hint.label), Role::Muted));
    }
    Line::from_spans(spans)
}

pub fn detail(label: &str, value: &str) -> Line {
    Line::from_spans([
        Span::new(format!("{label}  "), Role::Muted),
        Span::new(value, Role::Plain),
    ])
}

pub fn choice(label: &str, focused: bool, selected: bool) -> Line {
    Line::from_spans([
        Span::new(if focused { "▸ " } else { "  " }, Role::Accent),
        Span::new(
            if selected { "[✓] " } else { "[ ] " },
            if selected { Role::Success } else { Role::Muted },
        ),
        Span::new(label, if focused { Role::Strong } else { Role::Plain }),
    ])
}
