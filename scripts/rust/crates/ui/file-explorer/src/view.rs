use crate::{Directory, DirectoryStatus, Entry, InputKind, Selection};

pub use ui_widgets::{Line, Role, Span};

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
