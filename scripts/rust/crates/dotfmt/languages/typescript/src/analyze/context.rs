use oxc_ast::AstKind;
use oxc_ast::ast::{Comment, Program, Statement};
use oxc_semantic::{ReferenceFlags, Semantic, SymbolId};
use oxc_span::{SourceType, Span};

use crate::TypeDiagnostic;
use crate::config::Config;
use crate::tsconfig::Tsconfig;

use super::Options;

pub(crate) struct Context<'a, 'p> {
    pub text: &'p str,
    pub path: &'p std::path::Path,
    pub config: &'p Config,
    pub options: &'p Options,
    pub program: &'p Program<'a>,
    pub semantic: &'p Semantic<'a>,
    pub comments: &'p [Comment],
    pub tsconfig: Tsconfig,
    pub is_declaration: bool,
    pub is_typescript: bool,
    pub is_script: bool,
    pub has_eval: bool,
    pub type_diagnostics: Vec<TypeDiagnostic>,
    exported_spans: Vec<Span>,
}

impl<'a, 'p> Context<'a, 'p> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        text: &'p str,
        path: &'p std::path::Path,
        config: &'p Config,
        options: &'p Options,
        source_type: SourceType,
        program: &'p Program<'a>,
        semantic: &'p Semantic<'a>,
        comments: &'p [Comment],
        tsconfig: Tsconfig,
        type_diagnostics: Vec<TypeDiagnostic>,
    ) -> Self {
        let exported_spans = exported_spans(program);
        let has_eval = has_eval(semantic);
        // `source_type_for_path` only marks `.d.ts`; `SourceType::from_path`
        // also recognizes `.d.mts` and `.d.cts` declaration names.
        let is_declaration = source_type.is_typescript_definition()
            || SourceType::from_path(path)
                .is_ok_and(|source_type| source_type.is_typescript_definition());
        Self {
            text,
            path,
            config,
            options,
            program,
            semantic,
            comments,
            tsconfig,
            is_declaration,
            is_typescript: source_type.is_typescript(),
            is_script: source_type.is_script(),
            has_eval,
            type_diagnostics,
            exported_spans,
        }
    }

    pub(crate) fn scoping(&self) -> &oxc_semantic::Scoping {
        self.semantic.scoping()
    }

    pub(crate) fn is_unused(&self, symbol_id: SymbolId) -> bool {
        self.scoping()
            .get_resolved_reference_ids(symbol_id)
            .is_empty()
    }

    pub(crate) fn references_are_type_only(&self, symbol_id: SymbolId) -> bool {
        let scoping = self.scoping();
        let references = scoping.get_resolved_reference_ids(symbol_id);
        !references.is_empty()
            && references.iter().all(|reference_id| {
                let flags = scoping.get_reference(*reference_id).flags();
                flags.contains(ReferenceFlags::Type)
                    && !flags.intersects(
                        ReferenceFlags::Read | ReferenceFlags::Write | ReferenceFlags::ValueAsType,
                    )
            })
    }

    pub(crate) fn is_exported(&self, span: Span) -> bool {
        self.exported_spans
            .iter()
            .any(|exported| span.start == exported.start && span.end == exported.end)
    }

    pub(crate) fn is_ignored(&self, start: usize, end: usize) -> bool {
        self.comments.iter().any(|comment| {
            let comment_start = comment.span.start as usize;
            let comment_end = comment.span.end as usize;
            let marker = self
                .text
                .get(comment_start..comment_end)
                .is_some_and(|text| {
                    text.contains("prettier-ignore") || text.contains("oxfmt-ignore")
                });
            marker
                && (self.leading_comment(comment_end, start)
                    || self.trailing_comment(end, comment_start))
        })
    }

    pub(crate) fn has_adjacent_comment(&self, start: usize, end: usize) -> bool {
        self.comments.iter().any(|comment| {
            let comment_start = comment.span.start as usize;
            let comment_end = comment.span.end as usize;
            self.leading_comment(comment_end, start) || self.trailing_comment(end, comment_start)
        })
    }

    fn leading_comment(&self, comment_end: usize, start: usize) -> bool {
        comment_end <= start
            && self
                .text
                .get(comment_end..start)
                .is_some_and(|text| text.trim().is_empty() && text.matches('\n').count() <= 1)
    }

    fn trailing_comment(&self, end: usize, comment_start: usize) -> bool {
        comment_start >= end
            && self
                .text
                .get(end..comment_start)
                .is_some_and(|text| !text.contains('\n'))
    }

    pub(crate) fn has_comment_in(&self, start: usize, end: usize) -> bool {
        self.comments.iter().any(|comment| {
            let comment_start = comment.span.start as usize;
            let comment_end = comment.span.end as usize;
            comment_start < end && comment_end > start
        })
    }

    pub(crate) fn blocked_by_type_diagnostic(&self, start: usize, end: usize) -> Option<u32> {
        self.type_diagnostics
            .iter()
            .find(|diagnostic| overlaps(diagnostic.start, diagnostic.end, start, end))
            .map(|diagnostic| diagnostic.code)
    }

    pub(crate) fn type_diagnostic_related(
        &self,
        start: usize,
        end: usize,
    ) -> Option<TypeDiagnosticRelated> {
        self.type_diagnostics
            .iter()
            .find(|diagnostic| overlaps(diagnostic.start, diagnostic.end, start, end))
            .map(|diagnostic| TypeDiagnosticRelated {
                start: diagnostic.start,
                end: diagnostic.end,
                message: format!("TypeScript TS{}: {}", diagnostic.code, diagnostic.message),
            })
    }

    pub(crate) fn protected_jsx_names(&self) -> Vec<String> {
        self.tsconfig.protected_jsx_names()
    }

    pub(crate) fn line_start(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    pub(crate) fn line_end(&self, offset: usize) -> usize {
        self.text[offset.min(self.text.len())..]
            .find('\n')
            .map_or(self.text.len(), |index| offset + index)
    }

    /// The byte range that removes a whole statement without touching other
    /// statements that share its line.
    pub(crate) fn statement_edit(&self, start: usize, end: usize) -> (usize, usize) {
        let line_start = self.line_start(start);
        let start = if self.text[line_start..start].trim().is_empty() {
            line_start
        } else {
            start
        };
        let mut end = end;
        while end < self.text.len() && matches!(self.text.as_bytes()[end], b' ' | b'\t' | b';') {
            end += 1;
        }
        if !self.text[end..self.line_end(end)].trim().is_empty() {
            return (start, end);
        }
        if self.text.as_bytes().get(end) == Some(&b'\r') {
            end += 1;
        }
        if self.text.as_bytes().get(end) == Some(&b'\n') {
            end += 1;
        }
        (start, end)
    }
}

