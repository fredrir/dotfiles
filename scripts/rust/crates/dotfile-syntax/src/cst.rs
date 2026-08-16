//! The immutable lossless concrete syntax tree.
//!
//! The CST owns trivia gaps directly, has explicit error and missing-token
//! nodes, and carries no domain meaning. Replaying gaps and token byte
//! slices reproduces the complete input, including invalid and recovered
//! regions; missing tokens contribute no bytes (ADR 0001).

use dotfile_source::ByteRange;

use crate::token::{Gap, StringData, Token, TokenKind, escape_dump, slice};

/// Generic grammar node kinds. Domain meaning is assigned later by typed
/// lowering; `?@font` is valid generic syntax here.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NodeKind {
    File,
    LetDecl,
    ExtendEntry,
    QualifiedRef,
    Attribute,
    SigilBlock,
    NamedEntry,
    PathEntry,
    Block,
    List,
    StringExpr,
    VarRef,
    Reference,
    /// A recovered region owning every consumed token and gap in its range.
    Error,
}

impl NodeKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::LetDecl => "LetDecl",
            Self::ExtendEntry => "ExtendEntry",
            Self::QualifiedRef => "QualifiedRef",
            Self::Attribute => "Attribute",
            Self::SigilBlock => "SigilBlock",
            Self::NamedEntry => "NamedEntry",
            Self::PathEntry => "PathEntry",
            Self::Block => "Block",
            Self::List => "List",
            Self::StringExpr => "StringExpr",
            Self::VarRef => "VarRef",
            Self::Reference => "Reference",
            Self::Error => "Error",
        }
    }
}

/// Arena index of a CST node.
pub type NodeId = u32;

/// One child of a CST node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Element {
    Node(NodeId),
    /// Index into the token array.
    Token(u32),
    /// A zero-width expected terminal anchored at the byte offset where it
    /// was expected. Missing tokens contribute no bytes.
    Missing {
        kind: TokenKind,
        at: u64,
    },
}

/// One immutable CST node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    pub range: ByteRange,
    pub children: Vec<Element>,
}

/// Parser output events, assembled into a [`Cst`] by [`build`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Start(NodeKind),
    Token(u32),
    Missing { kind: TokenKind, at: u64 },
    Finish,
}

/// The immutable lossless CST plus the token stream it indexes.
#[derive(Clone, Debug)]
pub struct Cst {
    pub tokens: Vec<Token>,
    pub gaps: Vec<Gap>,
    pub strings: Vec<Option<StringData>>,
    nodes: Vec<Node>,
    root: NodeId,
    source_len: u64,
}

impl Cst {
    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn token(&self, index: u32) -> Token {
        self.tokens[index as usize]
    }

    pub fn token_text<'a>(&self, source: &'a [u8], index: u32) -> &'a [u8] {
        slice(source, self.tokens[index as usize].range)
    }

    pub fn string_data(&self, index: u32) -> Option<&StringData> {
        self.strings[index as usize].as_ref()
    }

    pub fn source_len(&self) -> u64 {
        self.source_len
    }

    /// Whether the tree contains a recovered error node or a missing token.
    pub fn has_error(&self) -> bool {
        self.nodes.iter().any(|node| {
            node.kind == NodeKind::Error
                || node
                    .children
                    .iter()
                    .any(|child| matches!(child, Element::Missing { .. }))
        })
    }

    /// Replays gaps and token byte slices; equals the original input.
    pub fn replay(&self, source: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(source.len());
        for (index, token) in self.tokens.iter().enumerate() {
            output.extend_from_slice(slice(source, self.gaps[index].range));
            output.extend_from_slice(slice(source, token.range));
        }
        output.extend_from_slice(slice(source, self.gaps[self.tokens.len()].range));
        output
    }

    /// A deterministic text dump of the tree for golden fixtures.
    pub fn dump(&self, source: &[u8]) -> String {
        let mut output = String::new();
        self.dump_node(&mut output, source, self.root, 0);
        output
    }

    fn dump_node(&self, output: &mut String, source: &[u8], id: NodeId, indent: usize) {
        let node = self.node(id);
        push_indent(output, indent);
        output.push_str(node.kind.name());
        output.push(' ');
        output.push_str(&node.range.to_string());
        output.push('\n');
        for child in &node.children {
            match *child {
                Element::Node(child_id) => self.dump_node(output, source, child_id, indent + 1),
                Element::Token(index) => {
                    let token = self.tokens[index as usize];
                    push_indent(output, indent + 1);
                    output.push_str(token.kind.name());
                    output.push(' ');
                    output.push_str(&token.range.to_string());
                    output.push(' ');
                    output.push_str(&escape_dump(slice(source, token.range)));
                    output.push('\n');
                }
                Element::Missing { kind, at } => {
                    push_indent(output, indent + 1);
                    output.push_str("Missing ");
                    output.push_str(kind.name());
                    output.push(' ');
                    output.push_str(&at.to_string());
                    output.push('\n');
                }
            }
        }
    }
}

