//! The deterministic recovering parser.
//!
//! Recursive descent with an event builder over the generic grammar.
//! Recovery follows ADR 0001: file and block bodies synchronize at a comma,
//! a newline run, a matching `}`, or EOF; lists synchronize at a comma, `]`,
//! or EOF; a newline-separated list value inserts one missing comma; a
//! missing closer is inserted at EOF or before a valid outer delimiter; an
//! unexpected closer is consumed into the smallest error node and never
//! closes multiple levels. Every loop consumes a token or inserts one
//! missing token, and depth and work budgets are bounded.

use dotfile_source::{
    ByteRange, Diagnostic, DiagnosticSink, LineIndex, RepoPath, Severity, SourceText, Stage,
};

use crate::cst::{self, Cst, Event, NodeKind};
use crate::lexer::lex_into;
use crate::token::TokenKind;

/// Maximum parser nesting depth (ADR 0001).
const MAX_DEPTH: u32 = 256;

/// The result of parsing one file: the lossless CST, the combined lexer and
/// parser diagnostics in canonical order, and the shared line index.
#[derive(Clone, Debug)]
pub struct Parse {
    pub cst: Cst,
    pub diagnostics: Vec<Diagnostic>,
    pub line_index: LineIndex,
}

impl Parse {
    /// Whether any lex or parse error exists. An erroneous file never
    /// produces validated compiler IR or a lock.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    /// The generic AST view of the parsed file.
    pub fn ast<'a>(&'a self, source: &'a SourceText) -> crate::ast::File<'a> {
        crate::ast::File::new(&self.cst, source.as_bytes())
    }
}

/// Lexes and parses one file. Lexer and parser diagnostics share the
/// retained-diagnostic limit.
pub fn parse(path: &RepoPath, source: &SourceText) -> Parse {
    let line_index = LineIndex::new(source.as_bytes());
    let mut sink = DiagnosticSink::new(path, source, &line_index);
    let (tokens, gaps, strings) = lex_into(source, &mut sink);
    let events = {
        let mut parser = Parser {
            tokens: &tokens,
            gaps: &gaps,
            source,
            pos: 0,
            events: Vec::new(),
            sink: &mut sink,
            delims: Vec::new(),
            depth: 0,
            budget: work_budget(tokens.len()),
            exhausted: false,
        };
        parser.file();
        parser.events
    };
    let cst = cst::build(&events, tokens, gaps, strings, source.len());
    let diagnostics = sink.finish();
    Parse {
        cst,
        diagnostics,
        line_index,
    }
}

fn work_budget(significant_tokens: usize) -> u64 {
    4096u64.saturating_add(64u64.saturating_mul(significant_tokens as u64))
}

/// Recovery synchronization sets.
#[derive(Clone, Copy)]
struct Sync {
    comma: bool,
    newline: bool,
    right_brace: bool,
    right_bracket: bool,
}

const BODY_SYNC: Sync = Sync {
    comma: true,
    newline: true,
    right_brace: true,
    right_bracket: false,
};

const LIST_SYNC: Sync = Sync {
    comma: true,
    newline: false,
    right_brace: false,
    right_bracket: true,
};

struct Parser<'d, 's, 'b> {
    tokens: &'d [crate::token::Token],
    gaps: &'d [crate::token::Gap],
    source: &'d SourceText,
    pos: usize,
    events: Vec<Event>,
    sink: &'b mut DiagnosticSink<'s>,
    /// Closers of enclosing constructs, outermost first.
    delims: Vec<TokenKind>,
    depth: u32,
    budget: u64,
    exhausted: bool,
}

