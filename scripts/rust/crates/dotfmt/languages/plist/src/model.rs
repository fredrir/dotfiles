pub(crate) const MAX_DEPTH: usize = 100;

pub(crate) struct Document<'a> {
    pub prolog: Vec<Node<'a>>,
    pub root: Element<'a>,
    pub epilog: Vec<Node<'a>>,
}

pub(crate) struct Node<'a> {
    pub kind: NodeKind<'a>,
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
    pub name: &'a str,
    pub attributes: &'a str,
    pub self_closed: bool,
    pub start: usize,
    pub content: Content<'a>,
}

pub(crate) enum Content<'a> {
    Empty,
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
