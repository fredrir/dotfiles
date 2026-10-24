use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use toml_edit::{
    Array as TomlArray, Datetime, ImDocument, InlineTable as TomlInlineTable, Item, Key, RawString,
    Table, Value as Toml,
};

use crate::trivia::{Gap, Trivia};

/// Levels accepted in one dimension before a document is rejected as too deep.
///
/// The embedded parser stops at 80 levels for arrays, inline tables, dotted
/// keys and table headers, so 79 is the largest value it can hand back.
pub(crate) const MAX_DEPTH: usize = 79;

/// A lossless view of a parsed document: every element keeps the spans it was
/// read from, so printing never re-serializes a key, literal or comment.
pub(crate) struct Document<'a> {
    pub entries: Vec<Entry<'a>>,
    pub sections: Vec<Section<'a>>,
    pub trailing: Trivia<'a>,
}

/// A table header with the elements written under it.
pub(crate) struct Section<'a> {
    pub lead: Trivia<'a>,
    /// Header key components, spelled as written.
    pub path: Vec<&'a str>,
    /// Header key components, decoded; used for indentation and sorting.
    pub decoded: Vec<&'a str>,
    pub array: bool,
    /// The header's written key path, between the brackets, when available.
    /// Keeping it per section preserves the spelling of a key that is shared
    /// with an earlier header written with different quotes.
    pub header: Option<&'a str>,
    /// False for tables that only exist because a deeper header named them.
    pub visible: bool,
    pub trailing: Option<&'a str>,
    pub entries: Vec<Entry<'a>>,
}

pub(crate) struct Entry<'a> {
    pub lead: Trivia<'a>,
    pub path: Vec<&'a str>,
    pub decoded: Vec<&'a str>,
    pub value: Value<'a>,
    pub trailing: Option<&'a str>,
}

pub(crate) enum Value<'a> {
    String { raw: &'a str, decoded: &'a str },
    Integer { raw: &'a str, value: i64 },
    Float { raw: &'a str, value: f64 },
    Boolean { raw: &'a str, value: bool },
    Datetime { raw: &'a str, value: &'a Datetime },
    Array(Array<'a>),
    InlineTable(InlineTable<'a>),
}

impl<'a> Value<'a> {
    pub(crate) fn raw(&self) -> &'a str {
        match self {
            Self::String { raw, .. }
            | Self::Integer { raw, .. }
            | Self::Float { raw, .. }
            | Self::Boolean { raw, .. }
            | Self::Datetime { raw, .. } => raw,
            Self::Array(array) => array.raw,
            Self::InlineTable(table) => table.raw,
        }
    }

    pub(crate) fn has_comments(&self) -> bool {
        match self {
            Self::Array(array) => array.has_comments(),
            Self::InlineTable(table) => table.has_comments(),
            _ => false,
        }
    }
}

pub(crate) struct Array<'a> {
    pub raw: &'a str,
    /// Whether the array was written across several lines; line breaks inside
    /// a multi-line string do not count.
    pub multiline: bool,
    pub items: Vec<ArrayItem<'a>>,
    /// Comments between the last item and `]`; only used by empty arrays.
    pub trailing: Trivia<'a>,
}

impl Array<'_> {
    /// Comments anywhere inside force the whole chain onto several lines,
    /// because a comment ends the line it starts.
    pub(crate) fn has_comments(&self) -> bool {
        self.trailing.has_comments()
            || self.items.iter().any(|item| {
                item.lead.has_comments()
                    || item.trailing.is_some()
                    || !item.after.is_empty()
                    || item.value.has_comments()
            })
    }
}

pub(crate) struct ArrayItem<'a> {
    pub lead: Trivia<'a>,
    pub value: Value<'a>,
    /// A comment on the line of the value.
    pub trailing: Option<&'a str>,
    /// Comments between the value and the next item.
    pub after: Vec<Gap<'a>>,
}

pub(crate) struct InlineTable<'a> {
    pub raw: &'a str,
    pub entries: Vec<Entry<'a>>,
}

impl InlineTable<'_> {
    pub(crate) fn has_comments(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.lead.has_comments() || entry.value.has_comments())
    }
}

pub(crate) fn build<S: AsRef<str>>(document: &ImDocument<S>) -> Result<Document<'_>, Diagnostic> {
    let reader = Reader {
        text: document.raw(),
    };
    let root = document.as_table();
    let mut placed = Vec::new();
    let mut position = 0;
    reader.children(root, 0, &mut Heading::default(), &mut position, &mut placed)?;
    // Headers are printed in the order they were written, as toml_edit does:
    // implicit parents share the position of the header that created them.
    placed.sort_by_key(|(position, _)| *position);
    Ok(Document {
        entries: reader.entries(root, 0)?,
        sections: placed.into_iter().map(|(_, section)| section).collect(),
        trailing: Trivia::parse(reader.text, Some(document.trailing())),
    })
}

