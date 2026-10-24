use std::collections::HashMap;

use crate::config::Config;
use crate::model::{Document, Entry, Section, Value};
use crate::trivia::{Trivia, format_comment};

/// What a document means, independent of its layout: every value addressed by
/// its decoded key path, plus every comment in written order.
pub(crate) struct Fingerprint {
    values: Vec<(String, String)>,
    comments: Vec<String>,
}

impl Fingerprint {
    pub(crate) fn of(document: &Document<'_>, config: &Config) -> Self {
        let mut fingerprint = Self {
            values: Vec::new(),
            comments: Vec::new(),
        };
        fingerprint.entries(&document.entries, "", config);
        fingerprint.sections(&document.sections, "", config);
        fingerprint.trivia(&document.trailing, config);
        fingerprint.values.sort();
        if config.reorder_keys {
            // Sorting entries reorders the comments that travel with them.
            fingerprint.comments.sort();
        }
        fingerprint
    }

    pub(crate) fn matches(&self, other: &Self) -> bool {
        self.values == other.values
    }

    pub(crate) fn comments_match(&self, other: &Self) -> bool {
        self.comments == other.comments
    }

    fn entries(&mut self, entries: &[Entry<'_>], prefix: &str, config: &Config) {
        for entry in entries {
            self.trivia(&entry.lead, config);
            let mut path = prefix.to_string();
            for (raw, decoded) in entry.path.iter().zip(&entry.decoded) {
                push(&mut path, decoded);
                push(&mut path, raw);
            }
            let mut value = String::new();
            self.value(&entry.value, &mut value, config);
            self.values.push((path, value));
            if let Some(comment) = entry.trailing {
                self.comment(comment, config);
            }
        }
    }

    fn sections(&mut self, sections: &[Section<'_>], prefix: &str, config: &Config) {
        let mut arrays: HashMap<Vec<&str>, usize> = HashMap::new();
        for section in sections {
            self.trivia(&section.lead, config);
            if let Some(comment) = section.trailing {
                self.comment(comment, config);
            }
            let mut path = prefix.to_string();
            for (raw, decoded) in section.path.iter().zip(&section.decoded) {
                push(&mut path, decoded);
                push(&mut path, raw);
            }
            if section.array {
                let index = arrays.entry(section.decoded.clone()).or_default();
                push(&mut path, &index.to_string());
                *index += 1;
            }
            self.entries(&section.entries, &path, config);
        }
    }

    fn value(&mut self, value: &Value<'_>, out: &mut String, config: &Config) {
        match value {
            Value::String { raw, decoded } => {
                out.push('S');
                push(out, decoded);
                push(out, raw);
            }
            Value::Integer { raw, value } => {
                out.push('I');
                push(out, &value.to_string());
                push(out, raw);
            }
            Value::Float { raw, value } => {
                // Bit patterns compare NaN with NaN and keep signed zero apart.
                out.push('F');
                push(out, &format!("{:016x}", value.to_bits()));
                push(out, raw);
            }
            Value::Boolean { raw, value } => {
                out.push('B');
                push(out, if *value { "true" } else { "false" });
                push(out, raw);
            }
            Value::Datetime { raw, value } => {
                out.push('D');
                push(out, &value.to_string());
                push(out, raw);
            }
            Value::Array(array) => {
                out.push('[');
                for item in &array.items {
                    self.trivia(&item.lead, config);
                    self.value(&item.value, out, config);
                    if let Some(comment) = item.trailing {
                        self.comment(comment, config);
                    }
                    for gap in &item.after {
                        self.comment(gap.comment, config);
                    }
                }
                self.trivia(&array.trailing, config);
                out.push(']');
            }
            Value::InlineTable(table) => {
                // Inline tables are maps: entry order carries no meaning.
                let mut entries = Vec::with_capacity(table.entries.len());
                for entry in &table.entries {
                    let mut rendered = String::new();
                    for (raw, decoded) in entry.path.iter().zip(&entry.decoded) {
                        push(&mut rendered, decoded);
                        push(&mut rendered, raw);
                    }
                    self.value(&entry.value, &mut rendered, config);
                    entries.push(rendered);
                }
                entries.sort();
                out.push('{');
                for entry in entries {
                    push(out, &entry);
                }
                out.push('}');
            }
        }
    }

    fn trivia(&mut self, trivia: &Trivia<'_>, config: &Config) {
        for comment in trivia.comments() {
            self.comment(comment, config);
        }
    }

    fn comment(&mut self, comment: &str, config: &Config) {
        self.comments
            .push(format_comment(comment, config.format_comments).into_owned());
    }
}

/// Length-prefixes `text` so that concatenated fields stay unambiguous.
fn push(out: &mut String, text: &str) {
    out.push_str(&text.len().to_string());
    out.push(':');
    out.push_str(text);
}
