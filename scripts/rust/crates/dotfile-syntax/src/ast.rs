//! The generic AST: typed zero-copy views over the CST grammar productions.
//!
//! The AST exposes structure only. Domain schemas assign meaning later; for
//! example `?@font` is valid generic syntax and a domain schema decides
//! whether the optional sigil is legal there. Contextual identifier and
//! binding validation ([`check_ident`], [`check_binding`]) is shared with
//! the parser, which reports violations as `parse/syntax` diagnostics.

use dotfile_source::ByteRange;

use crate::cst::{Cst, Element, NodeId, NodeKind};
use crate::token::{StringData, TokenKind};

/// Words reserved for a future `.dotfile` version and invalid as unquoted
/// identifiers in version 1.
const RESERVED: &[&str] = &[
    "if", "then", "else", "for", "in", "import", "as", "null", "true", "false",
];

/// A contextual name validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameError {
    /// The first character cannot start the name in this position.
    BadStart,
    /// A binding continuation character is invalid.
    BadChar,
    /// The identifier is exactly `.`.
    Dot,
    /// The identifier is exactly `..`.
    DotDot,
    /// The identifier is reserved for a future version.
    Reserved,
}

impl NameError {
    pub fn summary(self) -> &'static str {
        match self {
            Self::BadStart => "invalid leading character in this name",
            Self::BadChar => "invalid character in this binding name",
            Self::Dot => "`.` is not a valid identifier",
            Self::DotDot => "`..` is not a valid identifier",
            Self::Reserved => "reserved word is not a valid identifier",
        }
    }
}

/// Validates a `WORD` in an identifier position:
/// `(ALPHA | DIGIT | "."), { ID_CONT }`, excluding `.`, `..`, and reserved
/// words. The token shape already constrains continuation characters.
pub fn check_ident(text: &str) -> Result<(), NameError> {
    let first = text.chars().next().ok_or(NameError::BadStart)?;
    if !(first.is_ascii_alphabetic() || first.is_ascii_digit() || first == '.') {
        return Err(NameError::BadStart);
    }
    match text {
        "." => return Err(NameError::Dot),
        ".." => return Err(NameError::DotDot),
        _ => {}
    }
    if RESERVED.contains(&text) {
        return Err(NameError::Reserved);
    }
    Ok(())
}

/// Validates a `WORD` in a binding position:
/// `(ALPHA | "_"), { ALPHA | DIGIT | "_" }`.
pub fn check_binding(text: &str) -> Result<(), NameError> {
    let first = text.chars().next().ok_or(NameError::BadStart)?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(NameError::BadStart);
    }
    if !text
        .chars()
        .all(|scalar| scalar.is_ascii_alphanumeric() || scalar == '_')
    {
        return Err(NameError::BadChar);
    }
    Ok(())
}

/// The parsed file: typed views over the root CST node.
#[derive(Clone, Copy, Debug)]
pub struct File<'a> {
    pub(crate) cst: &'a Cst,
    pub(crate) source: &'a [u8],
}

/// Shared view plumbing.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    cst: &'a Cst,
    source: &'a [u8],
    node: NodeId,
}

impl<'a> View<'a> {
    fn child_nodes(&self, kind: NodeKind) -> impl Iterator<Item = View<'a>> + '_ {
        let cst = self.cst;
        let source = self.source;
        cst.node(self.node)
            .children
            .iter()
            .filter_map(move |element| match element {
                Element::Node(id) if cst.node(*id).kind == kind => Some(View {
                    cst,
                    source,
                    node: *id,
                }),
                _ => None,
            })
    }

    fn first_child_node(&self, kind: NodeKind) -> Option<View<'a>> {
        self.child_nodes(kind).next()
    }

    fn tokens_of(&self, kind: TokenKind) -> impl Iterator<Item = (u32, ByteRange)> + '_ {
        let cst = self.cst;
        cst.node(self.node)
            .children
            .iter()
            .filter_map(move |element| match element {
                Element::Token(index) if cst.token(*index).kind == kind => {
                    Some((*index, cst.token(*index).range))
                }
                _ => None,
            })
    }

    fn first_token(&self, kind: TokenKind) -> Option<(u32, ByteRange)> {
        self.tokens_of(kind).next()
    }

    fn text(&self, range: ByteRange) -> &'a str {
        std::str::from_utf8(&self.source[range.start() as usize..range.end() as usize])
            .unwrap_or("")
    }

    fn has_question(&self) -> bool {
        self.first_token(TokenKind::Question).is_some()
    }
}

