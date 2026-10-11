use saphyr_parser::ScalarStyle;
use unicode_width::UnicodeWidthStr;

use crate::config::{Config, QuoteStyle};
use crate::source::Source;
use crate::syntax::{Comment, Document, Entry, Item, Kind, Lines, Node, Stream, empty_line};

#[derive(Clone, Copy)]
struct Level {
    source: usize,
    output: usize,
}

#[derive(Clone, Copy)]
enum Parent {
    Root,
    Key(Level),
    Indicator(Level),
}

impl Parent {
    fn level(self) -> Level {
        match self {
            Parent::Root => Level {
                source: 0,
                output: 0,
            },
            Parent::Key(level) | Parent::Indicator(level) => level,
        }
    }
}

enum Layout {
    Flat(String),
    Lines,
    Wrapped,
    Verbatim,
}

pub(crate) fn print(source: &Source, stream: &Stream, config: &Config) -> String {
    let mut printer = Printer {
        source,
        stream,
        config,
        out: String::new(),
        column: 0,
        comment: 0,
        directive: 0,
        last_line: None,
        fresh: true,
        levels: Vec::new(),
        closed: Vec::new(),
    };
    for document in &stream.documents {
        printer.document(document);
    }
    printer.comments(usize::MAX);
    printer.out
}

struct Printer<'a> {
    source: &'a Source<'a>,
    stream: &'a Stream,
    config: &'a Config,
    out: String,
    column: usize,
    comment: usize,
    directive: usize,
    last_line: Option<usize>,
    fresh: bool,
    levels: Vec<Level>,
    closed: Vec<Level>,
}

impl<'a> Printer<'a> {
    fn document(&mut self, document: &'a Document) {
        let first = document.start.unwrap_or_else(|| document.root.first());
        while let Some(&(start, end)) = self
            .stream
            .directives
            .get(self.directive)
            .filter(|(start, _)| *start < first)
        {
            self.directive += 1;
            self.flush(start);
            self.end_line();
            self.write(self.source.text[start..end].trim_end());
            self.last_line = Some(self.source.line(start));
            self.trailing();
            self.newline();
        }
        self.levels.clear();
        match document.start {
            Some(marker) => {
                self.flush(marker);
                self.end_line();
                self.write("---");
                self.last_line = Some(self.source.line(marker));
                self.fresh = true;
                self.value(&document.root, Parent::Root, marker + 3);
            }
            None => {
                self.flush(document.root.first());
                self.value(&document.root, Parent::Root, document.root.first());
            }
        }
        self.end_line();
        if let Some(end) = document.end {
            self.flush(end);
            self.end_line();
            self.write("...");
            self.last_line = Some(self.source.line(end));
            self.trailing();
            self.newline();
        }
    }

    fn value(&mut self, node: &'a Node, parent: Parent, after: usize) {
        match &node.kind {
            Kind::Mapping { flow: false, .. } | Kind::Sequence { flow: false, .. } => {
                let separated = self.commented(after, node.first());
                let column = self.nested(node, parent);
                let compact =
                    matches!(parent, Parent::Indicator(_)) && node.props.is_empty() && !separated;
                if !compact {
                    let props = self.props(node);
                    if !props.is_empty() {
                        self.separate(parent);
                        self.write(&props);
                        self.last_line = node.props.last().map(|(at, _)| self.source.line(*at));
                    }
                    self.trailing();
                    self.end_line();
                    self.fresh = true;
                }
                self.collection(node, column);
            }
            Kind::Block { header, lines, .. } => {
                let pending = self.leading_props(node, parent, after);
                if self.commented(after, header.0) {
                    self.own_line(node, parent, header.0);
                } else {
                    self.separate(parent);
                }
                self.write_props(&pending);
                self.write(&self.source.text[header.0..header.1]);
                self.last_line = Some(self.source.line(header.0));
                self.trailing();
                self.newline();
                if let Some(lines) = lines {
                    let column = self.content_column(*header, lines, parent);
                    self.block(lines, column);
                }
            }
            Kind::Empty => {
                let props = self.props(node);
                if !props.is_empty() {
                    self.separate(parent);
                    self.write(&props);
                    self.last_line = node.props.last().map(|(at, _)| self.source.line(*at));
                }
                self.trailing();
            }
            _ => self.inline(node, parent, after),
        }
    }