pub(crate) struct TypeDiagnosticRelated {
    pub start: usize,
    pub end: usize,
    pub message: String,
}

impl From<TypeDiagnosticRelated> for dotfmt_core::edit::Related {
    fn from(related: TypeDiagnosticRelated) -> Self {
        Self {
            start: related.start,
            end: related.end,
            message: related.message,
        }
    }
}

fn overlaps(diagnostic_start: usize, diagnostic_end: usize, start: usize, end: usize) -> bool {
    if diagnostic_start == diagnostic_end {
        diagnostic_start >= start && diagnostic_start <= end
    } else {
        diagnostic_start < end && diagnostic_end > start
    }
}

fn exported_spans(program: &Program<'_>) -> Vec<Span> {
    let mut spans = Vec::new();
    for statement in &program.body {
        let Statement::ExportDeclaration(declaration) = statement else {
            continue;
        };
        match &declaration.declaration {
            oxc_ast::ast::Declaration::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    collect_binding_spans(&declarator.id, &mut spans);
                }
            }
            declaration => {
                if let Some(identifier) = declaration.id() {
                    spans.push(identifier.span);
                }
            }
        }
    }
    spans
}

fn collect_binding_spans(pattern: &oxc_ast::ast::BindingPattern<'_>, spans: &mut Vec<Span>) {
    match pattern {
        oxc_ast::ast::BindingPattern::BindingIdentifier(identifier) => {
            spans.push(identifier.span);
        }
        oxc_ast::ast::BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_binding_spans(&property.value, spans);
            }
            if let Some(rest) = &pattern.rest {
                collect_binding_spans(&rest.argument, spans);
            }
        }
        oxc_ast::ast::BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_binding_spans(element, spans);
            }
            if let Some(rest) = &pattern.rest {
                collect_binding_spans(&rest.argument, spans);
            }
        }
        oxc_ast::ast::BindingPattern::AssignmentPattern(pattern) => {
            collect_binding_spans(&pattern.left, spans);
        }
    }
}

fn has_eval(semantic: &Semantic<'_>) -> bool {
    semantic.nodes().iter().any(|node| match node.kind() {
        AstKind::CallExpression(call) => {
            matches!(identifier_name(&call.callee), Some("eval" | "Function"))
        }
        AstKind::NewExpression(new) => identifier_name(&new.callee) == Some("Function"),
        AstKind::IdentifierReference(identifier) => {
            matches!(identifier.name.as_str(), "eval" | "Function")
        }
        _ => false,
    })
}

fn identifier_name<'a>(expression: &'a oxc_ast::ast::Expression<'a>) -> Option<&'a str> {
    match expression {
        oxc_ast::ast::Expression::Identifier(identifier) => Some(identifier.name.as_str()),
        oxc_ast::ast::Expression::ParenthesizedExpression(expression) => {
            identifier_name(&expression.expression)
        }
        _ => None,
    }
}