impl<'a> File<'a> {
    /// Views `cst` as a generic AST over `source` bytes.
    pub fn new(cst: &'a Cst, source: &'a [u8]) -> Self {
        Self { cst, source }
    }

    fn view(&self) -> View<'a> {
        View {
            cst: self.cst,
            source: self.source,
            node: self.cst.root(),
        }
    }

    /// Every top-level entry in source order, including recovered error
    /// regions.
    pub fn entries(&self) -> Vec<Entry<'a>> {
        self.view()
            .child_nodes_all()
            .filter_map(Entry::from_view)
            .collect()
    }
}

impl<'a> View<'a> {
    fn child_nodes_all(&self) -> impl Iterator<Item = View<'a>> + '_ {
        let cst = self.cst;
        let source = self.source;
        self.cst
            .node(self.node)
            .children
            .iter()
            .filter_map(move |element| match element {
                Element::Node(id) => Some(View {
                    cst,
                    source,
                    node: *id,
                }),
                _ => None,
            })
    }
}

/// One generic entry.
#[derive(Clone, Copy, Debug)]
pub enum Entry<'a> {
    Let(LetDecl<'a>),
    Extend(ExtendEntry<'a>),
    Attribute(Attribute<'a>),
    SigilBlock(SigilBlock<'a>),
    Named(NamedEntry<'a>),
    Path(PathEntry<'a>),
    Error,
}

impl<'a> Entry<'a> {
    fn from_view(view: View<'a>) -> Option<Self> {
        match view.cst.node(view.node).kind {
            NodeKind::LetDecl => Some(Self::Let(LetDecl(view))),
            NodeKind::ExtendEntry => Some(Self::Extend(ExtendEntry(view))),
            NodeKind::Attribute => Some(Self::Attribute(Attribute(view))),
            NodeKind::SigilBlock => Some(Self::SigilBlock(SigilBlock(view))),
            NodeKind::NamedEntry => Some(Self::Named(NamedEntry(view))),
            NodeKind::PathEntry => Some(Self::Path(PathEntry(view))),
            NodeKind::Error => Some(Self::Error),
            _ => None,
        }
    }
}

macro_rules! entry_view {
    ($name:ident) => {
        impl<'a> $name<'a> {
            pub fn range(&self) -> ByteRange {
                self.0.cst.node(self.0.node).range
            }
        }
    };
}

/// `@let binding = string_expr`
#[derive(Clone, Copy, Debug)]
pub struct LetDecl<'a>(View<'a>);
entry_view!(LetDecl);

impl<'a> LetDecl<'a> {
    pub fn name(&self) -> Option<&'a str> {
        self.0
            .first_token(TokenKind::Word)
            .map(|(_, range)| self.0.text(range))
    }

    pub fn value(&self) -> Option<StringExpr<'a>> {
        self.0
            .first_child_node(NodeKind::StringExpr)
            .map(StringExpr)
    }
}

/// `@extend namespace/name { ... }`
#[derive(Clone, Copy, Debug)]
pub struct ExtendEntry<'a>(View<'a>);
entry_view!(ExtendEntry);

impl<'a> ExtendEntry<'a> {
    pub fn target(&self) -> Option<QualifiedRef<'a>> {
        self.0
            .first_child_node(NodeKind::QualifiedRef)
            .map(QualifiedRef)
    }

    pub fn block(&self) -> Option<Block<'a>> {
        self.0.first_child_node(NodeKind::Block).map(Block)
    }
}

/// `namespace/name`
#[derive(Clone, Copy, Debug)]
pub struct QualifiedRef<'a>(View<'a>);
entry_view!(QualifiedRef);

impl<'a> QualifiedRef<'a> {
    pub fn namespace(&self) -> Option<&'a str> {
        self.0
            .tokens_of(TokenKind::Word)
            .next()
            .map(|(_, range)| self.0.text(range))
    }

    pub fn name(&self) -> Option<&'a str> {
        self.0
            .tokens_of(TokenKind::Word)
            .nth(1)
            .map(|(_, range)| self.0.text(range))
    }
}

/// `@ident = value`
#[derive(Clone, Copy, Debug)]
pub struct Attribute<'a>(View<'a>);
entry_view!(Attribute);

impl<'a> Attribute<'a> {
    pub fn name(&self) -> Option<&'a str> {
        self.0
            .first_token(TokenKind::Word)
            .map(|(_, range)| self.0.text(range))
    }

    pub fn value(&self) -> Option<Value<'a>> {
        Value::first_from(self.0)
    }
}