    fn inline(&mut self, node: &'a Node, parent: Parent, after: usize) {
        let pending = self.leading_props(node, parent, after);
        let separated = self.commented(after, node.start);
        let prefix = if pending.is_empty() {
            0
        } else {
            pending.width() + 1
        };
        let own = self.own_column(parent);
        let mut layout = self.layout(
            node,
            if separated { own } else { self.column + 1 } + prefix,
            0,
        );
        let own_line = separated || (matches!(layout, Layout::Wrapped) && self.column > 0);
        if own_line {
            self.own_line(node, parent, node.start);
            if separated {
                layout = self.layout(node, own + prefix, 0);
            }
        } else {
            self.separate(parent);
        }
        self.write_props(&pending);
        match layout {
            Layout::Flat(text) => self.write(&text),
            Layout::Lines => self.lines(node, self.continuation(parent)),
            Layout::Wrapped => self.wrapped(node, if own_line { own } else { 0 }),
            Layout::Verbatim => self.verbatim(node, parent.level()),
        }
        self.last_line = Some(self.source.line(node.end.saturating_sub(1).max(node.start)));
        self.trailing();
    }

    fn leading_props(&mut self, node: &Node, parent: Parent, after: usize) -> String {
        let props = self.props(node);
        if props.is_empty() || self.commented(after, node.first()) {
            return props;
        }
        self.separate(parent);
        self.write(&props);
        self.last_line = node.props.last().map(|(at, _)| self.source.line(*at));
        String::new()
    }

    fn write_props(&mut self, props: &str) {
        if !props.is_empty() {
            self.separate_space();
            self.write(props);
            self.write(" ");
        }
    }

    fn own_line(&mut self, node: &Node, parent: Parent, before: usize) {
        let own = self.own_column(parent);
        self.trailing();
        self.end_line();
        self.fresh = true;
        self.levels.push(Level {
            source: self.source.column(node.first()),
            output: own,
        });
        self.flush(before);
        self.levels.pop();
        self.pad(own);
    }

    fn own_column(&self, parent: Parent) -> usize {
        match parent {
            Parent::Root => 0,
            _ => self.continuation(parent),
        }
    }

    fn continuation(&self, parent: Parent) -> usize {
        match parent {
            Parent::Root => self.config.indent,
            Parent::Key(level) => level.output + self.config.indent,
            Parent::Indicator(level) => level.output + self.config.dash_width(),
        }
    }

    fn nested(&self, node: &Node, parent: Parent) -> usize {
        match parent {
            Parent::Root => 0,
            Parent::Key(level) => match node.kind {
                Kind::Sequence { .. } if !self.config.indent_sequences => level.output,
                _ => level.output + self.config.indent,
            },
            Parent::Indicator(level) => level.output + self.config.dash_width(),
        }
    }

    fn content_column(&self, header: (usize, usize), lines: &Lines, parent: Parent) -> usize {
        let indent = lines.indent.unwrap_or(0);
        let explicit = self.source.text[header.0..header.1]
            .bytes()
            .any(|byte| byte.is_ascii_digit());
        match parent {
            Parent::Root => indent,
            Parent::Key(level) | Parent::Indicator(level) if explicit => {
                (indent + level.output).saturating_sub(level.source)
            }
            Parent::Key(level) | Parent::Indicator(level) => level.output + self.config.indent,
        }
    }

    fn collection(&mut self, node: &'a Node, column: usize) {
        match &node.kind {
            Kind::Mapping { entries, .. } => self.mapping(entries, column),
            Kind::Sequence { items, .. } => self.sequence(items, column),
            _ => {}
        }
    }

    fn mapping(&mut self, entries: &'a [Entry], column: usize) {
        let Some(first) = entries.first() else {
            return;
        };
        let level = Level {
            source: self.source.column(first.start),
            output: column,
        };
        self.levels.push(level);
        for entry in entries {
            self.flush(entry.start);
            self.closed.clear();
            self.pad(column);
            if entry.explicit || !self.simple_key(&entry.key) {
                self.write("?");
                self.last_line = Some(self.source.line(entry.start));
                self.value(&entry.key, Parent::Indicator(level), entry.start + 1);
                let empty = matches!(entry.value.kind, Kind::Empty) && entry.value.props.is_empty();
                if entry.colon.is_some() || !empty {
                    let colon = entry.colon.unwrap_or(entry.key.end);
                    self.end_line();
                    self.flush(colon);
                    self.pad(column);
                    self.write(":");
                    self.last_line = Some(self.source.line(colon));
                    self.value(&entry.value, Parent::Indicator(level), colon + 1);
                }
            } else {
                self.key(&entry.key);
                let colon = entry.colon.unwrap_or(entry.key.end);
                self.last_line = Some(self.source.line(colon));
                self.value(&entry.value, Parent::Key(level), colon + 1);
            }
            self.end_line();
            self.below(&entry.value, level, column + self.config.indent);
        }
        self.levels.pop();
        self.closed.push(level);
    }

