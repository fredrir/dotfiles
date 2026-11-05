use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrayExpression, JSXElement, JSXFragment, ObjectExpression, Program, Statement,
};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, SourceType};

use crate::dialect::Dialect;

/// Expands a byte range to the smallest syntax node that is safe to format.
///
/// The returned range is always a contiguous byte range within `input`.
/// Statements, JSX elements, fragments, object expressions, and array
/// expressions are candidates; the whole program covers selections outside
/// those nodes. Zero-width selections expand to a statement or JSX node.
/// Parse failures and reversed ranges return `None`.
pub fn expand_range(
    path: &Path,
    input: &str,
    start: usize,
    end: usize,
    dialect: Dialect,
) -> Option<(usize, usize)> {
    if start > end || crate::source::nesting_exceeded(input) {
        return None;
    }
    let zero_width = start == end;
    let start = char_boundary(input, start, false);
    let end = if zero_width {
        start
    } else {
        char_boundary(input, end, true)
    };
    if start > end || end > input.len() {
        return None;
    }
    let source_type = source_type(path, dialect);
    let allocator = Allocator::default();
    let parsed = oxc_parser::Parser::new(&allocator, input, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let mut expansion = Expansion {
        start,
        end,
        zero_width,
        best: None,
    };
    expansion.visit_program(&parsed.program);
    expansion
        .best
        .or_else(|| program_range(&parsed.program, input.len()))
}

struct Expansion {
    start: usize,
    end: usize,
    zero_width: bool,
    best: Option<(usize, usize)>,
}

impl Expansion {
    fn consider(&mut self, span: oxc_span::Span, zero_width: bool) {
        if self.zero_width && !zero_width {
            return;
        }
        let start = span.start as usize;
        let end = span.end as usize;
        if start > self.start || end < self.end {
            return;
        }
        let length = end - start;
        let best_length = self
            .best
            .map_or(usize::MAX, |(best_start, best_end)| best_end - best_start);
        if length < best_length {
            self.best = Some((start, end));
        }
    }

    fn consider_statement(&mut self, statement: &Statement<'_>) {
        self.consider(statement.span(), true);
    }

    fn consider_jsx<T: GetSpan>(&mut self, node: &T) {
        self.consider(node.span(), true);
    }

    fn consider_expression<T: GetSpan>(&mut self, node: &T) {
        self.consider(node.span(), false);
    }
}

impl<'a> Visit<'a> for Expansion {
    fn visit_statement(&mut self, statement: &Statement<'a>) {
        self.consider_statement(statement);
        oxc_ast_visit::walk::walk_statement(self, statement);
    }

    fn visit_jsx_element(&mut self, element: &JSXElement<'a>) {
        self.consider_jsx(element);
        oxc_ast_visit::walk::walk_jsx_element(self, element);
    }

    fn visit_jsx_fragment(&mut self, fragment: &JSXFragment<'a>) {
        self.consider_jsx(fragment);
        oxc_ast_visit::walk::walk_jsx_fragment(self, fragment);
    }

    fn visit_object_expression(&mut self, expression: &ObjectExpression<'a>) {
        self.consider_expression(expression);
        oxc_ast_visit::walk::walk_object_expression(self, expression);
    }

    fn visit_array_expression(&mut self, expression: &ArrayExpression<'a>) {
        self.consider_expression(expression);
        oxc_ast_visit::walk::walk_array_expression(self, expression);
    }
}

fn source_type(path: &Path, dialect: Dialect) -> SourceType {
    dialect
        .resolve(Dialect::Auto, path)
        .source_type_for_path(path)
}

fn program_range(program: &Program<'_>, input_len: usize) -> Option<(usize, usize)> {
    let start = program.span.start as usize;
    let end = program.span.end as usize;
    (start <= end && end <= input_len).then_some((start, end))
}

fn char_boundary(input: &str, offset: usize, round_up: bool) -> usize {
    let mut offset = offset.min(input.len());
    if input.is_char_boundary(offset) {
        return offset;
    }
    if !round_up {
        while offset > 0 && !input.is_char_boundary(offset) {
            offset -= 1;
        }
        return offset;
    }
    while offset < input.len() && !input.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}
