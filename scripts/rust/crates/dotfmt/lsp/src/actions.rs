//! LSP code actions and workspace edits for dotfmt findings and full-file modes.

use std::collections::HashMap;

use dotfmt_core::edit::{Applicability, Finding, Fix};
use lsp_types::{
    AnnotatedTextEdit, ChangeAnnotation, ChangeAnnotationIdentifier, CodeAction, CodeActionKind,
    CodeActionOrCommand, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier, Range,
    TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

use crate::diagnostics;
use crate::documents::Document;
use crate::edits;

pub const FIX_ALL: &str = "source.fixAll.dotfmt";
pub const ORGANIZE_IMPORTS: &str = "source.organizeImports.dotfmt";
pub const FORMAT: &str = "source.format.dotfmt";

#[derive(Clone, Copy, Debug, Default)]
pub struct Capabilities {
    pub versioned: bool,
    pub annotations: bool,
}

pub struct Context<'a> {
    pub document: &'a Document,
    pub uri: &'a Uri,
    pub range: Range,
    pub only: Vec<String>,
    pub trusted: bool,
    pub fixes: &'a [Fix],
    pub format: Option<&'a str>,
    pub fix_all: Option<&'a str>,
    pub organize: Option<&'a str>,
    pub capabilities: Capabilities,
}

pub fn code_actions(context: &Context<'_>) -> Vec<CodeActionOrCommand> {
    let selection = context.document.range(context.range);
    let mut actions = Vec::new();
    let mut source_fixes: Vec<(String, &[dotfmt_core::edit::Edit])> = Vec::new();
    if wants(&context.only, CodeActionKind::QUICKFIX.as_str()) {
        for fix in context.fixes {
            if !context.trusted && fix.applicability == Applicability::Unsafe {
                continue;
            }
            let kind = fix_kind(&fix.kind);
            let source_handled = match kind.as_str() {
                ORGANIZE_IMPORTS => context.organize.is_some(),
                FIX_ALL => context.fix_all.is_some(),
                FORMAT => context.format.is_some(),
                _ => false,
            };
            if source_handled {
                // Source actions are computed as one whole-document edit below;
                // emitting their individual fixes here duplicates them.
                continue;
            }
            if !wants(&context.only, kind.as_str()) {
                continue;
            }
            if !kind.as_str().starts_with(CodeActionKind::QUICKFIX.as_str()) {
                // A source fix supplied without a computed whole-document text
                // is still usable, but identical plans must not duplicate it.
                if source_fixes
                    .iter()
                    .any(|(seen, edits)| seen == kind.as_str() && *edits == fix.edits)
                {
                    continue;
                }
                source_fixes.push((kind.as_str().to_owned(), &fix.edits));
            }
            if kind.as_str().starts_with("quickfix")
                && fix.finding.is_some()
                && !selection
                    .as_ref()
                    .zip(fix.finding.as_ref())
                    .is_some_and(|(selection, finding)| touches(finding, selection))
            {
                continue;
            }
            let title = title(fix);
            let edits = edits::fix_text_edits(context.document, &fix.edits);
            if edits.is_empty() {
                continue;
            }
            let diagnostic = fix.finding.as_ref().and_then(|finding| {
                diagnostics::findings(
                    context.uri,
                    context.document.text(),
                    std::slice::from_ref(finding),
                )
                .into_iter()
                .next()
            });
            let edit = workspace_edit(
                context.document,
                context.uri,
                edits,
                &title,
                fix.applicability == Applicability::Unsafe,
                context.capabilities,
            );
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title,
                kind: Some(kind),
                diagnostics: diagnostic.map(|diagnostic| vec![diagnostic]),
                edit: Some(edit),
                is_preferred: (fix.applicability == Applicability::Safe).then_some(true),
                ..CodeAction::default()
            }));
        }
    }

    source(
        &mut actions,
        context,
        FORMAT,
        "Format with dotfmt",
        context.format,
    );
    source(
        &mut actions,
        context,
        FIX_ALL,
        "Fix all with dotfmt",
        context.fix_all,
    );
    source(
        &mut actions,
        context,
        ORGANIZE_IMPORTS,
        "Organize imports with dotfmt",
        context.organize,
    );
    actions
}

fn source(
    actions: &mut Vec<CodeActionOrCommand>,
    context: &Context<'_>,
    kind: &str,
    title: &str,
    text: Option<&str>,
) {
    if !wants(&context.only, kind) {
        return;
    }
    let Some(text) = text else {
        return;
    };
    if text == context.document.text() {
        return;
    }
    let edit = workspace_edit_text(
        context.document,
        context.uri,
        text,
        title,
        false,
        context.capabilities,
    );
    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: title.into(),
        kind: Some(CodeActionKind::from(kind.to_owned())),
        edit: Some(edit),
        ..CodeAction::default()
    }));
}

/// Fix kinds use the dotfmt-suffixed names the server advertises.
fn fix_kind(kind: &str) -> CodeActionKind {
    match kind {
        "" | "quickfix" => CodeActionKind::QUICKFIX,
        "source.fixAll" => CodeActionKind::from(FIX_ALL),
        "source.organizeImports" => CodeActionKind::from(ORGANIZE_IMPORTS),
        "source.format" => CodeActionKind::from(FORMAT),
        other => CodeActionKind::from(other.to_owned()),
    }
}

fn title(fix: &Fix) -> String {
    match fix.applicability {
        Applicability::Unsafe => format!("{} (unsafe)", fix.title),
        Applicability::Suggestion => format!("{} (suggestion)", fix.title),
        Applicability::Safe => fix.title.clone(),
    }
}