    fn sequence(&mut self, items: &'a [Item], column: usize) {
        let Some(first) = items.first() else {
            return;
        };
        let level = Level {
            source: self.source.column(first.start),
            output: column,
        };
        self.levels.push(level);
        for item in items {
            self.flush(item.start);
            self.closed.clear();
            self.pad(column);
            self.write("-");
            self.last_line = Some(self.source.line(item.start));
            self.value(&item.node, Parent::Indicator(level), item.start + 1);
            self.end_line();
            self.below(&item.node, level, column + self.config.dash_width());
        }
        self.levels.pop();
        self.closed.push(level);
    }

    /// Comments indented under an inline value stay under its key.
    fn below(&mut self, value: &Node, level: Level, output: usize) {
        if matches!(
            value.kind,
            Kind::Empty
                | Kind::Scalar(_)
                | Kind::Alias
                | Kind::Mapping { flow: true, .. }
                | Kind::Sequence { flow: true, .. }
        ) {
            self.closed.push(Level {
                source: level.source + 1,
                output,
            });
        }
    }

    fn simple_key(&self, key: &Node) -> bool {
        match key.kind {
            Kind::Scalar(_)
            | Kind::Alias
            | Kind::Mapping { flow: true, .. }
            | Kind::Sequence { flow: true, .. } => !self.multiline(key),
            _ => false,
        }
    }

    fn key(&mut self, key: &'a Node) {
        let props = self.props(key);
        if !props.is_empty() {
            self.write(&props);
            self.write(" ");
        }
        let text = self
            .flat(key)
            .unwrap_or_else(|| self.source.text[key.start..key.end].to_string());
        self.write(&text);
        self.last_line = Some(self.source.line(key.end.saturating_sub(1).max(key.start)));
        if matches!(key.kind, Kind::Alias) {
            self.write(" ");
        }
        self.write(":");
    }

    fn separate(&mut self, parent: Parent) {
        match parent {
            Parent::Indicator(level) if !self.out.ends_with(' ') => {
                self.pad(level.output + self.config.dash_width())
            }
            _ => self.separate_space(),
        }
    }

    fn separate_space(&mut self) {
        if self.column > 0 && !self.out.ends_with(' ') {
            self.write(" ");
        }
    }

