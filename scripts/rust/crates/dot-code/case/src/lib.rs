#![forbid(unsafe_code)]

use std::collections::HashMap;

use heck::{ToKebabCase, ToLowerCamelCase, ToPascalCase, ToSnakeCase, ToTitleCase};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Case {
    Camel,
    Kebab,
    Lower,
    Pascal,
    Snake,
    Title,
    Upper,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "scope", rename_all = "camelCase")]
pub enum Target {
    Selection { text: String },
    Cursor { before: String, after: String },
    FileName { name: String },
}

#[derive(Debug, Deserialize)]
pub struct Params {
    pub case: Case,
    pub targets: Vec<Target>,
}

/// `text` replaces the selection, or for a cursor the last `before` and first `after` UTF-16 units around it.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Replacement {
    pub text: String,
    pub before: usize,
    pub after: usize,
}

/// Words behind the last transform's results, so `someThing` lowered to `something` comes back as `SomeThing`.
#[derive(Debug, Default)]
pub struct Memory {
    words: HashMap<String, String>,
}

impl Memory {
    pub fn transform(&mut self, params: &Params) -> Vec<Replacement> {
        let mut converter = Converter {
            case: params.case,
            recalled: std::mem::take(&mut self.words),
            remembered: &mut self.words,
        };
        params
            .targets
            .iter()
            .map(|target| converter.target(target))
            .collect()
    }
}

struct Converter<'a> {
    case: Case,
    recalled: HashMap<String, String>,
    remembered: &'a mut HashMap<String, String>,
}

impl Converter<'_> {
    fn target(&mut self, target: &Target) -> Replacement {
        match target {
            Target::Selection { text } => Replacement {
                text: self.convert(text),
                before: 0,
                after: 0,
            },
            Target::Cursor { before, after } => self.around_cursor(before, after),
            Target::FileName { name } => Replacement {
                text: self.file_name(name),
                before: 0,
                after: 0,
            },
        }
    }

    /// Converts the word touching the cursor, or its whole line when there is none.
    fn around_cursor(&mut self, before: &str, after: &str) -> Replacement {
        let mut start = before.trim_end_matches(is_word).len();
        let mut end = after.len() - after.trim_start_matches(is_word).len();
        if start == before.len() && end == 0 {
            start = before.rfind('\n').map_or(0, |newline| newline + 1);
            end = after.find('\n').unwrap_or(after.len());
        }
        let (before, after) = (&before[start..], &after[..end]);
        Replacement {
            text: self.convert(&format!("{before}{after}")),
            before: before.encode_utf16().count(),
            after: after.encode_utf16().count(),
        }
    }

    /// Converts the name up to its first extension; a leading dot is part of the name.
    fn file_name(&mut self, name: &str) -> String {
        let dot = name
            .char_indices()
            .skip(1)
            .find_map(|(index, c)| (c == '.').then_some(index))
            .unwrap_or(name.len());
        let (stem, extensions) = name.split_at(dot);
        format!("{}{extensions}", self.convert(stem))
    }

    /// Line breaks and punctuation other than `_`, `-` and whitespace stay in place.
    fn convert(&mut self, text: &str) -> String {
        phrases(text)
            .map(|(phrase, run)| {
                if phrase {
                    self.phrase(run)
                } else {
                    run.to_owned()
                }
            })
            .collect()
    }

    fn phrase(&mut self, phrase: &str) -> String {
        let separator = |c: char| !c.is_alphanumeric();
        let start = phrase.len() - phrase.trim_start_matches(separator).len();
        let end = phrase.trim_end_matches(separator).len().max(start);
        format!(
            "{}{}{}",
            &phrase[..start],
            self.words(&phrase[start..end]),
            &phrase[end..]
        )
    }

    fn words(&mut self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let words = self
            .recalled
            .get(text)
            .cloned()
            .unwrap_or_else(|| text.to_snake_case());
        let converted = match self.case {
            Case::Camel => words.to_lower_camel_case(),
            Case::Kebab => words.to_kebab_case(),
            Case::Lower => text.to_lowercase(),
            Case::Pascal => words.to_pascal_case(),
            Case::Snake => words.to_snake_case(),
            Case::Title => words.to_title_case(),
            Case::Upper => text.to_uppercase(),
        };
        self.remembered.insert(converted.clone(), words);
        converted
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn is_phrase(c: char) -> bool {
    is_word(c) || (c.is_whitespace() && !matches!(c, '\n' | '\r'))
}

fn phrases(text: &str) -> impl Iterator<Item = (bool, &str)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let phrase = is_phrase(rest.chars().next()?);
        let end = rest.find(|c| is_phrase(c) != phrase).unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        rest = tail;
        Some((phrase, run))
    })
}
