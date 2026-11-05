use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::model::{Content, Document, Element, MAX_DEPTH, Node, NodeKind, decode_data};
use crate::source::line_column;

#[derive(Clone, Copy)]
enum TokenKind<'a> {
    Start { name: &'a str, raw: &'a str },
    Empty { name: &'a str, raw: &'a str },
    End,
    Text(&'a str),
    CData,
    GeneralRef,
    Comment(&'a str),
    Pi(&'a str),
    Decl(&'a str),
    DocType(&'a str),
}

#[derive(Clone, Copy)]
struct Token<'a> {
    kind: TokenKind<'a>,
    start: usize,
}

pub(crate) fn parse(text: &str) -> Result<Document<'_>, Diagnostic> {
    let parser = Parser {
        text,
        tokens: tokenize(text)?,
        index: 0,
    };
    parser.document()
}

fn tokenize(text: &str) -> Result<Vec<Token<'_>>, Diagnostic> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut tokens = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(error) => {
                let at = reader.error_position() as usize;
                return Err(syntax(text, at, format!("error parsing: {error}")));
            }
        };
        let end = reader.buffer_position() as usize;
        let raw = &text[start..end];
        let kind = match event {
            Event::Start(event) => {
                let name = element_name(text, start, &event);
                TokenKind::Start { name, raw }
            }
            Event::Empty(event) => {
                let name = element_name(text, start, &event);
                TokenKind::Empty { name, raw }
            }
            Event::End(_) => TokenKind::End,
            Event::Text(_) => TokenKind::Text(raw),
            Event::CData(_) => TokenKind::CData,
            Event::GeneralRef(_) => TokenKind::GeneralRef,
            Event::Comment(_) => TokenKind::Comment(raw),
            Event::PI(_) => TokenKind::Pi(raw),
            Event::Decl(_) => TokenKind::Decl(raw),
            Event::DocType(_) => TokenKind::DocType(raw),
            Event::Eof => break,
        };
        tokens.push(Token { kind, start });
    }
    Ok(tokens)
}

fn element_name<'a>(
    text: &'a str,
    start: usize,
    event: &quick_xml::events::BytesStart<'_>,
) -> &'a str {
    let length = event.name().as_ref().len();
    &text[start + 1..start + 1 + length]
}

struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token<'a>>,
    index: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<Token<'a>> {
        self.tokens.get(self.index).copied()
    }

    fn next(&mut self) -> Option<Token<'a>> {
        let token = self.peek()?;
        self.index += 1;
        Some(token)
    }

    fn document(mut self) -> Result<Document<'a>, Diagnostic> {
        let mut prolog = Vec::new();
        let mut pending = 0;
        loop {
            let Some(token) = self.peek() else {
                return Err(self.syntax(self.text.len(), "expected <plist>"));
            };
            match token.kind {
                TokenKind::Text(raw) if whitespace(raw) => {
                    pending = pending.max(blank_lines(raw));
                    self.index += 1;
                }
                TokenKind::Comment(_)
                | TokenKind::Pi(_)
                | TokenKind::Decl(_)
                | TokenKind::DocType(_) => {
                    prolog.push(node(self.next().expect("peeked"), pending));
                    pending = 0;
                }
                _ => break,
            }
        }
        let token = self
            .peek()
            .ok_or_else(|| self.syntax(self.text.len(), "expected <plist>"))?;
        let name = match token.kind {
            TokenKind::Start { name, .. } | TokenKind::Empty { name, .. } => name,
            _ => return Err(self.syntax(token.start, "expected <plist>")),
        };
        if name != "plist" {
            return Err(self.syntax(
                token.start,
                format!("root element must be <plist>, not <{name}>"),
            ));
        }
        let root = self.element(1)?;
        if matches!(root.content, Content::Empty) {
            return Err(self.syntax(root.start, "plist must contain exactly one value"));
        }

        let mut epilog = Vec::new();
        let mut pending = 0;
        while let Some(token) = self.peek() {
            match token.kind {
                TokenKind::Text(raw) if whitespace(raw) => {
                    pending = pending.max(blank_lines(raw));
                    self.index += 1;
                }
                TokenKind::Comment(_) | TokenKind::Pi(_) => {
                    epilog.push(node(self.next().expect("peeked"), pending));
                    pending = 0;
                }
                _ => {
                    return Err(self.syntax(token.start, "unexpected content after </plist>"));
                }
            }
        }
        Ok(Document {
            prolog,
            root,
            epilog,
        })
    }

    fn element(&mut self, depth: usize) -> Result<Element<'a>, Diagnostic> {
        let token = self
            .next()
            .ok_or_else(|| self.syntax(self.text.len(), "expected an element"))?;
        if depth > MAX_DEPTH {
            return Err(self.syntax(
                token.start,
                format!("unsupported nesting deeper than {MAX_DEPTH} levels"),
            ));
        }
        match token.kind {
            TokenKind::Start { name, raw } => {
                let attributes = attributes(raw, name, false);
                let content = match name {
                    "plist" | "dict" | "array" => {
                        Content::Container(self.container(name, depth, token.start)?)
                    }
                    "string" | "key" | "integer" | "real" | "date" => self.text_content(name)?,
                    "data" => self.data_content(token.start)?,
                    "true" | "false" => self.empty_content(name)?,
                    other => {
                        return Err(self.syntax(token.start, format!("unknown element <{other}>")));
                    }
                };
                Ok(Element {
                    name,
                    attributes,
                    self_closed: false,
                    start: token.start,
                    content,
                })
            }
            TokenKind::Empty { name, raw } => {
                let attributes = attributes(raw, name, true);
                match name {
                    "plist" | "dict" | "array" | "string" | "key" | "integer" | "real" | "date"
                    | "data" | "true" | "false" => {}
                    other => {
                        return Err(self.syntax(token.start, format!("unknown element <{other}>")));
                    }
                }
                Ok(Element {
                    name,
                    attributes,
                    self_closed: true,
                    start: token.start,
                    content: Content::Empty,
                })
            }
            _ => Err(self.syntax(token.start, "expected an element")),
        }
    }

    fn container(
        &mut self,
        name: &'a str,
        depth: usize,
        start: usize,
    ) -> Result<Vec<Node<'a>>, Diagnostic> {
        let mut children = Vec::new();
        let mut pending = 0;
        loop {
            let Some(token) = self.peek() else {
                return Err(self.syntax(self.text.len(), format!("unclosed <{name}>")));
            };
            match token.kind {
                TokenKind::End => {
                    self.index += 1;
                    break;
                }
                TokenKind::Text(raw) if whitespace(raw) => {
                    pending = pending.max(blank_lines(raw));
                    self.index += 1;
                }
                TokenKind::Start { .. } | TokenKind::Empty { .. } => {
                    let element = self.element(depth + 1)?;
                    children.push(Node {
                        kind: NodeKind::Element(element),
                        blank_before: pending,
                    });
                    pending = 0;
                }
                TokenKind::Text(_) | TokenKind::CData | TokenKind::GeneralRef => {
                    return Err(
                        self.syntax(token.start, format!("text is not allowed inside <{name}>"))
                    );
                }
                TokenKind::Comment(_) | TokenKind::Pi(_) => {
                    children.push(node(self.next().expect("peeked"), pending));
                    pending = 0;
                }
                TokenKind::Decl(_) | TokenKind::DocType(_) => {
                    return Err(
                        self.syntax(token.start, format!("unexpected markup inside <{name}>"))
                    );
                }
            }
        }
        validate(self.text, name, start, &children)?;
        Ok(children)
    }

    fn text_content(&mut self, name: &str) -> Result<Content<'a>, Diagnostic> {
        let content_start = self.peek().map_or(self.text.len(), |token| token.start);
        loop {
            let Some(token) = self.next() else {
                return Err(self.syntax(self.text.len(), format!("unclosed <{name}>")));
            };
            match token.kind {
                TokenKind::End => {
                    let raw = &self.text[content_start..token.start];
                    return Ok(if raw.is_empty() {
                        Content::Empty
                    } else {
                        Content::Text { raw }
                    });
                }
                TokenKind::Start { name: child, .. } | TokenKind::Empty { name: child, .. } => {
                    return Err(
                        self.syntax(token.start, format!("unexpected <{child}> inside <{name}>"))
                    );
                }
                TokenKind::Decl(_) | TokenKind::DocType(_) => {
                    return Err(
                        self.syntax(token.start, format!("unexpected markup inside <{name}>"))
                    );
                }
                _ => {}
            }
        }
    }

    fn data_content(&mut self, start: usize) -> Result<Content<'a>, Diagnostic> {
        match self.text_content("data")? {
            Content::Empty => Ok(Content::Empty),
            Content::Text { raw } => match decode_data(raw) {
                Ok(decoded) => Ok(Content::Data { raw, decoded }),
                Err(error) => Err(self.syntax(start, format!("invalid base64 in <data>: {error}"))),
            },
            Content::Container(_) | Content::Data { .. } => unreachable!(),
        }
    }

    fn empty_content(&mut self, name: &str) -> Result<Content<'a>, Diagnostic> {
        let content_start = self.peek().map_or(self.text.len(), |token| token.start);
        let Some(token) = self.next() else {
            return Err(self.syntax(self.text.len(), format!("unclosed <{name}>")));
        };
        match token.kind {
            TokenKind::End if token.start == content_start => Ok(Content::Empty),
            _ => Err(self.syntax(token.start, format!("unexpected content inside <{name}>"))),
        }
    }

    fn syntax(&self, offset: usize, message: impl Into<String>) -> Diagnostic {
        syntax(self.text, offset, message)
    }
}

