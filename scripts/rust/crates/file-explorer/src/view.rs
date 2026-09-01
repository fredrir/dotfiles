use crate::{Directory, DirectoryStatus, Entry, InputKind, Selection};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Plain,
    Strong,
    Muted,
    Accent,
    Success,
    Danger,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub role: Role,
}

impl Span {
    pub fn new(text: impl Into<String>, role: Role) -> Self {
        Self {
            text: text.into(),
            role,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            spans: vec![Span::new(text, Role::Plain)],
        }
    }

    pub fn styled(text: impl Into<String>, role: Role) -> Self {
        Self {
            spans: vec![Span::new(text, role)],
        }
    }

    pub fn from_spans(spans: impl IntoIterator<Item = Span>) -> Self {
        Self {
            spans: spans.into_iter().collect(),
        }
    }
}

pub struct ViewContext<'a, L> {
    pub directory: &'a Directory<L>,
    pub focused: Option<&'a Entry<L>>,
    pub selection: Option<&'a Selection<L>>,
    pub prompt: Option<(&'a str, InputKind)>,
    pub error: Option<&'a str>,
}

pub trait ExplorerView<L> {
    fn header(&self, _context: &ViewContext<'_, L>) -> Vec<Line> {
        Vec::new()
    }

    fn directory_title(&self, context: &ViewContext<'_, L>) -> Line {
        Line::styled(&context.directory.label, Role::Strong)
    }

    fn badge(&self, _context: &ViewContext<'_, L>, _entry: &Entry<L>) -> Option<Line> {
        None
    }

    fn accept_label(&self, _context: &ViewContext<'_, L>) -> String {
        "select".to_string()
    }

    fn state_label(&self, context: &ViewContext<'_, L>, has_matches: bool) -> Option<String> {
        match (&context.directory.status, context.prompt, has_matches) {
            (DirectoryStatus::Missing, _, _) => Some("(not found)".to_string()),
            (DirectoryStatus::Unreadable(reason), _, _) => Some(format!("(unreadable: {reason})")),
            (DirectoryStatus::Present, Some((text, InputKind::Search)), false)
                if !text.is_empty() =>
            {
                Some("(no matches)".to_string())
            }
            (DirectoryStatus::Present, _, false) => Some("(empty)".to_string()),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultView;

impl<L> ExplorerView<L> for DefaultView {}
