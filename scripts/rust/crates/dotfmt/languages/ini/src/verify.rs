use crate::config::Config;
use crate::dialect::Dialect;
use crate::parse::{Document, LineKind, Separator, comment_body, trim_end_layout, trim_layout};

#[derive(Debug, PartialEq, Eq)]
pub struct Fingerprint {
    bom: bool,
    items: Vec<Item>,
}

#[derive(Debug, PartialEq, Eq)]
struct Item {
    blank: usize,
    kind: ItemKind,
}

#[derive(Debug, PartialEq, Eq)]
enum ItemKind {
    Section(String),
    Entry {
        key: String,
        value: String,
        separator: Separator,
    },
    Comment {
        marker: char,
        body: String,
    },
    Continuation(String),
    Raw(String),
}

impl Fingerprint {
    pub fn of(document: &Document<'_>, config: &Config, dialect: Dialect) -> Fingerprint {
        let mut items: Vec<Item> = Vec::new();
        let mut pending_blank = 0;
        for line in &document.lines {
            if matches!(line.kind, LineKind::Blank) {
                pending_blank += 1;
                continue;
            }
            let blank = if items.is_empty() {
                0
            } else if dialect == Dialect::Generic && matches!(line.kind, LineKind::Continuation(_))
            {
                pending_blank
            } else {
                pending_blank.min(config.blank_lines)
            };
            items.push(Item {
                blank,
                kind: kind(&line.kind, config),
            });
            pending_blank = 0;
        }
        while matches!(
            items.last(),
            Some(Item {
                kind: ItemKind::Continuation(text),
                ..
            }) if text.is_empty()
        ) {
            items.pop();
        }
        Fingerprint {
            bom: document.bom,
            items,
        }
    }
}

fn kind(line: &LineKind<'_>, config: &Config) -> ItemKind {
    match line {
        LineKind::Blank => unreachable!("blank lines are not fingerprint items"),
        LineKind::Section(header) => ItemKind::Section((*header).to_owned()),
        LineKind::Entry(entry) => ItemKind::Entry {
            key: entry.key.to_owned(),
            value: if entry.value.ends_with('\\') {
                format!("{}{}", entry.value, entry.trailing)
            } else {
                trim_layout(entry.value).to_owned()
            },
            separator: entry.separator,
        },
        LineKind::Comment(comment) => ItemKind::Comment {
            marker: comment.marker,
            body: comment_body(comment.body, config.format_comments),
        },
        LineKind::Continuation(raw) => ItemKind::Continuation((*raw).to_owned()),
        LineKind::Raw(raw) => ItemKind::Raw(trim_end_layout(raw).to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::Dialect;
    use crate::parse::parse;

    #[test]
    fn fingerprints_distinguish_values_keys_and_separators() {
        let config = Config::default();
        let one = parse("key=one\n", Dialect::Generic);
        let two = parse("key=two\n", Dialect::Generic);
        let other = parse("other=one\n", Dialect::Generic);
        assert_ne!(
            Fingerprint::of(&one, &config, Dialect::Generic),
            Fingerprint::of(&two, &config, Dialect::Generic)
        );
        assert_ne!(
            Fingerprint::of(&one, &config, Dialect::Generic),
            Fingerprint::of(&other, &config, Dialect::Generic)
        );
        let equals = parse("Key=\n", Dialect::Ssh);
        let space = parse("Key value\n", Dialect::Ssh);
        assert_ne!(
            Fingerprint::of(&equals, &config, Dialect::Ssh),
            Fingerprint::of(&space, &config, Dialect::Ssh)
        );
    }

    #[test]
    fn fingerprints_clamp_blank_runs_and_ignore_outer_blank_runs() {
        let config = Config {
            blank_lines: 1,
            ..Config::default()
        };
        let one = parse("\n\na=1\n\n\nb=2\n\n", Dialect::Generic);
        let two = parse("a=1\n\nb=2", Dialect::Generic);
        assert_eq!(
            Fingerprint::of(&one, &config, Dialect::Generic),
            Fingerprint::of(&two, &config, Dialect::Generic)
        );
    }
}
