use std::borrow::Cow;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use saphyr_parser::{Event, Parser, ScalarStyle, Tag};

use crate::source::Source;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    DocumentStart(bool),
    DocumentEnd,
    Alias(usize),
    Scalar(String, Class, usize, Option<Tag>),
    SequenceStart(usize, Option<Tag>),
    SequenceEnd,
    MappingStart(usize, Option<Tag>),
    MappingEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Plain,
    Quoted,
    Literal,
    Folded,
}

pub(crate) struct Read {
    pub shape: Shape,
    pub style: ScalarStyle,
    pub start: usize,
    pub end: usize,
}

pub(crate) fn read(text: &str) -> Result<Vec<Read>, Diagnostic> {
    let mut parser = Parser::new_from_str(text);
    let mut reads = Vec::new();
    while let Some(next) = parser.next_event() {
        let (event, span) = next.map_err(|error| {
            let at = error.marker();
            Diagnostic::new(DiagnosticKind::Syntax, error.info())
                .with_location(at.line(), Some(at.col() + 1))
        })?;
        let mut style = ScalarStyle::Plain;
        let shape = match event {
            Event::DocumentStart(explicit) => Shape::DocumentStart(explicit),
            Event::DocumentEnd => Shape::DocumentEnd,
            Event::Alias(id) => Shape::Alias(id),
            Event::Scalar(value, scalar, anchor, tag) => {
                style = scalar;
                let class = match scalar {
                    ScalarStyle::Plain => Class::Plain,
                    ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted => Class::Quoted,
                    ScalarStyle::Literal => Class::Literal,
                    ScalarStyle::Folded => Class::Folded,
                };
                Shape::Scalar(value.into_owned(), class, anchor, tag.map(Cow::into_owned))
            }
            Event::SequenceStart(anchor, tag) => {
                Shape::SequenceStart(anchor, tag.map(Cow::into_owned))
            }
            Event::SequenceEnd => Shape::SequenceEnd,
            Event::MappingStart(anchor, tag) => {
                Shape::MappingStart(anchor, tag.map(Cow::into_owned))
            }
            Event::MappingEnd => Shape::MappingEnd,
            Event::Nothing | Event::StreamStart | Event::StreamEnd => continue,
        };
        reads.push(Read {
            shape,
            style,
            start: span.start.index(),
            end: span.end.index(),
        });
    }
    Ok(reads)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Dash,
    Question,
    Colon,
    Close,
    Other,
}

pub(crate) struct Comment {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
    pub own_line: bool,
}

pub(crate) struct Lines {
    pub first: usize,
    pub last: usize,
    pub indent: Option<usize>,
}

pub(crate) enum Kind {
    Empty,
    Scalar(ScalarStyle),
    Block {
        header: (usize, usize),
        lines: Option<Lines>,
        strip: bool,
    },
    Alias,
    Sequence {
        flow: bool,
        items: Vec<Item>,
    },
    Mapping {
        flow: bool,
        pair: bool,
        entries: Vec<Entry>,
    },
}

pub(crate) struct Node {
    pub start: usize,
    pub end: usize,
    pub props: Vec<(usize, usize)>,
    pub kind: Kind,
}

impl Node {
    pub fn first(&self) -> usize {
        self.props
            .first()
            .map_or(self.start, |(start, _)| (*start).min(self.start))
    }

    pub fn last(&self) -> &Node {
        match &self.kind {
            Kind::Sequence { flow: false, items } => {
                items.last().map_or(self, |item| item.node.last())
            }
            Kind::Mapping {
                flow: false,
                entries,
                ..
            } => entries.last().map_or(self, |entry| entry.value.last()),
            _ => self,
        }
    }
}

pub(crate) struct Item {
    pub start: usize,
    pub node: Node,
}

pub(crate) struct Entry {
    pub start: usize,
    pub explicit: bool,
    pub colon: Option<usize>,
    pub key: Node,
    pub value: Node,
}