/// `[?]@ident { ... }`
#[derive(Clone, Copy, Debug)]
pub struct SigilBlock<'a>(View<'a>);
entry_view!(SigilBlock);

impl<'a> SigilBlock<'a> {
    pub fn optional(&self) -> bool {
        self.0.has_question()
    }

    pub fn name(&self) -> Option<&'a str> {
        self.0
            .first_token(TokenKind::Word)
            .map(|(_, range)| self.0.text(range))
    }

    pub fn block(&self) -> Option<Block<'a>> {
        self.0.first_child_node(NodeKind::Block).map(Block)
    }
}

/// `[?]ident [= value | { ... }]`
#[derive(Clone, Copy, Debug)]
pub struct NamedEntry<'a>(View<'a>);
entry_view!(NamedEntry);

impl<'a> NamedEntry<'a> {
    pub fn optional(&self) -> bool {
        self.0.has_question()
    }

    pub fn name(&self) -> Option<&'a str> {
        self.0
            .first_token(TokenKind::Word)
            .map(|(_, range)| self.0.text(range))
    }

    pub fn value(&self) -> Option<Value<'a>> {
        Value::first_from(self.0)
    }

    pub fn block(&self) -> Option<Block<'a>> {
        self.0.first_child_node(NodeKind::Block).map(Block)
    }
}

/// `[?]PATHREF [{ ... }]`
#[derive(Clone, Copy, Debug)]
pub struct PathEntry<'a>(View<'a>);
entry_view!(PathEntry);

impl<'a> PathEntry<'a> {
    pub fn optional(&self) -> bool {
        self.0.has_question()
    }

    /// The raw source spelling of the path token.
    pub fn raw_path(&self) -> Option<&'a str> {
        self.0
            .first_token(TokenKind::PathRef)
            .map(|(_, range)| self.0.text(range))
    }

    /// The decoded path for a quoted path reference; bare paths decode to
    /// their spelling after `./`.
    pub fn decoded_path(&self) -> Option<String> {
        let (index, range) = self.0.first_token(TokenKind::PathRef)?;
        match self.0.cst.string_data(index) {
            Some(data) => Some(data.decoded()),
            None => Some(self.0.text(range).trim_start_matches("./").to_owned()),
        }
    }

    pub fn block(&self) -> Option<Block<'a>> {
        self.0.first_child_node(NodeKind::Block).map(Block)
    }
}

/// `{ entry, ... }`
#[derive(Clone, Copy, Debug)]
pub struct Block<'a>(View<'a>);
entry_view!(Block);

impl<'a> Block<'a> {
    pub fn entries(&self) -> Vec<Entry<'a>> {
        self.0
            .child_nodes_all()
            .filter_map(Entry::from_view)
            .collect()
    }
}

/// A generic value.
#[derive(Clone, Copy, Debug)]
pub enum Value<'a> {
    String(StringExpr<'a>),
    Reference(Reference<'a>),
    List(List<'a>),
}

impl<'a> Value<'a> {
    fn from_view(view: View<'a>) -> Option<Self> {
        match view.cst.node(view.node).kind {
            NodeKind::StringExpr => Some(Self::String(StringExpr(view))),
            NodeKind::Reference => Some(Self::Reference(Reference(view))),
            NodeKind::List => Some(Self::List(List(view))),
            _ => None,
        }
    }

    fn first_from(view: View<'a>) -> Option<Self> {
        view.child_nodes_all().find_map(Self::from_view)
    }
}

/// `atom { atom }`
#[derive(Clone, Copy, Debug)]
pub struct StringExpr<'a>(View<'a>);
entry_view!(StringExpr);

impl<'a> StringExpr<'a> {
    pub fn atoms(&self) -> Vec<Atom<'a>> {
        let mut atoms = Vec::new();
        for element in &self.0.cst.node(self.0.node).children {
            match *element {
                Element::Token(index) if self.0.cst.token(index).kind == TokenKind::String => {
                    atoms.push(Atom::String {
                        text: self.0.text(self.0.cst.token(index).range),
                        data: self.0.cst.string_data(index),
                    });
                }
                Element::Node(id) if self.0.cst.node(id).kind == NodeKind::VarRef => {
                    atoms.push(Atom::Var(VarRef(View {
                        cst: self.0.cst,
                        source: self.0.source,
                        node: id,
                    })));
                }
                _ => {}
            }
        }
        atoms
    }
}