fn touches(finding: &Finding, selection: &std::ops::Range<usize>) -> bool {
    let start = finding.start;
    let end = finding.end.max(start.saturating_add(1));
    if selection.start == selection.end {
        // A cursor exactly at the finding start touches it; the exclusive end
        // remains exclusive so a cursor after the token does not.
        return selection.start >= start && selection.start < end;
    }
    start < selection.end && end > selection.start
}

/// Whether a request for `requested` includes an action of `kind`.
pub fn wants(only: &[String], kind: &str) -> bool {
    only.is_empty()
        || only
            .iter()
            .any(|requested| requested == kind || kind.starts_with(&format!("{requested}.")))
}

pub fn workspace_edit_text(
    document: &Document,
    uri: &Uri,
    text: &str,
    label: &str,
    confirm: bool,
    capabilities: Capabilities,
) -> WorkspaceEdit {
    let edits = edits::whole(document, text);
    workspace_edit(document, uri, edits, label, confirm, capabilities)
}

pub fn workspace_edit(
    document: &Document,
    uri: &Uri,
    edits: Vec<TextEdit>,
    label: &str,
    confirm: bool,
    capabilities: Capabilities,
) -> WorkspaceEdit {
    if !capabilities.versioned {
        return WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), edits)])),
            ..WorkspaceEdit::default()
        };
    }
    let id: ChangeAnnotationIdentifier = "dotfmt.change".into();
    let edits = if capabilities.annotations {
        edits
            .into_iter()
            .map(|text_edit| {
                OneOf::Right(AnnotatedTextEdit {
                    text_edit,
                    annotation_id: id.clone(),
                })
            })
            .collect()
    } else {
        edits.into_iter().map(OneOf::Left).collect()
    };
    WorkspaceEdit {
        document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: Some(document.version),
            },
            edits,
        }])),
        change_annotations: capabilities.annotations.then(|| {
            HashMap::from([(
                id,
                ChangeAnnotation {
                    label: label.into(),
                    needs_confirmation: confirm.then_some(true),
                    description: None,
                },
            )])
        }),
        ..WorkspaceEdit::default()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use dotfmt_core::edit::{Edit, Fix};

    use super::*;

    fn document() -> Document {
        Document::new("a\n".into(), "typescript".into(), 7)
    }

    fn uri() -> Uri {
        Uri::from_str("file:///tmp/a.ts").unwrap()
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            versioned: true,
            annotations: true,
        }
    }

    #[test]
    fn quickfixes_are_versioned_and_unsafe_is_named() {
        let document = document();
        let uri = uri();
        let finding = Finding::new("test", "fix me", dotfmt_core::edit::Severity::Warning, 0, 1);
        let fixes = vec![
            Fix::new(
                "safe fix",
                "quickfix",
                Applicability::Safe,
                vec![Edit::replace(0, 1, "b")],
            )
            .with_finding(finding.clone()),
            Fix::new(
                "risky fix",
                "quickfix",
                Applicability::Unsafe,
                vec![Edit::replace(0, 1, "c")],
            )
            .with_finding(finding),
        ];
        let actions = code_actions(&Context {
            document: &document,
            uri: &uri,
            range: Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 1),
            ),
            only: Vec::new(),
            trusted: true,
            fixes: &fixes,
            format: None,
            fix_all: None,
            organize: None,
            capabilities: capabilities(),
        });
        let titles: Vec<String> = actions
            .iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title.clone()),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(titles.contains(&"safe fix".to_owned()));
        assert!(titles.contains(&"risky fix (unsafe)".to_owned()));
        let CodeActionOrCommand::CodeAction(first) = &actions[0] else {
            panic!("expected code action");
        };
        let WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(edits)),
            change_annotations: Some(annotations),
            ..
        } = first.edit.as_ref().unwrap()
        else {
            panic!("expected versioned edit");
        };
        assert_eq!(edits[0].text_document.version, Some(7));
        assert_eq!(annotations.len(), 1);
    }

    #[test]
    fn untrusted_requests_hide_unsafe_fixes() {
        let document = document();
        let uri = uri();
        let fixes = vec![Fix::new(
            "risky",
            "quickfix",
            Applicability::Unsafe,
            vec![Edit::replace(0, 1, "c")],
        )];
        let actions = code_actions(&Context {
            document: &document,
            uri: &uri,
            range: Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 1),
            ),
            only: Vec::new(),
            trusted: false,
            fixes: &fixes,
            format: None,
            fix_all: None,
            organize: None,
            capabilities: capabilities(),
        });
        assert!(actions.is_empty());
    }

    #[test]
    fn unsuffixed_source_kinds_are_mapped_to_dotfmt_kinds() {
        assert_eq!(fix_kind("source.fixAll").as_str(), FIX_ALL);
        assert_eq!(
            fix_kind("source.organizeImports").as_str(),
            ORGANIZE_IMPORTS
        );
        assert_eq!(fix_kind("source.format").as_str(), FORMAT);
    }

    #[test]
    fn only_matches_specific_and_parent_source_kinds() {
        assert!(wants(&[], FIX_ALL));
        assert!(wants(&["source".into()], FIX_ALL));
        assert!(wants(&["source.fixAll".into()], FIX_ALL));
        assert!(wants(&[FORMAT.into()], FORMAT));
        assert!(!wants(&["source.fixAll".into()], ORGANIZE_IMPORTS));
    }

    #[test]
    fn source_actions_reuse_the_full_document_edit() {
        let document = document();
        let uri = uri();
        let actions = code_actions(&Context {
            document: &document,
            uri: &uri,
            range: Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 0),
            ),
            only: vec![FORMAT.into()],
            trusted: true,
            fixes: &[],
            format: Some("b\n"),
            fix_all: None,
            organize: None,
            capabilities: capabilities(),
        });
        assert_eq!(actions.len(), 1);
    }
}