/// The heading of the table currently being collected.
#[derive(Default)]
struct Heading<'a> {
    path: Vec<&'a str>,
    decoded: Vec<&'a str>,
}

struct Reader<'a> {
    text: &'a str,
}

impl<'a> Reader<'a> {
    fn slice(&self, span: Option<std::ops::Range<usize>>) -> &'a str {
        span.and_then(|span| self.text.get(span)).unwrap_or("")
    }

    fn key_text(&self, key: &'a Key) -> &'a str {
        key.as_repr()
            .and_then(|repr| repr.as_raw().span())
            .and_then(|span| self.text.get(span))
            .unwrap_or_else(|| key.get())
    }

    fn trivia(&self, raw: Option<&RawString>) -> Trivia<'a> {
        Trivia::parse(self.text, raw)
    }

    fn line_break(&self, raw: Option<&RawString>) -> bool {
        let span = raw.and_then(RawString::span);
        self.slice(span).contains('\n')
    }

    fn entries(&self, table: &'a Table, depth: usize) -> Result<Vec<Entry<'a>>, Diagnostic> {
        let mut values = table.get_values();
        // Dotted keys are flattened per first component; the leaf keys keep
        // the spans they were written at, which restores the written order.
        values.sort_by_key(|(path, _)| leaf_start(path));
        let mut entries = Vec::new();
        for (path, value) in values {
            if let Some(entry) = self.entry(&path, value, depth)? {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    fn entry(
        &self,
        path: &[&'a Key],
        value: &'a Toml,
        depth: usize,
    ) -> Result<Option<Entry<'a>>, Diagnostic> {
        let Some(leaf) = path.last() else {
            return Ok(None);
        };
        // A dotted key nests tables, but the parser counts it separately from
        // value containers, so both dimensions get their own budget.
        if path.len() > MAX_DEPTH {
            return Err(too_deep());
        }
        Ok(Some(Entry {
            lead: self.trivia(leaf.leaf_decor().prefix()),
            path: path.iter().map(|key| self.key_text(key)).collect(),
            decoded: path.iter().map(|key| key.get()).collect(),
            value: self.value(value, depth)?,
            trailing: self.trivia(value.decor().suffix()).inline,
        }))
    }

    fn value(&self, value: &'a Toml, depth: usize) -> Result<Value<'a>, Diagnostic> {
        let raw = self.slice(value.span());
        Ok(match value {
            Toml::String(text) => Value::String {
                raw,
                decoded: text.value(),
            },
            Toml::Integer(number) => Value::Integer {
                raw,
                value: *number.value(),
            },
            Toml::Float(number) => Value::Float {
                raw,
                value: *number.value(),
            },
            Toml::Boolean(flag) => Value::Boolean {
                raw,
                value: *flag.value(),
            },
            Toml::Datetime(time) => Value::Datetime {
                raw,
                value: time.value(),
            },
            Toml::Array(array) => {
                let inner = depth + 1;
                if inner > MAX_DEPTH {
                    return Err(too_deep());
                }
                Value::Array(self.array(array, inner)?)
            }
            Toml::InlineTable(table) => {
                let inner = depth + 1;
                if inner > MAX_DEPTH {
                    return Err(too_deep());
                }
                Value::InlineTable(self.inline_table(table, inner)?)
            }
        })
    }

    fn array(&self, array: &'a TomlArray, depth: usize) -> Result<Array<'a>, Diagnostic> {
        let mut items: Vec<ArrayItem<'a>> = Vec::with_capacity(array.len());
        let mut pending = 0;
        let mut multiline = self.line_break(Some(array.trailing()));
        for value in array.iter() {
            let decor = value.decor();
            multiline |= self.line_break(decor.prefix()) || self.line_break(decor.suffix());
            let mut lead = self.trivia(decor.prefix());
            let suffix = self.trivia(decor.suffix());
            if let Some(previous) = items.last_mut() {
                // A comment at the start of an item's decor trails the comma
                // that introduced it, which belongs to the item before.
                if let Some(inline) = lead.inline.take() {
                    match previous.trailing {
                        None => previous.trailing = Some(inline),
                        Some(_) => previous.after.push(Gap {
                            blank: 0,
                            comment: inline,
                        }),
                    }
                }
                lead.prepend(pending);
            }
            pending = suffix.blank;
            items.push(ArrayItem {
                lead,
                value: self.value(value, depth)?,
                trailing: suffix.inline,
                after: suffix.gaps,
            });
        }
        let mut trailing = self.trivia(Some(array.trailing()));
        match items.last_mut() {
            Some(last) => {
                if let Some(inline) = trailing.inline {
                    match last.trailing {
                        None => last.trailing = Some(inline),
                        Some(_) => last.after.push(Gap {
                            blank: 0,
                            comment: inline,
                        }),
                    }
                }
                last.after.extend(trailing.gaps);
                trailing = Trivia::default();
            }
            None => trailing.prepend(pending),
        }
        Ok(Array {
            raw: self.slice(array.span()),
            multiline,
            items,
            trailing,
        })
    }

    fn inline_table(
        &self,
        table: &'a TomlInlineTable,
        depth: usize,
    ) -> Result<InlineTable<'a>, Diagnostic> {
        let mut values = table.get_values();
        values.sort_by_key(|(path, _)| leaf_start(path));
        let mut entries = Vec::new();
        for (path, value) in values {
            if let Some(entry) = self.entry(&path, value, depth)? {
                entries.push(entry);
            }
        }
        Ok(InlineTable {
            raw: self.slice(table.span()),
            entries,
        })
    }

    /// Walks the tables written below `table`, emitting one section per header
    /// in discovery order; the caller sorts them by their document position.
    fn children(
        &self,
        table: &'a Table,
        depth: usize,
        heading: &mut Heading<'a>,
        last_position: &mut usize,
        placed: &mut Vec<(usize, Section<'a>)>,
    ) -> Result<(), Diagnostic> {
        for (name, item) in table.iter() {
            let Some(key) = table.key(name) else {
                continue;
            };
            heading.path.push(self.key_text(key));
            heading.decoded.push(key.get());
            let result = match item {
                // Dotted keys nest tables, but each dotted path has its own
                // parser budget and must not consume the header depth.
                Item::Table(child) if child.is_dotted() => {
                    self.children(child, depth, heading, last_position, placed)
                }
                Item::Table(child) => {
                    self.sections(child, depth + 1, heading, last_position, placed)
                }
                Item::ArrayOfTables(children) => children.iter().try_for_each(|child| {
                    self.push_section(child, depth + 1, true, heading, last_position, placed)?;
                    self.children(child, depth + 1, heading, last_position, placed)
                }),
                _ => Ok(()),
            };
            heading.path.pop();
            heading.decoded.pop();
            result?;
        }
        Ok(())
    }

    fn sections(
        &self,
        table: &'a Table,
        depth: usize,
        heading: &mut Heading<'a>,
        last_position: &mut usize,
        placed: &mut Vec<(usize, Section<'a>)>,
    ) -> Result<(), Diagnostic> {
        // A dotted key's own entries are already flattened into the parent's
        // entries; only tables below it need a header.
        if !table.is_dotted() {
            self.push_section(table, depth, false, heading, last_position, placed)?;
        }
        self.children(table, depth, heading, last_position, placed)
    }

    /// The key path written in this table's own header, without brackets.
    fn header(&self, table: &'a Table, array: bool) -> Option<&'a str> {
        let text = self.text.get(table.span()?)?;
        let open = if array { "[[" } else { "[" };
        let start = open.len();
        if !text.starts_with(open) {
            return None;
        }
        let bytes = text.as_bytes();
        let mut index = start;
        let mut state = 0u8;
        while index < bytes.len() {
            let byte = bytes[index];
            match state {
                0 => match byte {
                    b'"' => state = 1,
                    b'\'' => state = 2,
                    b']' if !array || bytes.get(index + 1) == Some(&b']') => {
                        return text.get(start..index);
                    }
                    _ => {}
                },
                1 => match byte {
                    b'\\' => index += 1,
                    b'"' => state = 0,
                    _ => {}
                },
                _ => {
                    if byte == b'\'' {
                        state = 0;
                    }
                }
            }
            index += 1;
        }
        None
    }

    fn push_section(
        &self,
        table: &'a Table,
        depth: usize,
        array: bool,
        heading: &Heading<'a>,
        last_position: &mut usize,
        placed: &mut Vec<(usize, Section<'a>)>,
    ) -> Result<(), Diagnostic> {
        if depth > MAX_DEPTH {
            return Err(too_deep());
        }
        if let Some(position) = table.position() {
            *last_position = position;
        }
        placed.push((
            *last_position,
            Section {
                lead: self.trivia(table.decor().prefix()),
                path: heading.path.clone(),
                decoded: heading.decoded.clone(),
                array,
                header: self.header(table, array),
                visible: array || !table.is_implicit() || !table.get_values().is_empty(),
                trailing: self.trivia(table.decor().suffix()).inline,
                entries: self.entries(table, depth)?,
            },
        ));
        Ok(())
    }
}

/// Where the last key of a (possibly dotted) entry was written.
fn leaf_start(path: &[&Key]) -> usize {
    path.last()
        .and_then(|key| key.span())
        .map_or(usize::MAX, |span| span.start)
}

fn too_deep() -> Diagnostic {
    Diagnostic::new(
        DiagnosticKind::Syntax,
        format!("unsupported nesting deeper than {MAX_DEPTH} levels"),
    )
}