/// One string atom.
#[derive(Clone, Copy, Debug)]
pub enum Atom<'a> {
    String {
        /// The raw source spelling including quotes.
        text: &'a str,
        data: Option<&'a StringData>,
    },
    Var(VarRef<'a>),
}

/// `$binding`
#[derive(Clone, Copy, Debug)]
pub struct VarRef<'a>(View<'a>);
entry_view!(VarRef);

impl<'a> VarRef<'a> {
    pub fn name(&self) -> Option<&'a str> {
        self.0
            .first_token(TokenKind::Word)
            .map(|(_, range)| self.0.text(range))
    }
}

/// A bare-word value.
#[derive(Clone, Copy, Debug)]
pub struct Reference<'a>(View<'a>);
entry_view!(Reference);

impl<'a> Reference<'a> {
    pub fn name(&self) -> Option<&'a str> {
        self.0
            .first_token(TokenKind::Word)
            .map(|(_, range)| self.0.text(range))
    }
}

/// `[ value, ... ]`
#[derive(Clone, Copy, Debug)]
pub struct List<'a>(View<'a>);
entry_view!(List);

impl<'a> List<'a> {
    pub fn values(&self) -> Vec<Value<'a>> {
        self.0
            .child_nodes_all()
            .filter_map(Value::from_view)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use dotfile_source::{RepoPath, SourceText};

    fn ast(input: &str) -> (Cst, Vec<u8>) {
        let path = RepoPath::new("fixture.dotfile").unwrap();
        let result = parse(&path, &SourceText::from(input));
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        (result.cst, input.as_bytes().to_vec())
    }

    #[test]
    fn name_validation() {
        assert!(check_ident("foo").is_ok());
        assert!(check_ident("7z").is_ok());
        assert!(check_ident(".zshrc").is_ok());
        assert_eq!(check_ident("_foo"), Err(NameError::BadStart));
        assert_eq!(check_ident("."), Err(NameError::Dot));
        assert_eq!(check_ident(".."), Err(NameError::DotDot));
        assert_eq!(check_ident("import"), Err(NameError::Reserved));

        assert!(check_binding("vault").is_ok());
        assert!(check_binding("_vault").is_ok());
        assert_eq!(check_binding("7vault"), Err(NameError::BadStart));
        assert_eq!(check_binding("va-ult"), Err(NameError::BadChar));
    }

    #[test]
    fn typed_entries() {
        let (cst, source) = ast(
            "@let vault = \"~/main\"\n@destination = \"${vault}/.obsidian\"\n?ncdu\n./.zshrc { @destination = \"~/.zshrc\" }\n",
        );
        let file = File::new(&cst, &source);
        let entries = file.entries();
        assert_eq!(entries.len(), 4);
        let Entry::Let(decl) = entries[0] else {
            panic!("expected let declaration");
        };
        assert_eq!(decl.name(), Some("vault"));
        let Entry::Attribute(attribute) = entries[1] else {
            panic!("expected attribute");
        };
        assert_eq!(attribute.name(), Some("destination"));
        let Entry::Named(named) = entries[2] else {
            panic!("expected named entry");
        };
        assert!(named.optional());
        assert_eq!(named.name(), Some("ncdu"));
        let Entry::Path(path) = entries[3] else {
            panic!("expected path entry");
        };
        assert_eq!(path.decoded_path().as_deref(), Some(".zshrc"));
        let block = path.block().unwrap();
        let entries = block.entries();
        let Entry::Attribute(attribute) = entries[0] else {
            panic!("expected attribute");
        };
        assert_eq!(attribute.name(), Some("destination"));
    }

    #[test]
    fn string_expression_atoms() {
        let (cst, source) = ast("@destination = $vault \"/.obsidian\"\n");
        let file = File::new(&cst, &source);
        let entries = file.entries();
        let Entry::Attribute(attribute) = entries[0] else {
            panic!("expected attribute");
        };
        let Some(Value::String(expr)) = attribute.value() else {
            panic!("expected string expression");
        };
        let atoms = expr.atoms();
        assert_eq!(atoms.len(), 2);
        let Atom::Var(var) = atoms[0] else {
            panic!("expected variable");
        };
        assert_eq!(var.name(), Some("vault"));
        let Atom::String { data, .. } = atoms[1] else {
            panic!("expected string");
        };
        assert_eq!(data.unwrap().decoded(), "/.obsidian");
    }
}
