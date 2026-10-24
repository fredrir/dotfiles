use std::borrow::Cow;
use std::collections::HashMap;

use crate::config::Config;
use crate::model::{Array, Document, Entry, InlineTable, Section, Value};
use crate::trivia::{Trivia, format_comment};

pub(crate) fn print(document: &Document<'_>, config: &Config) -> String {
    let mut printer = Printer {
        config,
        output: String::new(),
        rows: Vec::new(),
        level: 0,
        headers: HashMap::new(),
    };
    printer.entries(&document.entries);
    printer.sections(&document.sections);
    printer.flush();
    printer.lead(&document.trailing, printer.level);
    printer.output
}

/// One entry while it waits for the alignment of its group.
struct Row<'a> {
    lead: &'a Trivia<'a>,
    path: &'a [&'a str],
    decoded: &'a [&'a str],
    value: &'a Value<'a>,
    trailing: Option<&'a str>,
    level: usize,
}

/// A rendered element with the comment that follows it.
struct Rendered {
    body: String,
    comment: Option<String>,
}

struct Printer<'a> {
    config: &'a Config,
    output: String,
    rows: Vec<Row<'a>>,
    /// Indentation level used for entries.
    level: usize,
    /// Level of every printed header, by decoded path, so a subtable indents
    /// under a parent even when other sections were printed in between.
    headers: HashMap<Vec<&'a str>, usize>,
}