fn push_indent(output: &mut String, indent: usize) {
    for _ in 0..indent {
        output.push_str("  ");
    }
}

/// Assembles parser events into the immutable CST. The final `Finish` must
/// close the root `File` node.
pub fn build(
    events: &[Event],
    tokens: Vec<Token>,
    gaps: Vec<Gap>,
    strings: Vec<Option<StringData>>,
    source_len: u64,
) -> Cst {
    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<(NodeKind, Vec<Element>)> = Vec::new();
    for event in events {
        match *event {
            Event::Start(kind) => stack.push((kind, Vec::new())),
            Event::Token(index) => stack
                .last_mut()
                .expect("token outside a node")
                .1
                .push(Element::Token(index)),
            Event::Missing { kind, at } => stack
                .last_mut()
                .expect("missing token outside a node")
                .1
                .push(Element::Missing { kind, at }),
            Event::Finish => {
                let (kind, children) = stack.pop().expect("unbalanced finish");
                let range = node_range(&children, &tokens, &nodes, source_len);
                let id = nodes.len() as NodeId;
                nodes.push(Node {
                    kind,
                    range,
                    children,
                });
                match stack.last_mut() {
                    Some((_, parent)) => parent.push(Element::Node(id)),
                    None => {
                        debug_assert_eq!(kind, NodeKind::File, "root must be the file node");
                        debug_assert_eq!(id as usize + 1, nodes.len());
                        return Cst {
                            tokens,
                            gaps,
                            strings,
                            nodes,
                            root: id,
                            source_len,
                        };
                    }
                }
            }
        }
    }
    panic!("parser events did not close the root node");
}

fn node_range(
    children: &[Element],
    tokens: &[Token],
    nodes: &[Node],
    source_len: u64,
) -> ByteRange {
    let mut range: Option<ByteRange> = None;
    for child in children {
        let child_range = match *child {
            Element::Token(index) => tokens[index as usize].range,
            Element::Node(id) => nodes[id as usize].range,
            Element::Missing { at, .. } => ByteRange::at(at, source_len).unwrap(),
        };
        range = Some(match range {
            Some(existing) => existing.cover(child_range),
            None => child_range,
        });
    }
    range.unwrap_or_else(|| ByteRange::at(0, source_len).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tokens_contribute_no_bytes() {
        let source = b"foo";
        let tokens = vec![Token {
            kind: TokenKind::Word,
            range: ByteRange::new(0, 3, 3).unwrap(),
        }];
        let gaps = vec![
            Gap {
                range: ByteRange::new(0, 0, 3).unwrap(),
            },
            Gap {
                range: ByteRange::new(3, 3, 3).unwrap(),
            },
        ];
        let events = [
            Event::Start(NodeKind::File),
            Event::Start(NodeKind::NamedEntry),
            Event::Token(0),
            Event::Finish,
            Event::Missing {
                kind: TokenKind::RightBrace,
                at: 3,
            },
            Event::Finish,
        ];
        let cst = build(&events, tokens, gaps, vec![None], 3);
        assert_eq!(cst.replay(source), source);
        assert!(cst.has_error());
        let root = cst.node(cst.root());
        assert_eq!(root.range, ByteRange::new(0, 3, 3).unwrap());
    }
}
