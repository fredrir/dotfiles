use std::collections::BTreeMap;

use crate::model::{Content, Document, Element, Node, NodeKind};

/// What a document means, independent of its layout: raw text bytes, element
/// order, duplicate-key shadowing, attributes and every comment.
pub(crate) struct Fingerprint {
    root: NodeFp,
    comments: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
enum NodeFp {
    Text {
        name: String,
        attributes: String,
        raw: String,
    },
    Data {
        attributes: String,
        bytes: Vec<u8>,
    },
    Empty {
        name: String,
        attributes: String,
    },
    Container {
        name: String,
        attributes: String,
        children: Vec<NodeFp>,
        /// Present for `<dict>`: the value each key would resolve to.
        last_wins: Option<BTreeMap<String, NodeFp>>,
    },
}

impl Fingerprint {
    pub(crate) fn of(document: &Document<'_>) -> Self {
        let mut comments = Vec::new();
        for node in &document.prolog {
            comment(node, &mut comments);
        }
        let root = element(&document.root, &mut comments);
        for node in &document.epilog {
            comment(node, &mut comments);
        }
        Self { root, comments }
    }

    pub(crate) fn matches(&self, other: &Self) -> bool {
        self.root == other.root
    }

    pub(crate) fn comments_match(&self, other: &Self) -> bool {
        self.comments == other.comments
    }
}

fn comment(node: &Node<'_>, comments: &mut Vec<String>) {
    if let NodeKind::Comment { raw } = &node.kind {
        comments.push((*raw).to_owned());
    }
}

fn element(element_node: &Element<'_>, comments: &mut Vec<String>) -> NodeFp {
    let name = element_node.name.to_owned();
    let attributes = element_node.attributes.to_owned();
    match &element_node.content {
        Content::Empty => NodeFp::Empty { name, attributes },
        Content::Text { raw } => NodeFp::Text {
            name,
            attributes,
            raw: (*raw).to_owned(),
        },
        Content::Data { decoded, .. } => {
            if decoded.is_empty() {
                NodeFp::Empty { name, attributes }
            } else {
                NodeFp::Data {
                    attributes,
                    bytes: decoded.clone(),
                }
            }
        }
        Content::Container(children) if children.is_empty() => NodeFp::Empty { name, attributes },
        Content::Container(children) => {
            let mut fingerprints = Vec::new();
            let mut pairs = Vec::new();
            let mut key = None;
            for node in children {
                comment(node, comments);
                let Some(child) = node.element() else {
                    continue;
                };
                let fingerprint = element(child, comments);
                if element_node.name == "dict" {
                    if child.name == "key" {
                        key = Some(key_text(child));
                    } else if let Some(key) = key.take() {
                        pairs.push((key, fingerprint.clone()));
                    }
                }
                fingerprints.push(fingerprint);
            }
            let last_wins = (element_node.name == "dict")
                .then(|| pairs.into_iter().collect::<BTreeMap<String, NodeFp>>());
            NodeFp::Container {
                name,
                attributes,
                children: fingerprints,
                last_wins,
            }
        }
    }
}

fn key_text(key: &Element<'_>) -> String {
    match &key.content {
        Content::Empty => String::new(),
        Content::Text { raw } => (*raw).to_owned(),
        Content::Container(_) | Content::Data { .. } => {
            unreachable!("keys are text elements")
        }
    }
}