impl Parser<'_, '_, '_> {
    // -- Budgets and basic navigation ------------------------------------

    fn charge(&mut self) {
        if self.exhausted {
            return;
        }
        self.budget = self.budget.saturating_sub(1);
        if self.budget == 0 {
            self.drain();
        }
    }

    /// Work exhaustion: consume the remaining tokens into one lossless tail
    /// error node and emit one resource-limit diagnostic. Every open node is
    /// closed by the unwinding callers, which all observe EOF afterwards.
    fn drain(&mut self) {
        if self.exhausted {
            return;
        }
        self.exhausted = true;
        self.start(NodeKind::Error);
        while self.pos < self.tokens.len() {
            self.bump_raw();
        }
        self.finish();
        let at = self.source.len();
        self.error_at(
            ByteRange::at(at, at).unwrap(),
            "parser work limit was reached",
            "simplify the file",
            Some("resource_limit"),
        );
    }

    fn at_eof(&self) -> bool {
        self.exhausted || self.pos >= self.tokens.len()
    }

    fn peek(&self) -> Option<TokenKind> {
        if self.exhausted {
            return None;
        }
        self.tokens.get(self.pos).map(|token| token.kind)
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.peek() == Some(kind)
    }

    fn bump(&mut self) {
        self.charge();
        if self.exhausted || self.pos >= self.tokens.len() {
            return;
        }
        self.bump_raw();
    }

    fn bump_raw(&mut self) {
        let index = self.pos as u32;
        self.pos += 1;
        self.events.push(Event::Token(index));
    }

    fn current_offset(&self) -> u64 {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].range.start()
        } else {
            self.source.len()
        }
    }

    // -- Events -----------------------------------------------------------

    fn start(&mut self, kind: NodeKind) {
        self.events.push(Event::Start(kind));
    }

    fn finish(&mut self) {
        self.events.push(Event::Finish);
    }

    fn mark(&self) -> usize {
        self.events.len()
    }

    /// Wraps everything emitted since `mark` in a new node.
    fn start_at(&mut self, mark: usize, kind: NodeKind) {
        self.events.insert(mark, Event::Start(kind));
    }

    fn missing(&mut self, kind: TokenKind, summary: &'static str, remedy: &'static str) {
        self.charge();
        let at = self.current_offset();
        self.events.push(Event::Missing { kind, at });
        let range = ByteRange::at(at, self.source.len()).unwrap();
        self.error_at(range, summary, remedy, None);
    }

    // -- Diagnostics ------------------------------------------------------

    fn error_at(
        &mut self,
        range: ByteRange,
        summary: &'static str,
        remedy: &'static str,
        detail: Option<&'static str>,
    ) {
        let span = self.sink.span(range);
        let mut diagnostic = Diagnostic::new(
            "parse/syntax",
            Stage::Parse,
            Severity::Error,
            summary,
            remedy,
            span,
        );
        if let Some(detail) = detail {
            diagnostic = diagnostic.with_detail(detail);
        }
        self.sink.push(diagnostic);
    }

    fn error_here(&mut self, summary: &'static str, remedy: &'static str) {
        let range = match self.tokens.get(self.pos) {
            Some(token) => token.range,
            None => ByteRange::at(self.source.len(), self.source.len()).unwrap(),
        };
        self.error_at(range, summary, remedy, None);
    }

    // -- Recovery ---------------------------------------------------------

    fn in_sync(&self, sync: Sync) -> bool {
        if self.at_eof() {
            return true;
        }
        let kind = self.tokens[self.pos].kind;
        if (sync.comma && kind == TokenKind::Comma)
            || (sync.newline && kind == TokenKind::Newline)
            || (sync.right_brace && kind == TokenKind::RightBrace)
            || (sync.right_bracket && kind == TokenKind::RightBracket)
        {
            return true;
        }
        // A closer matching an enclosing construct synchronizes too, so an
        // unexpected closer never closes multiple levels.
        matches!(kind, TokenKind::RightBrace | TokenKind::RightBracket)
            && self.delims.contains(&kind)
    }

    /// Consumes tokens into one error node until a synchronization point.
    /// When already at a synchronization token, exactly one token is
    /// consumed so every recovery step makes forward progress.
    fn error_node(&mut self, sync: Sync, summary: &'static str, remedy: &'static str) {
        self.charge();
        self.error_here(summary, remedy);
        self.start(NodeKind::Error);
        if !self.at_eof() && self.in_sync(sync) {
            self.bump();
        }
        while !self.in_sync(sync) {
            self.bump();
        }
        self.finish();
    }

    /// Consumes one balanced bracketed construct into an error node without
    /// descending further.
    fn consume_balanced(&mut self) {
        self.start(NodeKind::Error);
        let mut local = 0u32;
        while !self.at_eof() {
            let kind = self.tokens[self.pos].kind;
            match kind {
                TokenKind::LeftBrace | TokenKind::LeftBracket => local += 1,
                TokenKind::RightBrace | TokenKind::RightBracket => {
                    if local == 0 {
                        break;
                    }
                    local -= 1;
                }
                _ => {}
            }
            self.bump();
            if local == 0 {
                break;
            }
        }
        self.finish();
    }

    /// Depth exhaustion: consume the offending construct and emit one
    /// resource-limit diagnostic over its range.
    fn consume_balanced_error(&mut self) {
        let start = self.current_offset();
        self.consume_balanced();
        let end = self.current_offset();
        let range = ByteRange::new(start, end, self.source.len()).unwrap();
        self.error_at(
            range,
            "parser nesting limit was reached",
            "reduce the nesting depth of this construct",
            Some("resource_limit"),
        );
    }

    // -- Grammar ----------------------------------------------------------

    /// `file = newlines, [ entry, { separator, entry } ], newlines, EOF`
    fn file(&mut self) {
        self.start(NodeKind::File);
        self.skip_newlines();
        self.entry_sequence(None);
        self.finish();
    }

    fn skip_newlines(&mut self) -> usize {
        let mut count = 0;
        while self.at(TokenKind::Newline) {
            self.bump();
            count += 1;
        }
        count
    }

    /// Whether the current token ends this entry sequence: EOF, the body's
    /// own closer, or a closer matching an enclosing construct.
    fn at_body_end(&self, end_kind: Option<TokenKind>) -> bool {
        if self.at_eof() || end_kind.is_some_and(|kind| self.at(kind)) {
            return true;
        }
        matches!(
            self.peek(),
            Some(TokenKind::RightBrace | TokenKind::RightBracket)
        ) && self.delims.contains(&self.tokens[self.pos].kind)
    }

    /// Parses entries separated by comma or newline separators until
    /// `end_kind` (a block's `}`) or EOF. A top-level trailing comma is
    /// invalid; a block permits one trailing comma.
    fn entry_sequence(&mut self, end_kind: Option<TokenKind>) {
        if self.at_body_end(end_kind) {
            return;
        }
        self.entry_or_error();
        loop {
            self.charge();
            if self.at_body_end(end_kind) {
                return;
            }
            let skipped = self.skip_newlines();
            if self.at_body_end(end_kind) {
                return;
            }
            if self.at(TokenKind::Comma) {
                self.bump();
                self.skip_newlines();
                if self.at_body_end(end_kind) {
                    if end_kind.is_none() && self.at_eof() {
                        self.error_here("top-level trailing comma", "remove the trailing comma");
                    }
                    return;
                }
                self.entry_or_error();
                continue;
            }
            if skipped > 0 {
                self.entry_or_error();
                continue;
            }
            // Two entries without a separator on one line.
            self.error_node(
                BODY_SYNC,
                "expected a separator between entries",
                "separate entries with a newline or a comma",
            );
        }
    }

    fn entry_or_error(&mut self) {
        self.charge();
        let mark = self.mark();
        match self.peek() {
            Some(TokenKind::AtLet) => self.let_decl(),
            Some(TokenKind::AtExtend) => self.extend_entry(),
            Some(TokenKind::At) => self.at_entry(mark, false),
            Some(TokenKind::Question) => self.optional_entry(mark),
            Some(TokenKind::Word) => self.named_entry(mark),
            Some(TokenKind::PathRef) => self.path_entry(mark),
            Some(TokenKind::RightBrace | TokenKind::RightBracket) => {
                // An unexpected closer is consumed into the smallest error
                // node and never closes multiple levels.
                self.error_here("unexpected closer", "remove the unmatched delimiter");
                self.start(NodeKind::Error);
                self.bump();
                self.finish();
            }
            _ => {
                self.error_node(
                    BODY_SYNC,
                    "expected an entry",
                    "start an entry with a name, `@`, `?`, or a path",
                );
            }
        }
    }

    /// `let_decl = AT_LET, binding, "=", soft_break, string_expr`
    fn let_decl(&mut self) {
        self.start(NodeKind::LetDecl);
        self.bump(); // @let
        if self.at(TokenKind::Word) {
            self.validate_binding();
            self.bump();
        } else {
            self.missing(
                TokenKind::Word,
                "expected a binding name after @let",
                "declare the binding as @let name = \"value\"",
            );
        }
        if self.at(TokenKind::Eq) {
            self.bump();
        } else {
            self.missing(
                TokenKind::Eq,
                "expected `=` in the binding declaration",
                "add `=`",
            );
        }
        self.skip_newlines(); // soft break
        if self.at(TokenKind::String) || self.at(TokenKind::Dollar) {
            self.string_expr();
        } else {
            self.error_here(
                "expected a string expression as the binding value",
                "bindings initialize from string expressions only",
            );
        }
        self.finish();
    }

    /// `extend_entry = AT_EXTEND, qualified_ref, block`
    fn extend_entry(&mut self) {
        self.start(NodeKind::ExtendEntry);
        self.bump(); // @extend
        self.start(NodeKind::QualifiedRef);
        if self.at(TokenKind::Word) {
            self.validate_ident();
            self.bump();
        } else {
            self.missing(
                TokenKind::Word,
                "expected a namespace in the extension target",
                "write entity/name or kind/key",
            );
        }
        if self.at(TokenKind::Slash) {
            self.bump();
        } else {
            self.missing(
                TokenKind::Slash,
                "expected `/` in the extension target",
                "add `/`",
            );
        }
        if self.at(TokenKind::Word) {
            self.validate_ident();
            self.bump();
        } else {
            self.missing(
                TokenKind::Word,
                "expected a name in the extension target",
                "write entity/name or kind/key",
            );
        }
        self.finish(); // QualifiedRef
        if self.at(TokenKind::LeftBrace) {
            self.block();
        } else {
            self.missing(
                TokenKind::LeftBrace,
                "expected a block after the extension target",
                "add a `{ ... }` block",
            );
        }
        self.finish(); // ExtendEntry
    }

    /// `attribute = "@", ident, "=", soft_break, value` or
    /// `sigil_block = [ "?" ], "@", ident, block`
    fn at_entry(&mut self, mark: usize, had_question: bool) {
        self.bump(); // @
        if self.at(TokenKind::Word) {
            self.validate_ident();
            self.bump();
        } else {
            self.missing(TokenKind::Word, "expected a name after `@`", "add a name");
        }
        match self.peek() {
            Some(TokenKind::Eq) => {
                self.start_at(mark, NodeKind::Attribute);
                if had_question {
                    self.error_here(
                        "an optional sigil is not valid on an attribute",
                        "remove the `?`",
                    );
                }
                self.bump();
                self.skip_newlines(); // soft break
                self.value();
            }
            Some(TokenKind::LeftBrace) => {
                self.start_at(mark, NodeKind::SigilBlock);
                self.block();
            }
            _ => {
                self.start_at(mark, NodeKind::SigilBlock);
                self.missing(
                    TokenKind::LeftBrace,
                    "expected `=` or a block after the `@` name",
                    "write @name = value or @name { ... }",
                );
            }
        }
        self.finish();
    }

    /// `[ "?" ], ( "@" ident block | ident [ ... ] | PATHREF [ block ] )`
    fn optional_entry(&mut self, mark: usize) {
        self.bump(); // ?
        match self.peek() {
            Some(TokenKind::At) => self.at_entry(mark, true),
            Some(TokenKind::Word) => self.named_entry(mark),
            Some(TokenKind::PathRef) => self.path_entry(mark),
            _ => {
                self.start_at(mark, NodeKind::Error);
                self.error_here(
                    "expected `@`, a name, or a path after `?`",
                    "follow `?` with an optional demand, resource, or path",
                );
                while !self.in_sync(BODY_SYNC) {
                    self.bump();
                }
                self.finish();
            }
        }
    }

    /// `named_entry = [ "?" ], ident, [ "=", soft_break, value | block ]`
    fn named_entry(&mut self, mark: usize) {
        self.start_at(mark, NodeKind::NamedEntry);
        self.validate_ident();
        self.bump(); // name
        match self.peek() {
            Some(TokenKind::Eq) => {
                self.bump();
                self.skip_newlines(); // soft break
                self.value();
            }
            Some(TokenKind::LeftBrace) => self.block(),
            _ => {}
        }
        self.finish();
    }

    /// `path_entry = [ "?" ], PATHREF, [ block ]`
    fn path_entry(&mut self, mark: usize) {
        self.start_at(mark, NodeKind::PathEntry);
        self.bump(); // path
        if self.at(TokenKind::LeftBrace) {
            self.block();
        }
        self.finish();
    }

    /// `block = "{", body, "}"`
    fn block(&mut self) {
        if self.depth >= MAX_DEPTH {
            self.consume_balanced_error();
            return;
        }
        self.start(NodeKind::Block);
        self.bump(); // {
        self.delims.push(TokenKind::RightBrace);
        self.depth += 1;
        self.skip_newlines();
        self.entry_sequence(Some(TokenKind::RightBrace));
        self.depth -= 1;
        self.delims.pop();
        if self.at(TokenKind::RightBrace) {
            self.bump();
        } else {
            self.missing(
                TokenKind::RightBrace,
                "missing closing brace",
                "close the block with `}`",
            );
        }
        self.finish();
    }

    /// `value = string_expr | reference | list`
    fn value(&mut self) {
        self.charge();
        match self.peek() {
            Some(TokenKind::String | TokenKind::Dollar) => self.string_expr(),
            Some(TokenKind::Word) => {
                self.start(NodeKind::Reference);
                self.validate_ident();
                self.bump();
                self.finish();
            }
            Some(TokenKind::LeftBracket) => self.list(),
            Some(TokenKind::LeftBrace) => {
                // `{...}` is not a first-class value in version 1.
                self.error_here("expected a value", "write a string, a reference, or a list");
                self.consume_balanced();
            }
            _ => {
                self.error_here("expected a value", "write a string, a reference, or a list");
            }
        }
    }

    /// `string_expr = string_atom, { string_atom }`
    ///
    /// Atoms cannot cross a physical newline, and consecutive atoms must
    /// have at least one horizontal whitespace byte between their spans.
    fn string_expr(&mut self) {
        self.start(NodeKind::StringExpr);
        self.string_atom();
        loop {
            self.charge();
            if !(self.at(TokenKind::String) || self.at(TokenKind::Dollar)) {
                break;
            }
            // The gap between the previous token and this one must contain
            // horizontal whitespace; newlines end the expression because NL
            // is a significant token.
            let gap = self.gaps[self.pos].range;
            let spaced = self.source.as_bytes()[gap.start() as usize..gap.end() as usize]
                .iter()
                .any(|byte| matches!(byte, b' ' | b'\t'));
            if !spaced {
                self.error_here(
                    "adjacent string atoms require horizontal whitespace",
                    "separate the atoms with a space or tab",
                );
            }
            self.string_atom();
        }
        self.finish();
    }

    /// `string_atom = STRING | VARREF` where `VARREF = "$", binding`
    fn string_atom(&mut self) {
        if self.at(TokenKind::String) {
            self.bump();
            return;
        }
        self.start(NodeKind::VarRef);
        self.bump(); // $
        if self.at(TokenKind::Word) {
            self.validate_binding();
            self.bump();
        } else {
            self.missing(
                TokenKind::Word,
                "expected a binding name after `$`",
                "write $binding",
            );
        }
        self.finish();
    }

    /// `list = "[", newlines, [ value, { newlines, ",", newlines, value },
    ///   [ trailing_comma ] ], newlines, "]" `
    fn list(&mut self) {
        if self.depth >= MAX_DEPTH {
            self.consume_balanced_error();
            return;
        }
        self.start(NodeKind::List);
        self.bump(); // [
        self.delims.push(TokenKind::RightBracket);
        self.depth += 1;
        self.skip_newlines();
        let mut closed = false;
        if self.at(TokenKind::RightBracket) {
            self.bump();
            closed = true;
        } else {
            self.list_value();
        }
        while !closed {
            self.charge();
            let skipped = self.skip_newlines();
            if self.at(TokenKind::RightBracket) {
                self.bump();
                closed = true;
                break;
            }
            if self.at(TokenKind::Comma) {
                self.bump();
                self.skip_newlines();
                if self.at(TokenKind::RightBracket) {
                    self.bump();
                    closed = true;
                    break;
                }
                if self.at_list_end() {
                    break;
                }
                self.list_value();
                continue;
            }
            if self.at_list_end() {
                break;
            }
            if skipped > 0 && self.at_value_start() {
                // A newline-separated list value recovers as a missing comma.
                self.missing(
                    TokenKind::Comma,
                    "missing comma between list values",
                    "separate list values with commas",
                );
                self.list_value();
                continue;
            }
            self.error_node(
                LIST_SYNC,
                "expected a comma or `]` in the list",
                "separate list values with commas and close the list with `]`",
            );
        }
        self.depth -= 1;
        self.delims.pop();
        if !closed {
            self.missing(
                TokenKind::RightBracket,
                "missing closing bracket",
                "close the list with `]`",
            );
        }
        self.finish();
    }

    /// EOF or a closer matching an enclosing construct (not the current
    /// list's own `]`).
    fn at_list_end(&self) -> bool {
        if self.at_eof() {
            return true;
        }
        match self.peek() {
            Some(TokenKind::RightBrace) => self.delims.contains(&TokenKind::RightBrace),
            Some(TokenKind::RightBracket) => self.delims[..self.delims.len().saturating_sub(1)]
                .contains(&TokenKind::RightBracket),
            _ => false,
        }
    }

    fn at_value_start(&self) -> bool {
        matches!(
            self.peek(),
            Some(TokenKind::String | TokenKind::Dollar | TokenKind::Word | TokenKind::LeftBracket)
        )
    }

    fn list_value(&mut self) {
        if self.at_value_start() {
            self.value();
        } else if self.at(TokenKind::Comma) {
            // A stray comma is diagnosed here and consumed by the loop's
            // comma branch, keeping the error node non-empty elsewhere.
            self.error_here(
                "expected a list value",
                "write a string, a reference, or a nested list",
            );
        } else {
            self.error_node(
                LIST_SYNC,
                "expected a list value",
                "write a string, a reference, or a nested list",
            );
        }
    }

    // -- Contextual name validation ---------------------------------------

    fn validate_ident(&mut self) {
        let Some(token) = self.tokens.get(self.pos) else {
            return;
        };
        let text = self.source.slice_str(token.range).unwrap_or("");
        if let Err(error) = crate::ast::check_ident(text) {
            let span = self.sink.span(token.range);
            self.sink.push(Diagnostic::new(
                "parse/syntax",
                Stage::Parse,
                Severity::Error,
                error.summary(),
                "choose a valid identifier",
                span,
            ));
        }
    }

    fn validate_binding(&mut self) {
        let Some(token) = self.tokens.get(self.pos) else {
            return;
        };
        let text = self.source.slice_str(token.range).unwrap_or("");
        if let Err(error) = crate::ast::check_binding(text) {
            let span = self.sink.span(token.range);
            self.sink.push(Diagnostic::new(
                "parse/syntax",
                Stage::Parse,
                Severity::Error,
                error.summary(),
                "choose a valid binding name",
                span,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(input: &str) -> Parse {
        let path = RepoPath::new("fixture.dotfile").unwrap();
        parse(&path, &SourceText::from(input))
    }

    #[test]
    fn empty_file() {
        let result = parse_str("");
        assert!(!result.has_errors());
        assert_eq!(result.cst.replay(b""), b"");
        assert_eq!(result.cst.dump(b""), "File 0..0\n");
    }

    #[test]
    fn replay_on_valid_documents() {
        for input in [
            "foo\n",
            "foo = \"bar\"\n",
            "?ncdu\n",
            "wezterm {\n    @version = \"1\"\n    hammerspoon\n}\n",
            "@let vault = \"~/Documents/main\"\n@destination = \"${vault}/.obsidian\"\n",
            "@destination = $vault \"/.obsidian\"\n",
            "./.zshrc { @destination = \"~/.zshrc\" }\n",
            "?./generated-report { @deploy = \"none\" }\n",
            "@font {\n    @key = hack_nerd_font\n    @family = [\"Hack Nerd Font Mono\", \"JetBrainsMono Nerd Font\"]\n}\n",
            "@extend font/hack_nerd_font {\n    @pkg = \"font-hack-nerd-font\"\n}\n",
            "a = [\n    \"x\",\n    \"y\",\n]\n",
            "brew =\n\"homebrew\"\n",
            "a, b, c\n",
            "foo, bar\n",
        ] {
            let result = parse_str(input);
            assert_eq!(
                result.cst.replay(input.as_bytes()),
                input.as_bytes(),
                "{input:?}"
            );
            assert!(!result.has_errors(), "{input:?}: {:?}", result.diagnostics);
        }
    }

    #[test]
    fn soft_break_assignment() {
        let result = parse_str("brew =\n\"homebrew\"\n");
        assert!(!result.has_errors());
        let dump = result.cst.dump("brew =\n\"homebrew\"\n".as_bytes());
        assert!(dump.contains("NamedEntry"), "{dump}");
        assert!(dump.contains("StringExpr"), "{dump}");
    }

    #[test]
    fn multi_atom_spacing_rule() {
        let spaced = parse_str("@destination = $vault \"/.obsidian\"\n");
        assert!(!spaced.has_errors());

        let adjacent = parse_str("@destination = $vault\"/.obsidian\"\n");
        assert!(adjacent.has_errors());
        assert!(
            adjacent
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary
                    == "adjacent string atoms require horizontal whitespace")
        );
    }

    #[test]
    fn top_level_trailing_comma_is_invalid() {
        let result = parse_str("foo,\n");
        assert!(result.has_errors());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary == "top-level trailing comma")
        );
    }

    #[test]
    fn block_trailing_comma_is_valid() {
        let result = parse_str("foo {\n    bar,\n}\n");
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
    }

    #[test]
    fn missing_closer_at_eof() {
        let input = "foo {\n    bar\n";
        let result = parse_str(input);
        assert_eq!(result.cst.replay(input.as_bytes()), input.as_bytes());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary == "missing closing brace")
        );
        let dump = result.cst.dump(input.as_bytes());
        assert!(dump.contains("Missing RightBrace"), "{dump}");
    }

    #[test]
    fn unexpected_closer_never_closes_levels() {
        let input = "foo {\n    bar\n}\n}\n";
        let result = parse_str(input);
        assert_eq!(result.cst.replay(input.as_bytes()), input.as_bytes());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary == "unexpected closer")
        );
    }

    #[test]
    fn list_missing_comma_recovery() {
        let input = "a = [\n    \"x\"\n    \"y\"\n]\n";
        let result = parse_str(input);
        assert_eq!(result.cst.replay(input.as_bytes()), input.as_bytes());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary == "missing comma between list values")
        );
        let dump = result.cst.dump(input.as_bytes());
        assert!(dump.contains("Missing Comma"), "{dump}");
    }

    #[test]
    fn list_outer_delimiter_inserts_missing_closer() {
        let input = "foo {\n    a = [\"x\"\n}\n";
        let result = parse_str(input);
        assert_eq!(result.cst.replay(input.as_bytes()), input.as_bytes());
        let dump = result.cst.dump(input.as_bytes());
        assert!(dump.contains("Missing RightBracket"), "{dump}");
        // The block still closes with its own brace.
        assert!(!dump.contains("Missing RightBrace"), "{dump}");
    }

    #[test]
    fn entry_without_separator() {
        let input = "foo bar\n";
        let result = parse_str(input);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary == "expected a separator between entries")
        );
    }

    #[test]
    fn reserved_words_are_invalid_idents() {
        for word in [
            "if", "then", "else", "for", "in", "import", "as", "null", "true", "false",
        ] {
            let result = parse_str(&format!("{word}\n"));
            assert!(result.has_errors(), "{word}");
        }
        // Quoted, they are ordinary data.
        let result = parse_str("a = \"if\"\n");
        assert!(!result.has_errors());
    }

    #[test]
    fn contextual_name_validation() {
        assert!(parse_str("_foo\n").has_errors());
        assert!(parse_str(".\n").has_errors());
        assert!(parse_str("..\n").has_errors());
        assert!(!parse_str("7z\n").has_errors());
        assert!(!parse_str(".zshrc\n").has_errors());
        assert!(parse_str("@let 7x = \"a\"\n").has_errors());
        assert!(!parse_str("@let _x = \"a\"\n").has_errors());
    }

    #[test]
    fn malformed_let_is_a_binding_error() {
        let result = parse_str("@let = \"x\"\n");
        assert!(result.has_errors());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary == "expected a binding name after @let")
        );
    }

    #[test]
    fn optional_entries_own_the_question_sigil() {
        let input = "?ncdu\n?./report { @deploy = \"none\" }\n?@font { @key = hack_nerd_font }\n";
        let result = parse_str(input);
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        let dump = result.cst.dump(input.as_bytes());
        assert!(dump.contains("NamedEntry 0..5"), "{dump}");
        assert!(dump.contains("PathEntry"), "{dump}");
        assert!(dump.contains("SigilBlock"), "{dump}");
    }

    #[test]
    fn optional_attribute_is_an_error() {
        let result = parse_str("?@groups = [macos]\n");
        assert!(result.has_errors());
        assert!(result.diagnostics.iter().any(
            |diagnostic| diagnostic.summary == "an optional sigil is not valid on an attribute"
        ));
    }

    #[test]
    fn depth_limit_consumes_balanced_construct() {
        let mut input = String::from("a = ");
        input.push_str(&"[".repeat(300));
        input.push_str(&"]".repeat(300));
        input.push_str("\nfoo\n");
        let result = parse_str(&input);
        assert_eq!(result.cst.replay(input.as_bytes()), input.as_bytes());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.detail.as_deref() == Some("resource_limit"))
        );
        // The entry after the deep construct still parses.
        let dump = result.cst.dump(input.as_bytes());
        assert!(dump.contains("NamedEntry"), "{dump}");
    }

    #[test]
    fn diagnostic_cap_reports_suppressed() {
        let mut input = String::new();
        for _ in 0..600 {
            input.push_str("} ");
        }
        let result = parse_str(&input);
        assert!(result.diagnostics.len() <= 512);
        let last = result.diagnostics.last().unwrap();
        assert_eq!(last.detail.as_deref(), Some("resource_limit"));
        assert!(last.actual.contains_key("suppressed_diagnostics"));
    }
}