    fn props(&self, node: &Node) -> String {
        node.props
            .iter()
            .map(|(start, end)| &self.source.text[*start..*end])
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn layout(&self, node: &Node, column: usize, reserve: usize) -> Layout {
        match &node.kind {
            Kind::Scalar(_) if self.multiline(node) => Layout::Lines,
            Kind::Mapping { flow: true, .. } | Kind::Sequence { flow: true, .. } => {
                let Some(text) = self.flat(node) else {
                    return Layout::Verbatim;
                };
                let joined = !self.multiline(node) || self.config.prefer_single_line;
                let fits = column + text.width() + reserve <= self.config.width;
                if self.empty(node) || (joined && fits) {
                    Layout::Flat(text)
                } else {
                    Layout::Wrapped
                }
            }
            _ => Layout::Flat(self.flat(node).unwrap_or_default()),
        }
    }

    fn flat(&self, node: &Node) -> Option<String> {
        match &node.kind {
            Kind::Scalar(style) if !self.multiline(node) => Some(quote(
                &self.source.text[node.start..node.end],
                *style,
                self.config.quote_style,
            )),
            Kind::Alias => Some(self.source.text[node.start..node.end].to_string()),
            Kind::Sequence { flow: true, items } if !self.inner_comment(node) => {
                let items = items
                    .iter()
                    .map(|item| self.flat_child(&item.node))
                    .collect::<Option<Vec<_>>>()?;
                Some(brackets(
                    "[",
                    "]",
                    self.config.bracket_spacing,
                    &items.join(", "),
                ))
            }
            Kind::Mapping {
                flow: true,
                pair: false,
                entries,
            } if !self.inner_comment(node) => {
                let entries = entries
                    .iter()
                    .map(|entry| {
                        if entry.explicit || matches!(entry.value.kind, Kind::Empty) {
                            return None;
                        }
                        Some(format!(
                            "{}{} {}",
                            self.flat_child(&entry.key)?,
                            separator(&entry.key),
                            self.flat_child(&entry.value)?
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(brackets(
                    "{",
                    "}",
                    self.config.brace_spacing,
                    &entries.join(", "),
                ))
            }
            _ => None,
        }
    }

    fn flat_child(&self, node: &Node) -> Option<String> {
        let text = self.flat(node)?;
        Some(if node.props.is_empty() {
            text
        } else {
            format!("{} {text}", self.props(node))
        })
    }

    fn wrapped(&mut self, node: &'a Node, column: usize) {
        let inner = column + self.config.indent;
        match &node.kind {
            Kind::Sequence { items, .. } => {
                self.write("[");
                for (index, item) in items.iter().enumerate() {
                    let last = index + 1 == items.len();
                    self.newline();
                    self.pad(inner);
                    self.child(&item.node, inner, last);
                    self.comma(last);
                }
                self.newline();
                self.pad(column);
                self.write("]");
            }
            Kind::Mapping { entries, .. } => {
                self.write("{");
                for (index, entry) in entries.iter().enumerate() {
                    let last = index + 1 == entries.len();
                    self.newline();
                    self.pad(inner);
                    let key = self.flat_child(&entry.key).unwrap_or_default();
                    self.write(&key);
                    self.write(separator(&entry.key));
                    self.write(" ");
                    self.child(&entry.value, inner, last);
                    self.comma(last);
                }
                self.newline();
                self.pad(column);
                self.write("}");
            }
            _ => {}
        }
    }

    fn child(&mut self, node: &'a Node, column: usize, last: bool) {
        let props = self.props(node);
        if !props.is_empty() {
            self.write(&props);
            self.write(" ");
        }
        let reserve = usize::from(!last || self.config.trailing_comma);
        match self.layout(node, self.column, reserve) {
            Layout::Wrapped => self.wrapped(node, column),
            Layout::Flat(text) => self.write(&text),
            Layout::Lines | Layout::Verbatim => {}
        }
    }

    fn comma(&mut self, last: bool) {
        if !last || self.config.trailing_comma {
            self.write(",");
        }
    }

    fn verbatim(&mut self, node: &'a Node, level: Level) {
        let first = self.source.line(node.start);
        let last = self.source.line(node.end.saturating_sub(1).max(node.start));
        for line in first..=last {
            let from = if line == first {
                node.start
            } else {
                self.source.start(line)
            };
            let to = if line == last {
                node.end
            } else {
                self.source.end(line)
            };
            let text = &self.source.text[from..to];
            if line == first {
                self.write(text);
                continue;
            }
            self.newline();
            let content = text.trim_start_matches([' ', '\t']);
            if !content.is_empty() {
                self.pad((self.source.indent(line) + level.output).saturating_sub(level.source));
                self.write(content);
            }
        }
        while self
            .stream
            .comments
            .get(self.comment)
            .is_some_and(|comment| comment.start < node.end)
        {
            self.comment += 1;
        }
    }

    fn lines(&mut self, node: &Node, continuation: usize) {
        let raw = &self.source.text[node.start..node.end];
        for (index, line) in raw.split('\n').enumerate() {
            if index == 0 {
                self.write(line);
                continue;
            }
            self.newline();
            let content = line.trim_start_matches([' ', '\t']);
            if !content.is_empty() {
                self.pad(continuation);
                self.write(content);
            }
        }
    }

    fn block(&mut self, lines: &Lines, column: usize) {
        for line in lines.first..=lines.last {
            if let Some(indent) = lines.indent
                && !empty_line(self.source, line, Some(indent))
            {
                let text = self.source.line_text(line);
                let spaces = self.source.indent(line);
                self.pad((spaces + column).saturating_sub(indent));
                self.write(&text[spaces..]);
            }
            self.out.push('\n');
            self.column = 0;
        }
        self.last_line = Some(lines.last);
    }

    fn flush(&mut self, before: usize) {
        self.comments(before);
        self.gap(self.source.line(before.min(self.source.text.len())));
    }

    fn comments(&mut self, before: usize) {
        while let Some(comment) = self
            .stream
            .comments
            .get(self.comment)
            .filter(|comment| comment.start < before)
        {
            self.comment += 1;
            self.end_line();
            self.gap(comment.line);
            self.pad(self.comment_column(comment.column));
            self.write(&self.comment_text(comment));
            self.newline();
            self.last_line = Some(comment.line);
        }
    }

    fn trailing(&mut self) {
        let Some(line) = self.last_line else {
            return;
        };
        if self.column == 0 {
            return;
        }
        if let Some(comment) = self
            .stream
            .comments
            .get(self.comment)
            .filter(|comment| comment.line == line && !comment.own_line)
        {
            self.comment += 1;
            let text = self.comment_text(comment);
            self.separate_space();
            self.write(&text);
        }
    }

    fn commented(&self, from: usize, to: usize) -> bool {
        self.stream.comments[self.comment..]
            .iter()
            .take_while(|comment| comment.start < to)
            .any(|comment| comment.start >= from)
    }

    fn inner_comment(&self, node: &Node) -> bool {
        let comments = &self.stream.comments;
        let index = comments.partition_point(|comment| comment.start <= node.start);
        comments
            .get(index)
            .is_some_and(|comment| comment.start < node.end)
    }

    /// Ties prefer open levels, then the outermost closed level.
    fn comment_column(&self, column: usize) -> usize {
        self.closed
            .iter()
            .chain(&self.levels)
            .filter(|level| level.source <= column)
            .max_by_key(|level| level.source)
            .map_or(0, |level| level.output)
    }

    fn comment_text(&self, comment: &Comment) -> String {
        let text = self.source.text[comment.start..comment.end].trim_end();
        match text.strip_prefix('#') {
            Some(rest)
                if self.config.format_comments
                    && !(comment.line == 0 && comment.own_line)
                    && !rest.is_empty()
                    && !rest.starts_with([' ', '\t', '#', '!']) =>
            {
                format!("# {rest}")
            }
            _ => text.to_string(),
        }
    }

    fn gap(&mut self, line: usize) {
        let blank = !self.fresh
            && self
                .last_line
                .is_some_and(|last| (last + 1..line).any(|line| self.source.blank(line)));
        self.fresh = false;
        if blank {
            self.end_line();
            self.out.push('\n');
        }
    }

    fn multiline(&self, node: &Node) -> bool {
        self.source.text[node.start..node.end].contains('\n')
    }

    fn empty(&self, node: &Node) -> bool {
        match &node.kind {
            Kind::Sequence { items, .. } => items.is_empty(),
            Kind::Mapping { entries, .. } => entries.is_empty(),
            _ => false,
        }
    }

    fn write(&mut self, text: &str) {
        self.out.push_str(text);
        self.column += text.width();
    }

    fn pad(&mut self, column: usize) {
        if self.column < column {
            self.write(&" ".repeat(column - self.column));
        } else if self.column > 0 && !self.out.ends_with(' ') {
            self.write(" ");
        }
    }

    fn newline(&mut self) {
        let trimmed = self.out.trim_end_matches([' ', '\t']);
        if !trimmed.ends_with('\\') {
            self.out.truncate(trimmed.len());
        }
        self.out.push('\n');
        self.column = 0;
    }

    fn end_line(&mut self) {
        if self.column > 0 {
            self.newline();
        }
    }
}

fn separator(key: &Node) -> &'static str {
    if matches!(key.kind, Kind::Alias) {
        " :"
    } else {
        ":"
    }
}

fn brackets(open: &str, close: &str, spacing: bool, inner: &str) -> String {
    if inner.is_empty() {
        format!("{open}{close}")
    } else if spacing {
        format!("{open} {inner} {close}")
    } else {
        format!("{open}{inner}{close}")
    }
}

/// Picks the quotes needing fewer escapes, or the forced ones; other escapes keep double quotes.
fn quote(raw: &str, style: ScalarStyle, preference: QuoteStyle) -> String {
    let content = match style {
        ScalarStyle::SingleQuoted => raw[1..raw.len() - 1].replace("''", "'"),
        ScalarStyle::DoubleQuoted => match unescape(&raw[1..raw.len() - 1]) {
            Some(content) => content,
            None => return raw.to_string(),
        },
        _ => return raw.to_string(),
    };
    let single_cost = content.matches('\'').count();
    let double_cost = content.matches(['"', '\\']).count();
    let double = match preference {
        QuoteStyle::Double => true,
        QuoteStyle::Single => false,
        QuoteStyle::PreferDouble => double_cost <= single_cost,
        QuoteStyle::PreferSingle => double_cost < single_cost,
    };
    if double {
        format!("\"{}\"", content.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{}'", content.replace('\'', "''"))
    }
}

fn unescape(inner: &str) -> Option<String> {
    let mut content = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next()? {
                escaped @ ('"' | '\\' | '/') => content.push(escaped),
                _ => return None,
            }
        } else {
            content.push(character);
        }
    }
    Some(content)
}
