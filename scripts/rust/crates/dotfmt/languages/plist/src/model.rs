/// Deeper documents are rejected instead of formatted. Real property lists are
/// shallow; the limit keeps a recursive printer and fingerprint bounded.
pub(crate) const MAX_DEPTH: usize = 100;

pub(crate) struct Document<'a> {
    /// XML declaration, DOCTYPE, comments and processing instructions before
    /// the root element, in written order.
    pub prolog: Vec<Node<'a>>,
    pub root: Element<'a>,
    /// Comments and processing instructions after the root element.
    pub epilog: Vec<Node<'a>>,
}

pub(crate) struct Node<'a> {
    pub kind: NodeKind<'a>,
    /// Blank lines written between the previous sibling and this node.
    pub blank_before: usize,
}

pub(crate) enum NodeKind<'a> {
    Comment { raw: &'a str },
    Pi { raw: &'a str },
    Decl { raw: &'a str },
    DocType { raw: &'a str },
    Element(Element<'a>),
}

pub(crate) struct Element<'a> {
    /// Element name exactly as written, e.g. `string`.
    pub name: &'a str,
    /// Attribute text between the name and `>` or `/>`, exactly as written.
    pub attributes: &'a str,
    pub self_closed: bool,
    /// Byte offset of the opening `<`, used for diagnostics.
    pub start: usize,
    pub content: Content<'a>,
}

pub(crate) enum Content<'a> {
    /// No content bytes at all, written either self-closed or paired.
    Empty,
    /// Whitespace-only text between child elements is not represented; blank
    /// runs live on [`Node::blank_before`].
    Container(Vec<Node<'a>>),
    Text {
        raw: &'a str,
    },
    Data {
        raw: &'a str,
        decoded: Vec<u8>,
    },
}

impl<'a> Node<'a> {
    pub(crate) fn element(&self) -> Option<&Element<'a>> {
        match &self.kind {
            NodeKind::Element(element) => Some(element),
            _ => None,
        }
    }
}

pub(crate) fn decode_data(raw: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let cleaned: Vec<u8> = raw
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD_PAD_INDIFFERENT
        .decode(cleaned)
        .map_err(|error| error.to_string())
}