impl<'a> Printer<'a> {
    fn sections(&mut self, sections: &'a [Section<'a>]) {
        for section in sections {
            if section.visible {
                self.flush();
                let level = self.section_level(section);
                // `indent_entries` indents a header's contents, not the header.
                let indent = if self.config.indent_tables {
                    level.saturating_sub(usize::from(self.config.indent_entries))
                } else {
                    0
                };
                self.lead(&section.lead, indent);
                let mut line = self.pad(indent);
                line.push_str(if section.array { "[[" } else { "[" });
                match section.header {
                    Some(header) => push_header(&mut line, header),
                    None => line.push_str(&section.path.join(".")),
                }
                line.push_str(if section.array { "]]" } else { "]" });
                if let Some(comment) = section.trailing {
                    line.push(' ');
                    line.push_str(&self.comment(comment));
                }
                self.push_line(&line);
            }
            self.entries(&section.entries);
            self.flush();
        }
    }

    fn section_level(&mut self, section: &'a Section<'a>) -> usize {
        if self.config.indent_entries && self.level == 0 {
            self.level = 1;
        }
        if self.config.indent_tables {
            let level = (1..section.decoded.len())
                .rev()
                .find_map(|length| self.headers.get(&section.decoded[..length]))
                .map_or(usize::from(self.config.indent_entries), |parent| parent + 1);
            self.level = level;
            self.headers.insert(section.decoded.clone(), level);
        }
        self.level
    }

    fn entries(&mut self, entries: &'a [Entry<'a>]) {
        let level = self.level;
        self.rows.extend(entries.iter().map(|entry| Row {
            lead: &entry.lead,
            path: &entry.path,
            decoded: &entry.decoded,
            value: &entry.value,
            trailing: entry.trailing,
            level,
        }));
    }

    /// Writes the buffered entries, aligning each contiguous group.
    fn flush(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let rows = std::mem::take(&mut self.rows);
        let mut group: Vec<&Row<'a>> = Vec::new();
        for row in &rows {
            if self.separates(row.lead) {
                self.group(&mut group);
                self.lead(row.lead, row.level);
            }
            group.push(row);
        }
        self.group(&mut group);
    }

    /// Whether this trivia will emit a blank line or a comment after the
    /// configured blank-line limit is applied. Alignment groups must follow
    /// the emitted layout, not the source layout, or collapsing a blank line
    /// would merge two groups that were already aligned separately.
    fn separates(&self, trivia: &Trivia<'_>) -> bool {
        trivia.has_comments() || trivia.blank.min(self.config.allowed_blank_lines) > 0
    }

    fn group<'r>(&mut self, group: &mut Vec<&'r Row<'a>>) {
        if group.is_empty() {
            return;
        }
        if self.config.reorder_keys {
            group.sort_by(|left, right| left.decoded.cmp(right.decoded));
        }
        let separator = if self.config.compact_entries { 1 } else { 3 };
        let mut rendered = Vec::with_capacity(group.len());
        for row in group.iter() {
            let key = row.path.join(".");
            let column = self.indent_width(row.level) + width(&key) + separator;
            let reserve = row
                .trailing
                .map_or(0, |comment| width(&self.comment(comment)) + 1);
            let mut value = self.value(row.value, row.level, column, false, true);
            if self.expandable(row.value) && !fits(column, &value, reserve, self.config.width) {
                value = self.value(row.value, row.level, column, true, true);
            }
            rendered.push((key, value));
        }
        // A row spanning lines has no single column to align against.
        let alignable = rendered
            .iter()
            .all(|(key, value)| !key.contains('\n') && !value.contains('\n'));
        let align_keys = alignable && self.config.align_entries;
        let comments = group.iter().filter(|row| row.trailing.is_some()).count();
        let align_comments = alignable && self.aligns_comments(comments);
        let widest_key = rendered
            .iter()
            .map(|(key, _)| width(key))
            .max()
            .unwrap_or(0);
        let widest_body = if align_comments {
            group
                .iter()
                .zip(&rendered)
                .map(|(row, (key, value))| {
                    let key = if align_keys { widest_key } else { width(key) };
                    self.indent_width(row.level) + key + separator + width(value)
                })
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        for (row, (key, value)) in group.iter().zip(rendered) {
            let mut line = self.pad(row.level);
            line.push_str(&key);
            if align_keys {
                line.push_str(&" ".repeat(widest_key - width(&key)));
            }
            line.push_str(if self.config.compact_entries {
                "="
            } else {
                " = "
            });
            line.push_str(&value);
            if let Some(comment) = row.trailing {
                if align_comments {
                    line.push_str(&" ".repeat(widest_body.saturating_sub(width(&line))));
                }
                line.push(' ');
                line.push_str(&self.comment(comment));
            }
            self.push_line(&line);
        }
        group.clear();
    }

    /// Whether a value can trade line width for several lines.
    fn expandable(&self, value: &Value<'_>) -> bool {
        match value {
            Value::Array(array) => self.config.array_auto_expand && !array.items.is_empty(),
            Value::InlineTable(table) => {
                self.config.inline_table_expand && !table.entries.is_empty()
            }
            _ => false,
        }
    }

    fn aligns_comments(&self, comments: usize) -> bool {
        self.config.align_comments && (comments != 1 || self.config.align_single_comments)
    }

    fn lead(&mut self, trivia: &Trivia<'_>, level: usize) {
        if let Some(comment) = trivia.inline {
            let line = format!("{}{}", self.pad(level), self.comment(comment));
            self.push_line(&line);
        }
        for gap in &trivia.gaps {
            self.blank(gap.blank);
            let line = format!("{}{}", self.pad(level), self.comment(gap.comment));
            self.push_line(&line);
        }
        self.blank(trivia.blank);
    }

    fn blank(&mut self, count: usize) {
        for _ in 0..count.min(self.config.allowed_blank_lines) {
            if self.output.is_empty() {
                return;
            }
            self.output.push('\n');
        }
    }

    /// `force` expands a container even where it would fit, `expand` allows the
    /// configured width to expand it at all.
    fn value(
        &self,
        value: &Value<'_>,
        level: usize,
        column: usize,
        force: bool,
        expand: bool,
    ) -> String {
        match value {
            Value::Array(array) => self.array(array, level, column, force, expand),
            Value::InlineTable(table) => self.inline_table(table, level, column, force),
            scalar => scalar.raw().to_string(),
        }
    }

    fn array(
        &self,
        array: &Array<'_>,
        level: usize,
        column: usize,
        force: bool,
        expand: bool,
    ) -> String {
        if !self.expands(array, column, force, expand) {
            return self.flat_array(array);
        }
        let mut out = String::from("[");
        if array.items.is_empty() {
            if let Some(comment) = array.trailing.inline {
                out.push(' ');
                out.push_str(&self.comment(comment));
            }
            for gap in &array.trailing.gaps {
                self.break_line(&mut out, level + 1, gap.blank, gap.comment);
            }
            out.push('\n');
            out.push_str(&self.pad(level));
            out.push(']');
            return out;
        }
        let column = self.indent_width(level + 1);
        let mut bodies: Vec<Rendered> = Vec::with_capacity(array.items.len());
        for (index, item) in array.items.iter().enumerate() {
            let comma = index + 1 < array.items.len() || self.config.array_trailing_comma;
            let mut body = self.value(&item.value, level + 1, column, false, true);
            // An inline table has no line of its own to overflow, so only a
            // nested array can absorb the excess width inside an array item.
            if matches!(item.value, Value::InlineTable(_))
                && self.config.inline_table_expand
                && !fits(column, &body, usize::from(comma), self.config.width)
            {
                body = self.value(&item.value, level + 1, column, true, true);
            }
            bodies.push(Rendered {
                body: format!("{}{}", body, if comma { "," } else { "" }),
                comment: item
                    .trailing
                    .map(|comment| self.comment(comment).into_owned()),
            });
        }
        let runs = runs(array, self.config.allowed_blank_lines);
        let mut start = 0;
        for end in runs {
            self.array_items(&mut out, array, level, &bodies, start, end);
            start = end;
        }
        out.push('\n');
        out.push_str(&self.pad(level));
        out.push(']');
        out
    }

    /// Writes `items[start..end]`, aligning their comments as one group.
    fn array_items(
        &self,
        out: &mut String,
        array: &Array<'_>,
        level: usize,
        bodies: &[Rendered],
        start: usize,
        end: usize,
    ) {
        let group = &bodies[start..end];
        let comments = group.iter().filter(|body| body.comment.is_some()).count();
        let single = group.iter().all(|body| !body.body.contains('\n'));
        let align = single && self.aligns_comments(comments);
        let widest = if align {
            group
                .iter()
                .map(|body| width(&body.body))
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        for (index, item) in array.items[start..end].iter().enumerate() {
            let body = &bodies[start + index];
            if start + index == 0
                && let Some(comment) = item.lead.inline
            {
                out.push(' ');
                out.push_str(&self.comment(comment));
            }
            for gap in &item.lead.gaps {
                self.break_line(out, level + 1, gap.blank, gap.comment);
            }
            self.break_line(out, level + 1, item.lead.blank, "");
            out.push_str(&self.pad(level + 1));
            out.push_str(&body.body);
            if let Some(comment) = &body.comment {
                if align {
                    out.push_str(&" ".repeat(widest.saturating_sub(width(&body.body))));
                }
                out.push(' ');
                out.push_str(comment);
            }
            for gap in &item.after {
                self.break_line(out, level + 1, gap.blank, gap.comment);
            }
        }
    }

    fn expands(&self, array: &Array<'_>, column: usize, force: bool, expand: bool) -> bool {
        if array.items.is_empty() {
            return array.trailing.has_comments();
        }
        if force || array.has_comments() {
            return true;
        }
        let mut multiline = array.multiline && !self.config.array_auto_collapse;
        if !multiline && expand && self.config.array_auto_expand {
            multiline = !fits(column, &self.flat_array(array), 0, self.config.width);
        }
        multiline
    }

    fn flat_array(&self, array: &Array<'_>) -> String {
        let inner = array
            .items
            .iter()
            .map(|item| self.flat_value(&item.value))
            .collect::<Vec<_>>()
            .join(", ");
        if inner.is_empty() {
            return "[]".to_string();
        }
        if self.config.compact_arrays {
            format!("[{inner}]")
        } else {
            format!("[ {inner} ]")
        }
    }

    fn flat_value(&self, value: &Value<'_>) -> String {
        match value {
            Value::Array(array) => self.flat_array(array),
            Value::InlineTable(table) => self.inline_table(table, 0, 0, false),
            scalar => scalar.raw().to_string(),
        }
    }

    fn inline_table(
        &self,
        table: &InlineTable<'_>,
        level: usize,
        column: usize,
        force: bool,
    ) -> String {
        if table.entries.is_empty() {
            return "{}".to_string();
        }
        let mut entries: Vec<&Entry<'_>> = table.entries.iter().collect();
        if self.config.reorder_keys {
            entries.sort_by(|left, right| left.decoded.cmp(&right.decoded));
        }
        let compact = self.config.compact_entries;
        let separator = if compact { 1 } else { 3 };
        let mut position = column + 1 + usize::from(!self.config.compact_inline_tables);
        let mut inner = String::new();
        for (index, entry) in entries.iter().enumerate() {
            if index > 0 {
                inner.push_str(", ");
                position += 2;
            }
            let key = entry.path.join(".");
            position += width(&key) + separator;
            inner.push_str(&key);
            inner.push_str(if compact { "=" } else { " = " });
            let expand = force && self.config.inline_table_expand;
            let value = self.value(&entry.value, level, position, expand, expand);
            position += width(&value);
            inner.push_str(&value);
        }
        if self.config.compact_inline_tables {
            format!("{{{inner}}}")
        } else {
            format!("{{ {inner} }}")
        }
    }

    fn comment<'b>(&self, text: &'b str) -> Cow<'b, str> {
        format_comment(text, self.config.format_comments)
    }

    fn pad(&self, level: usize) -> String {
        " ".repeat(self.indent_width(level))
    }

    fn indent_width(&self, level: usize) -> usize {
        level * self.config.indent
    }

    /// A line break with the blank lines before it and an optional comment.
    fn break_line(&self, out: &mut String, level: usize, blank: usize, comment: &str) {
        out.push('\n');
        for _ in 0..blank.min(self.config.allowed_blank_lines) {
            out.push('\n');
        }
        if !comment.is_empty() {
            out.push_str(&self.pad(level));
            out.push_str(&self.comment(comment));
        }
    }

    fn push_line(&mut self, line: &str) {
        self.output.push_str(line);
        self.output.push('\n');
    }
}