pub(crate) struct Document {
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub root: Node,
}

pub(crate) struct Stream {
    pub documents: Vec<Document>,
    pub comments: Vec<Comment>,
    pub directives: Vec<(usize, usize)>,
}

impl Stream {
    pub fn ends_in_block_scalar(&self) -> bool {
        let Some(document) = self.documents.last() else {
            return false;
        };
        let last = document.root.last();
        document.end.is_none()
            && matches!(last.kind, Kind::Block { strip: false, .. })
            && self
                .comments
                .last()
                .is_none_or(|comment| comment.start < last.end)
    }
}

pub(crate) fn parse(source: &Source, reads: &[Read]) -> Result<Stream, Diagnostic> {
    let spans: Vec<(usize, usize)> = reads
        .iter()
        .map(|read| {
            let (start, end) = (source.byte(read.start), source.byte(read.end));
            match read.style {
                // Quoted scalar spans can extend past their closing quote.
                ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted
                    if matches!(read.shape, Shape::Scalar(..)) =>
                {
                    (start, quoted_end(source.text, start).unwrap_or(end))
                }
                _ if matches!(read.shape, Shape::Scalar(..) | Shape::Alias(_)) => {
                    (start, start + source.text[start..end].trim_end().len())
                }
                _ => (start, end),
            }
        })
        .collect();
    let mut covered = vec![false; source.text.len()];
    let mut blocks = vec![None; reads.len()];
    for (index, read) in reads.iter().enumerate() {
        let (start, end) = spans[index];
        match &read.shape {
            Shape::Scalar(value, Class::Literal | Class::Folded, ..) => {
                if value.contains(|character| character != '\n') {
                    let first = source.line(start);
                    let indent = source.column(start);
                    let last = content_end(source, first, indent);
                    covered[source.start(first)..source.end(last)].fill(true);
                    blocks[index] = Some((last, indent));
                }
            }
            Shape::Scalar(value, class, ..) if !value.is_empty() || *class != Class::Plain => {
                covered[start..end].fill(true)
            }
            Shape::Alias(_) => covered[start..end].fill(true),
            _ => {}
        }
    }
    let tokens = scan(source, &covered)?;
    let mut builder = Builder {
        source,
        reads,
        spans,
        blocks,
        covered,
        tokens,
        at: 0,
        depth: 0,
        property: 0,
        header: 0,
    };
    let documents = builder.documents()?;
    Ok(Stream {
        documents,
        comments: builder.tokens.comments,
        directives: builder.tokens.directives,
    })
}

fn quoted_end(text: &str, start: usize) -> Option<usize> {
    let quote = text[start..].chars().next()?;
    let mut characters = text[start + 1..].char_indices().peekable();
    while let Some((at, character)) = characters.next() {
        match character {
            '\\' if quote == '"' => {
                characters.next();
            }
            '\'' if quote == '\'' && characters.peek().is_some_and(|(_, next)| *next == '\'') => {
                characters.next();
            }
            found if found == quote => return Some(start + 1 + at + 1),
            _ => {}
        }
    }
    None
}

fn content_end(source: &Source, first: usize, indent: usize) -> usize {
    let mut last = first;
    for line in first + 1..source.lines() {
        if empty_line(source, line, Some(indent)) {
            continue;
        }
        if source.indent(line) < indent || marker(source.line_text(line)) {
            break;
        }
        last = line;
    }
    last
}

pub(crate) fn empty_line(source: &Source, line: usize, indent: Option<usize>) -> bool {
    let text = source.line_text(line);
    text.bytes().all(|byte| byte == b' ') && indent.is_none_or(|indent| text.len() <= indent)
}

struct Tokens {
    marks: Vec<(usize, Mark)>,
    properties: Vec<(usize, usize)>,
    headers: Vec<(usize, usize)>,
    comments: Vec<Comment>,
    directives: Vec<(usize, usize)>,
}

