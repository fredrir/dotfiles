use std::borrow::Cow;

use base64::Engine;

use crate::config::{Config, EmptyStyle, IndentType, LineEndings, PrologStyle};
use crate::model::{Content, Document, Element, Node, NodeKind};

const XML_DECLARATION: &str = r#"<?xml version="1.0" encoding="UTF-8"?>"#;
const DOCTYPE: &str = r#"<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#;

pub(crate) fn print(document: &Document<'_>, config: &Config) -> String {
    let mut printer = Printer {
        out: String::new(),
        config,
        unit: match config.indent_type {
            IndentType::Spaces => " ".repeat(config.indent),
            IndentType::Tabs => "\t".to_owned(),
        },
        newline: match config.line_endings {
            LineEndings::Unix => "\n",
            LineEndings::Windows => "\r\n",
        },
    };
    printer.prolog(&document.prolog);
    printer.element(&document.root, 0);
    printer.epilog(&document.epilog);
    printer.finish()
}

struct Printer<'a> {
    out: String,
    config: &'a Config,
    unit: String,
    newline: &'static str,
}

impl Printer<'_> {
    fn prolog(&mut self, nodes: &[Node<'_>]) {
        let mut lines: Vec<(usize, Cow<'_, str>)> = nodes
            .iter()
            .map(|node| (node.blank_before, prolog_text(&node.kind, self.config)))
            .collect();
        if self.config.prolog == PrologStyle::Normalize
            && !nodes
                .iter()
                .any(|node| matches!(node.kind, NodeKind::DocType { .. }))
        {
            let after_declaration = nodes
                .iter()
                .position(|node| matches!(node.kind, NodeKind::Decl { .. }))
                .map_or(0, |index| index + 1);
            lines.insert(after_declaration, (0, Cow::Borrowed(DOCTYPE)));
        }
        for (index, (blank, text)) in lines.iter().enumerate() {
            if index > 0 {
                self.blank(*blank);
            }
            self.line(0, text);
        }
    }

    fn epilog(&mut self, nodes: &[Node<'_>]) {
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 {
                self.blank(node.blank_before.min(self.config.blank_lines));
            }
            self.node(node, 0);
        }
    }

    fn node(&mut self, node: &Node<'_>, depth: usize) {
        match &node.kind {
            NodeKind::Comment { raw } | NodeKind::Pi { raw } => self.line(depth, raw),
            NodeKind::Element(element) => self.element(element, depth),
            NodeKind::Decl { .. } | NodeKind::DocType { .. } => {
                unreachable!("declarations only appear before the root element")
            }
        }
    }

    fn element(&mut self, element: &Element<'_>, depth: usize) {
        match &element.content {
            Content::Empty => self.empty(element, depth),
            Content::Container(children) if children.is_empty() => self.empty(element, depth),
            Content::Container(children) => {
                self.line(depth, &open(element));
                // Apple writes the single property list value at column zero,
                // directly under <plist>, rather than one level in.
                let child_depth = if element.name == "plist" {
                    depth
                } else {
                    depth + 1
                };
                for (index, node) in children.iter().enumerate() {
                    if index > 0 {
                        self.blank(node.blank_before.min(self.config.blank_lines));
                    }
                    self.node(node, child_depth);
                }
                self.line(depth, &close(element));
            }
            Content::Data { decoded, .. } if self.config.data_width > 0 && !decoded.is_empty() => {
                self.data(element, decoded, depth);
            }
            Content::Data { raw, .. } | Content::Text { raw } => {
                let mut line = open(element);
                line.push_str(raw);
                line.push_str(&close(element));
                self.line(depth, &line);
            }
        }
    }

    fn empty(&mut self, element: &Element<'_>, depth: usize) {
        let self_closed = match self.config.self_closing {
            EmptyStyle::Preserve => element.self_closed,
            EmptyStyle::Always => true,
            EmptyStyle::Never => false,
        };
        let line = if self_closed {
            format!("<{}{}/>", element.name, element.attributes)
        } else {
            format!(
                "<{}{}></{}>",
                element.name, element.attributes, element.name
            )
        };
        self.line(depth, &line);
    }

    fn data(&mut self, element: &Element<'_>, decoded: &[u8], depth: usize) {
        self.line(depth, &open(element));
        let encoded = base64::engine::general_purpose::STANDARD.encode(decoded);
        for chunk in encoded.as_bytes().chunks(self.config.data_width) {
            self.line(
                depth + 1,
                std::str::from_utf8(chunk).expect("base64 output is ASCII"),
            );
        }
        self.line(depth, &close(element));
    }

    fn line(&mut self, depth: usize, text: &str) {
        for _ in 0..depth {
            self.out.push_str(&self.unit);
        }
        self.out.push_str(text);
        self.out.push_str(self.newline);
    }

    fn blank(&mut self, count: usize) {
        for _ in 0..count {
            self.out.push_str(self.newline);
        }
    }

    fn finish(mut self) -> String {
        if !self.config.final_newline {
            self.out.truncate(self.out.len() - self.newline.len());
        }
        self.out
    }
}

fn prolog_text<'a>(kind: &NodeKind<'a>, config: &Config) -> Cow<'a, str> {
    match kind {
        NodeKind::Comment { raw } | NodeKind::Pi { raw } => Cow::Borrowed(raw),
        NodeKind::Decl { raw } => {
            if config.prolog == PrologStyle::Normalize {
                Cow::Borrowed(XML_DECLARATION)
            } else {
                Cow::Borrowed(raw)
            }
        }
        NodeKind::DocType { raw } => {
            if config.prolog == PrologStyle::Normalize {
                Cow::Borrowed(DOCTYPE)
            } else {
                Cow::Borrowed(raw)
            }
        }
        NodeKind::Element(_) => unreachable!("the root element is not prolog text"),
    }
}

fn open(element: &Element<'_>) -> String {
    format!("<{}{}>", element.name, element.attributes)
}

fn close(element: &Element<'_>) -> String {
    format!("</{}>", element.name)
}
