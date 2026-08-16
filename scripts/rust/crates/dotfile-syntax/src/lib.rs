//! The `.dotfile` v1 syntax layer: byte lexer, lossless CST, recovering
//! parser, and generic AST.
//!
//! There is one authoritative parser. The CLI, compiler, formatter, linter,
//! and LSP all consume the same lossless CST and typed lowering. Parsing
//! always returns a CST with explicit error and missing nodes; any lex or
//! parse error prevents validated compiler IR and lock emission.

mod ast;
mod cst;
mod lexer;
mod parser;
mod token;

pub use ast::{
    Atom, Attribute, Entry, ExtendEntry, File, LetDecl, List, NameError, NamedEntry, PathEntry,
    QualifiedRef, Reference, SigilBlock, StringExpr, Value, VarRef, check_binding, check_ident,
};
pub use cst::{Cst, Element, Node, NodeId, NodeKind};
pub use lexer::lex;
pub use parser::{Parse, parse};
pub use token::{
    EscapeData, Gap, Lexed, StringData, StringSegment, Token, TokenKind, dump_tokens, escape_dump,
};