fn scan(source: &Source, covered: &[bool]) -> Result<Tokens, Diagnostic> {
    let text = source.text;
    let bytes = text.as_bytes();
    let mut tokens = Tokens {
        marks: Vec::new(),
        properties: Vec::new(),
        headers: Vec::new(),
        comments: Vec::new(),
        directives: Vec::new(),
    };
    let mut at = 0;
    let (mut line, mut line_start, mut line_end) = (0, 0, source.end(0));
    while at < bytes.len() {
        if covered[at] {
            at += 1;
            continue;
        }
        if at > line_end {
            line = source.line(at);
            line_start = source.start(line);
            line_end = source.end(line);
        }
        let separated = at == line_start || matches!(bytes[at - 1], b' ' | b'\t');
        let token_end = |stop: &[u8]| {
            bytes[at..line_end]
                .iter()
                .position(|byte| matches!(byte, b' ' | b'\t') || stop.contains(byte))
                .map_or(line_end, |length| at + length)
        };
        match bytes[at] {
            b' ' | b'\t' | b'\n' => at += 1,
            b'#' if separated => {
                tokens.comments.push(Comment {
                    start: at,
                    end: line_end,
                    line,
                    column: source.column(at),
                    own_line: text[line_start..at].trim().is_empty(),
                });
                at = line_end;
            }
            b'%' if at == line_start => {
                let end = text[at..line_end]
                    .find(" #")
                    .map_or(line_end, |length| at + length);
                tokens.directives.push((at, end));
                at = end;
            }
            b'-' | b'.' if at == line_start && marker(&text[at..line_end]) => {
                tokens.marks.push((at, Mark::Other));
                at += 3;
            }
            b'-' => {
                tokens.marks.push((at, Mark::Dash));
                at += 1;
            }
            b'?' => {
                tokens.marks.push((at, Mark::Question));
                at += 1;
            }
            b':' => {
                tokens.marks.push((at, Mark::Colon));
                at += 1;
            }
            b']' | b'}' => {
                tokens.marks.push((at, Mark::Close));
                at += 1;
            }
            b',' | b'[' | b'{' => {
                tokens.marks.push((at, Mark::Other));
                at += 1;
            }
            b'!' if text[at..].starts_with("!<") => {
                let end = text[at..line_end]
                    .find('>')
                    .map_or(line_end, |length| at + length + 1);
                tokens.properties.push((at, end));
                at = end;
            }
            b'&' | b'!' => {
                let end = token_end(b",[]{}");
                tokens.properties.push((at, end));
                at = end;
            }
            b'|' | b'>' => {
                let end = token_end(b"");
                tokens.headers.push((at, end));
                at = end;
            }
            _ => {
                return Err(
                    Diagnostic::new(DiagnosticKind::Internal, "unsupported YAML syntax")
                        .with_location(line + 1, Some(source.column(at) + 1)),
                );
            }
        }
    }
    Ok(tokens)
}

fn marker(line: &str) -> bool {
    (line.starts_with("---") || line.starts_with("..."))
        && line[3..]
            .chars()
            .next()
            .is_none_or(|next| next == ' ' || next == '\t')
}

struct Builder<'s, 'r> {
    source: &'s Source<'s>,
    reads: &'r [Read],
    spans: Vec<(usize, usize)>,
    blocks: Vec<Option<(usize, usize)>>,
    covered: Vec<bool>,
    tokens: Tokens,
    at: usize,
    depth: usize,
    property: usize,
    header: usize,
}

impl Builder<'_, '_> {
    fn documents(&mut self) -> Result<Vec<Document>, Diagnostic> {
        let mut documents = Vec::new();
        while let Some(read) = self.reads.get(self.at) {
            let Shape::DocumentStart(explicit) = read.shape else {
                return Err(unsupported());
            };
            let (start, _) = self.spans[self.at];
            self.at += 1;
            let root = self.node(false, false)?;
            let (end, _) = self.spans.get(self.at).copied().ok_or_else(unsupported)?;
            self.at += 1;
            documents.push(Document {
                start: explicit.then_some(start),
                end: self.source.text[end..].starts_with("...").then_some(end),
                root,
            });
        }
        Ok(documents)
    }