/// Splits array items where an own-line comment or a blank line separates them,
/// so that comment alignment stays local, returning the end of each run.
fn runs(array: &Array<'_>, allowed_blank_lines: usize) -> Vec<usize> {
    let mut runs = Vec::new();
    for (index, item) in array.items.iter().enumerate() {
        let next = array.items.get(index + 1);
        let split = match next {
            None => true,
            Some(next) => {
                next.lead.has_comments()
                    || next.lead.blank.min(allowed_blank_lines) > 0
                    || !item.after.is_empty()
            }
        };
        if split {
            runs.push(index + 1);
        }
    }
    runs
}

/// Copies a written header path, dropping the whitespace TOML allows around
/// dots while keeping quoted keys byte for byte.
fn push_header(out: &mut String, raw: &str) {
    let mut state = 0u8;
    let mut characters = raw.chars();
    while let Some(character) = characters.next() {
        match state {
            0 => match character {
                '"' => {
                    state = 1;
                    out.push(character);
                }
                '\'' => {
                    state = 2;
                    out.push(character);
                }
                ' ' | '\t' => {}
                _ => out.push(character),
            },
            1 => {
                out.push(character);
                match character {
                    '\\' => {
                        if let Some(escaped) = characters.next() {
                            out.push(escaped);
                        }
                    }
                    '"' => state = 0,
                    _ => {}
                }
            }
            _ => {
                out.push(character);
                if character == '\'' {
                    state = 0;
                }
            }
        }
    }
}

/// Width is measured on the first line: content after a line break is a
/// literal or an already laid out value that expanding cannot shorten.
fn fits(column: usize, text: &str, reserve: usize, limit: usize) -> bool {
    let first = text.split('\n').next().unwrap_or("");
    column + width(first) + reserve <= limit
}

fn width(text: &str) -> usize {
    text.chars().count()
}