fn node<'a>(token: Token<'a>, blank_before: usize) -> Node<'a> {
    let kind = match token.kind {
        TokenKind::Comment(raw) => NodeKind::Comment { raw },
        TokenKind::Pi(raw) => NodeKind::Pi { raw },
        TokenKind::Decl(raw) => NodeKind::Decl { raw },
        TokenKind::DocType(raw) => NodeKind::DocType { raw },
        _ => unreachable!("only prolog and epilog markup is wrapped in a node"),
    };
    Node { kind, blank_before }
}

fn attributes<'a>(raw: &'a str, name: &str, self_closed: bool) -> &'a str {
    let end = raw.len() - if self_closed { 2 } else { 1 };
    &raw[1 + name.len()..end]
}

fn validate(text: &str, name: &str, start: usize, children: &[Node<'_>]) -> Result<(), Diagnostic> {
    let elements: Vec<&Element<'_>> = children.iter().filter_map(Node::element).collect();
    match name {
        "dict" => {
            let mut index = 0;
            while index < elements.len() {
                let key = elements[index];
                if key.name != "key" {
                    return Err(syntax(
                        text,
                        key.start,
                        format!("expected <key>, found <{}>", key.name),
                    ));
                }
                let Some(value) = elements.get(index + 1) else {
                    return Err(syntax(text, key.start, "missing value for <key>"));
                };
                if value.name == "key" {
                    return Err(syntax(
                        text,
                        value.start,
                        "<key> must be followed by a value",
                    ));
                }
                if value.name == "plist" {
                    return Err(syntax(text, value.start, "nested <plist> is not allowed"));
                }
                index += 2;
            }
        }
        "array" => {
            for element in elements {
                match element.name {
                    "key" => {
                        return Err(syntax(
                            text,
                            element.start,
                            "<key> is only allowed inside <dict>",
                        ));
                    }
                    "plist" => {
                        return Err(syntax(text, element.start, "nested <plist> is not allowed"));
                    }
                    _ => {}
                }
            }
        }
        "plist" => {
            let Some(value) = elements.first() else {
                return Err(syntax(text, start, "plist must contain exactly one value"));
            };
            if elements.len() > 1 {
                return Err(syntax(
                    text,
                    elements[1].start,
                    "plist must contain exactly one value",
                ));
            }
            if value.name == "key" {
                return Err(syntax(
                    text,
                    value.start,
                    "<key> is only allowed inside <dict>",
                ));
            }
            if value.name == "plist" {
                return Err(syntax(text, value.start, "nested <plist> is not allowed"));
            }
        }
        _ => unreachable!("only containers are validated"),
    }
    Ok(())
}

fn syntax(text: &str, offset: usize, message: impl Into<String>) -> Diagnostic {
    let (line, column) = line_column(text.as_bytes(), offset);
    Diagnostic::new(DiagnosticKind::Syntax, message).with_location(line, Some(column))
}

fn whitespace(raw: &str) -> bool {
    raw.bytes()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
}

fn blank_lines(raw: &str) -> usize {
    let mut breaks: usize = 0;
    let mut bytes = raw.bytes().peekable();
    while let Some(byte) = bytes.next() {
        match byte {
            b'\r' => {
                if bytes.peek() == Some(&b'\n') {
                    bytes.next();
                }
                breaks += 1;
            }
            b'\n' => breaks += 1,
            _ => {}
        }
    }
    breaks.saturating_sub(1)
}