    fn node(&mut self, flow: bool, key: bool) -> Result<Node, Diagnostic> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            let (start, _) = self.spans.get(self.at).copied().unwrap_or_default();
            let line = self.source.line(start);
            return Err(Diagnostic::new(
                DiagnosticKind::Syntax,
                format!("nesting deeper than {MAX_DEPTH} levels"),
            )
            .with_location(line + 1, Some(self.source.column(start) + 1)));
        }
        let node = self.build(flow, key);
        self.depth -= 1;
        node
    }

    fn build(&mut self, flow: bool, key: bool) -> Result<Node, Diagnostic> {
        let index = self.at;
        let read = self.reads.get(index).ok_or_else(unsupported)?;
        let (start, end) = self.spans[index];
        self.at += 1;
        let (anchor, tag) = match &read.shape {
            Shape::Scalar(_, _, anchor, tag)
            | Shape::SequenceStart(anchor, tag)
            | Shape::MappingStart(anchor, tag) => (*anchor != 0, tag.is_some()),
            _ => (false, false),
        };
        let props = self.props(usize::from(anchor) + usize::from(tag))?;
        let make = |start, end, kind| Node {
            start,
            end,
            props,
            kind,
        };
        Ok(match &read.shape {
            Shape::Scalar(value, Class::Plain, ..) if value.is_empty() => {
                make(start, start, Kind::Empty)
            }
            Shape::Scalar(_, Class::Literal | Class::Folded, ..) => {
                let header = *self
                    .tokens
                    .headers
                    .get(self.header)
                    .ok_or_else(unsupported)?;
                self.header += 1;
                let header_line = self.source.line(header.0);
                let (mut last, indent) = match self.blocks[index] {
                    Some((last, indent)) => (last, Some(indent)),
                    None => (header_line, None),
                };
                let keep = self.source.text[header.0..header.1].contains('+');
                while keep
                    && last + 1 < self.source.lines()
                    && empty_line(self.source, last + 1, indent)
                {
                    last += 1;
                }
                let lines = (last > header_line).then_some(Lines {
                    first: header_line + 1,
                    last,
                    indent,
                });
                let end = lines
                    .as_ref()
                    .map_or(header.1, |lines| self.source.end(lines.last));
                let strip = self.source.text[header.0..header.1].contains('-');
                make(
                    header.0,
                    end,
                    Kind::Block {
                        header,
                        lines,
                        strip,
                    },
                )
            }
            Shape::Scalar(..) => make(start, end, Kind::Scalar(read.style)),
            Shape::Alias(_) => make(start, end, Kind::Alias),
            Shape::SequenceStart(..) => {
                let flow = flow || self.opens(index, start, '[');
                let mut items = Vec::new();
                while !matches!(self.current()?, Shape::SequenceEnd) {
                    let node = self.node(flow, false)?;
                    let first = node.first();
                    let start = if flow {
                        first
                    } else {
                        self.last_mark(Mark::Dash, first).unwrap_or(first)
                    };
                    items.push(Item { start, node });
                }
                self.at += 1;
                let (start, end) = if flow {
                    (
                        start,
                        self.close(start, items.last().map(|item| &item.node)),
                    )
                } else {
                    (
                        items.first().map_or(start, |item| item.start),
                        items.last().map_or(start, |item| item.node.end),
                    )
                };
                make(start, end, Kind::Sequence { flow, items })
            }
            Shape::MappingStart(..) => {
                let brace = self.opens(index, start, '{');
                let pair = flow && !brace;
                let flow = flow || brace;
                let mut entries = Vec::new();
                while !matches!(self.current()?, Shape::MappingEnd) {
                    let key_node = self.node(flow, true)?;
                    let value = self.node(flow, false)?;
                    let first = key_node.first();
                    // A block key's own entries cannot claim the `?` that introduces it.
                    let question = (flow || !key || !entries.is_empty())
                        .then(|| self.previous_mark(first))
                        .flatten()
                        .filter(|(at, mark)| {
                            *mark == Mark::Question && !self.covered[*at..first].contains(&true)
                        })
                        .map(|(at, _)| at);
                    let explicit = question.is_some() || matches!(key_node.kind, Kind::Empty);
                    let limit = if matches!(value.kind, Kind::Empty) {
                        self.spans
                            .get(self.at)
                            .map_or(self.source.text.len(), |(start, _)| *start)
                    } else {
                        value.first()
                    };
                    let colon = self.first_mark(Mark::Colon, key_node.end, limit);
                    entries.push(Entry {
                        start: question.unwrap_or(first).min(first),
                        explicit,
                        colon,
                        key: key_node,
                        value,
                    });
                }
                self.at += 1;
                let (start, end) = if brace {
                    (
                        start,
                        self.close(start, entries.last().map(|entry| &entry.value)),
                    )
                } else {
                    (
                        entries.first().map_or(start, |entry| entry.start),
                        entries
                            .last()
                            .map_or(start, |entry| match entry.value.kind {
                                Kind::Empty => entry.colon.map_or(entry.key.end, |colon| colon + 1),
                                _ => entry.value.end,
                            }),
                    )
                };
                make(
                    start,
                    end,
                    Kind::Mapping {
                        flow,
                        pair,
                        entries,
                    },
                )
            }
            _ => return Err(unsupported()),
        })
    }

    fn close(&self, start: usize, last: Option<&Node>) -> usize {
        let from = last.map_or(start + 1, |node| node.end.max(start + 1));
        self.first_mark(Mark::Close, from, usize::MAX)
            .map_or(from, |at| at + 1)
    }

    /// A block collection whose first entry is a flow collection starts at the same position.
    fn opens(&self, index: usize, start: usize, bracket: char) -> bool {
        let shared = self.reads.get(index + 1).is_some_and(|next| {
            matches!(
                next.shape,
                Shape::SequenceStart(..) | Shape::MappingStart(..)
            ) && self.spans[index + 1].0 == start
        });
        self.source.text[start..].starts_with(bracket) && !shared
    }

    fn current(&self) -> Result<&Shape, Diagnostic> {
        self.reads
            .get(self.at)
            .map(|read| &read.shape)
            .ok_or_else(unsupported)
    }

    fn props(&mut self, count: usize) -> Result<Vec<(usize, usize)>, Diagnostic> {
        let props = self
            .tokens
            .properties
            .get(self.property..self.property + count)
            .ok_or_else(unsupported)?
            .to_vec();
        self.property += count;
        Ok(props)
    }

    fn previous_mark(&self, before: usize) -> Option<(usize, Mark)> {
        let index = self.tokens.marks.partition_point(|(at, _)| *at < before);
        index.checked_sub(1).map(|index| self.tokens.marks[index])
    }

    fn last_mark(&self, mark: Mark, before: usize) -> Option<usize> {
        let index = self.tokens.marks.partition_point(|(at, _)| *at < before);
        self.tokens.marks[..index]
            .iter()
            .rev()
            .find(|(_, found)| *found == mark)
            .map(|(at, _)| *at)
    }

    fn first_mark(&self, mark: Mark, from: usize, before: usize) -> Option<usize> {
        let index = self.tokens.marks.partition_point(|(at, _)| *at < from);
        self.tokens.marks[index..]
            .iter()
            .take_while(|(at, _)| *at < before)
            .find(|(_, found)| *found == mark)
            .map(|(at, _)| *at)
    }
}

const MAX_DEPTH: usize = 256;

fn unsupported() -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, "unsupported YAML syntax")
}
