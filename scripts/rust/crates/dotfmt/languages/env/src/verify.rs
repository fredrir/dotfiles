use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

use crate::comment;
use crate::config::Config;
use crate::model::{Document, Item, Style};

/// What a document means, independent of its layout: entries in written order
/// with their raw values and quote style, verbatim lines, and comments at the
/// positions they occupy.
pub(crate) struct Fingerprint {
    content: Vec<Content>,
    comments: Vec<Comment>,
}

#[derive(PartialEq, Eq)]
enum Content {
    Entry {
        lead_ws: String,
        export: bool,
        key: String,
        /// Unaligned layout keeps the gaps around `=` byte-for-byte because
        /// they are word boundaries to a shell, not padding.
        before_eq_gap: Option<String>,
        after_eq_gap: Option<String>,
        value: String,
        style: Style,
    },
    Verbatim(String),
    ProtectedBlank(usize),
}

#[derive(PartialEq, Eq)]
struct Comment {
    /// Number of content items written before this comment.
    after: usize,
    trailing: bool,
    text: String,
}

impl Fingerprint {
    pub(crate) fn of(document: &Document<'_>, config: &Config) -> Self {
        let mut fingerprint = Self {
            content: Vec::new(),
            comments: Vec::new(),
        };
        for item in &document.items {
            match item {
                Item::Blank { count, protected } => {
                    if *protected {
                        fingerprint.content.push(Content::ProtectedBlank(*count));
                    }
                }
                Item::Comment { text } => fingerprint.comment(text, config, false),
                Item::Verbatim { raw } => {
                    fingerprint
                        .content
                        .push(Content::Verbatim((*raw).to_owned()));
                }
                Item::Entry(entry) => {
                    fingerprint.content.push(Content::Entry {
                        lead_ws: entry.lead_ws.to_owned(),
                        export: entry.export.is_some(),
                        key: entry.key.to_owned(),
                        before_eq_gap: (!config.align).then(|| entry.before_eq_gap.to_owned()),
                        after_eq_gap: (!config.align).then(|| entry.after_eq_gap.to_owned()),
                        value: entry.value.raw().to_owned(),
                        style: entry.value.style(),
                    });
                    if let Some(comment) = entry.comment {
                        fingerprint.comment(comment, config, true);
                    }
                }
            }
        }
        fingerprint
    }

    fn comment(&mut self, text: &str, config: &Config, trailing: bool) {
        self.comments.push(Comment {
            after: self.content.len(),
            trailing,
            text: comment::format(text, config).into_owned(),
        });
    }
}

pub(crate) fn check(expected: &Fingerprint, actual: &Fingerprint) -> Result<(), Diagnostic> {
    if expected.content != actual.content {
        return Err(internal("formatting would change values"));
    }
    if expected.comments != actual.comments {
        return Err(internal("formatting would change comments"));
    }
    Ok(())
}

fn internal(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}
