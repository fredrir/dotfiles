use std::ops::Range;

use oxc_ast::AstKind;
use oxc_ast::ast::{Program, Statement};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;

/// Statement lists a declaration can be inserted into.
pub fn statements<'a>(kind: AstKind<'a>) -> Option<&'a [Statement<'a>]> {
    match kind {
        AstKind::Program(program) => Some(&program.body),
        AstKind::FunctionBody(body) => Some(&body.statements),
        AstKind::BlockStatement(block) => Some(&block.body),
        AstKind::TSModuleBlock(block) => Some(&block.body),
        AstKind::StaticBlock(block) => Some(&block.body),
        _ => None,
    }
}

/// The chain of nodes enclosing `selection`, outermost first.
pub fn enclosing<'a>(program: &Program<'a>, selection: &Range<usize>) -> Vec<AstKind<'a>> {
    let mut walker = Walker {
        selection: selection.clone(),
        stack: Vec::new(),
        entered: Vec::new(),
        deepest: Vec::new(),
    };
    walker.visit_program(program);
    walker.deepest
}

struct Walker<'a> {
    selection: Range<usize>,
    stack: Vec<AstKind<'a>>,
    entered: Vec<bool>,
    deepest: Vec<AstKind<'a>>,
}

impl<'a> Visit<'a> for Walker<'a> {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        let span = kind.span();
        let inside = self.entered.last().is_none_or(|parent| *parent)
            && span.start as usize <= self.selection.start
            && self.selection.end <= span.end as usize;
        self.entered.push(inside);
        if inside {
            self.stack.push(kind);
            if self.stack.len() > self.deepest.len() {
                self.deepest.clone_from(&self.stack);
            }
        }
    }

    fn leave_node(&mut self, _kind: AstKind<'a>) {
        if self.entered.pop() == Some(true) {
            self.stack.pop();
        }
    }
}
