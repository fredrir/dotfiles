use std::collections::HashSet;
use std::fmt::Write;

use ui_theme::{Role, Style};

const DESCRIPTION_WIDTH: usize = 90;
const NAME_WIDTH: usize = 36;
const GAP: &str = "  ";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reply {
    skip: Option<String>,
    sections: Vec<Section>,
    delegate: bool,
    fallback: Vec<Section>,
    seen: HashSet<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Section {
    Group(Group),
    Files { directories: bool },
    Message(String),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Group {
    pub tag: String,
    pub label: String,
    pub tone: Tone,
    pub items: Vec<Item>,
    pub unsorted: bool,
    pub suffix: Option<String>,
    pub removable_suffix: bool,
    pub insert_prefix: Option<String>,
    pub replace: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Muted,
    #[default]
    Accent,
    Info,
    Success,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub value: String,
    pub details: Vec<(Tone, String)>,
}

impl Tone {
    fn role(self) -> Role {
        match self {
            Tone::Plain => Role::Plain,
            Tone::Muted => Role::Muted,
            Tone::Accent => Role::Accent,
            Tone::Info => Role::Info,
            Tone::Success => Role::Success,
            Tone::Warning => Role::Warning,
        }
    }
}

impl Item {
    pub fn new(value: impl Into<String>, description: impl Into<String>) -> Item {
        Item::bare(value).detail(Tone::Muted, description)
    }

    pub fn bare(value: impl Into<String>) -> Item {
        Item {
            value: value.into(),
            details: Vec::new(),
        }
    }

    // An empty detail still holds its column, so the ones after it stay aligned.
    pub fn detail(mut self, tone: Tone, text: impl Into<String>) -> Item {
        let text = clean(&text.into()).trim().to_string();
        self.details.push((tone, text));
        self
    }
}

impl Group {
    pub fn new(tag: &str, label: &str) -> Group {
        Group {
            tag: tag.into(),
            label: label.into(),
            ..Group::default()
        }
    }

    pub fn tone(mut self, tone: Tone) -> Group {
        self.tone = tone;
        self
    }

    pub fn unsorted(mut self) -> Group {
        self.unsorted = true;
        self
    }

    pub fn suffix(mut self, suffix: &str, removable: bool) -> Group {
        self.suffix = Some(suffix.into());
        self.removable_suffix = removable;
        self
    }

    // Offered without matching the typed word, which the chosen value replaces.
    pub fn replace(mut self) -> Group {
        self.replace = true;
        self
    }

    // Inserted before each value without taking part in matching.
    pub fn insert_prefix(mut self, prefix: &str) -> Group {
        self.insert_prefix = Some(prefix.into());
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = Item>) -> Group {
        self.items.extend(items);
        self
    }
}

impl Reply {
    pub fn new() -> Reply {
        Reply::default()
    }

    pub fn skip(&mut self, literal: impl Into<String>) {
        let literal = literal.into();
        if !literal.is_empty() {
            self.skip = Some(literal);
        }
    }

    pub fn group(&mut self, mut group: Group) {
        let seen = &mut self.seen;
        group
            .items
            .retain(|item| !item.value.is_empty() && seen.insert(item.value.clone()));
        if group.items.is_empty() {
            return;
        }
        self.target().push(Section::Group(group));
    }

    pub fn files(&mut self) {
        self.target().push(Section::Files { directories: false });
    }

    pub fn directories(&mut self) {
        self.target().push(Section::Files { directories: true });
    }

    pub fn message(&mut self, text: impl Into<String>) {
        self.target().push(Section::Message(text.into()));
    }

    // Whatever is added afterwards is used only when the command has no completer of its own.
    pub fn delegate(&mut self) {
        self.delegate = true;
    }

    #[cfg(test)]
    pub fn values(&self) -> Vec<&str> {
        self.sections
            .iter()
            .chain(&self.fallback)
            .filter_map(|section| match section {
                Section::Group(group) => Some(group),
                _ => None,
            })
            .flat_map(|group| group.items.iter().map(|item| item.value.as_str()))
            .collect()
    }

    pub fn render(&self, style: &Style) -> String {
        let mut out = String::new();
        if let Some(skip) = &self.skip {
            let _ = writeln!(out, "skip\t{}", clean(skip));
        }
        render_sections(&mut out, &self.sections, style);
        if self.delegate {
            out.push_str("delegate\n");
            render_sections(&mut out, &self.fallback, style);
        }
        out
    }

    #[cfg(test)]
    pub fn plain(&self) -> String {
        self.render(&Style::plain())
    }

    fn target(&mut self) -> &mut Vec<Section> {
        if self.delegate {
            &mut self.fallback
        } else {
            &mut self.sections
        }
    }
}

fn render_sections(out: &mut String, sections: &[Section], style: &Style) {
    for section in sections {
        match section {
            Section::Files { directories } => {
                out.push_str(if *directories { "dirs\n" } else { "files\n" });
            }
            Section::Message(text) => {
                let _ = writeln!(out, "message\t{}", clean(text));
            }
            Section::Group(group) => render_group(out, group, style),
        }
    }
}

// Each item carries its own display line: the name padded to a column, then its details.
fn render_group(out: &mut String, group: &Group, style: &Style) {
    let described = group
        .items
        .iter()
        .any(|item| item.details.iter().any(|(_, text)| !text.is_empty()));
    let columns = column_widths(group);
    let width = group
        .items
        .iter()
        .map(|item| item.value.chars().count())
        .max()
        .unwrap_or(0)
        .min(NAME_WIDTH);
    let mut options = Vec::new();
    if described {
        options.push("lines".to_string());
    }
    if group.unsorted {
        options.push("unsorted".to_string());
    }
    if let Some(suffix) = &group.suffix {
        options.push(format!("suffix={suffix}"));
    }
    if group.removable_suffix {
        options.push("removable".to_string());
    }
    if group.replace {
        options.push("replace".to_string());
    }
    if let Some(prefix) = &group.insert_prefix {
        options.push(format!("prefix={prefix}"));
    }
    let _ = write!(out, "group\t{}\t{}", clean(&group.tag), clean(&group.label));
    for option in options {
        let _ = write!(out, "\t{}", clean(&option));
    }
    out.push('\n');
    for item in &group.items {
        let value = clean(&item.value);
        let mut display = paint_name(style, group.tone, &value);
        let shown = item
            .details
            .iter()
            .rposition(|(_, text)| !text.is_empty())
            .map_or(0, |last| last + 1);
        if shown > 0 {
            display.push_str(&" ".repeat(width.saturating_sub(value.chars().count())));
            let mut room = DESCRIPTION_WIDTH;
            for (column, (tone, text)) in item.details[..shown].iter().enumerate() {
                if room < 2 {
                    break;
                }
                let text = shorten(text, room);
                let used = if column + 1 < shown {
                    columns[column]
                } else {
                    text.chars().count()
                };
                display.push_str(GAP);
                display.push_str(&style.paint(tone.role(), &text));
                display.push_str(&" ".repeat(used.saturating_sub(text.chars().count())));
                room = room.saturating_sub(used + GAP.len());
            }
        }
        let _ = writeln!(out, "item\t{value}\t{display}");
    }
}

// A scoped package reads as its scope, a slash, then the name, each in its own color.
fn paint_name(style: &Style, tone: Tone, value: &str) -> String {
    let at = if value.starts_with('@') {
        Some(0)
    } else {
        value.find(":@").map(|colon| colon + 1)
    };
    let split = at.and_then(|at| {
        let slash = at + value[at..].find('/')?;
        let name = &value[slash + 1..];
        (slash > at + 1 && !name.is_empty() && !name.contains('/')).then_some((at, slash))
    });
    let Some((at, slash)) = split else {
        return style.paint(tone.role(), value);
    };
    [
        style.paint(tone.role(), &value[..at]),
        style.paint(Tone::Info.role(), &value[at..slash]),
        style.paint(Tone::Muted.role(), "/"),
        style.paint(tone.role(), &value[slash + 1..]),
    ]
    .concat()
}

fn column_widths(group: &Group) -> Vec<usize> {
    let mut widths: Vec<usize> = Vec::new();
    for item in &group.items {
        for (column, (_, text)) in item.details.iter().enumerate() {
            let width = text.chars().count().min(DESCRIPTION_WIDTH);
            match widths.get_mut(column) {
                Some(known) => *known = (*known).max(width),
                None => widths.push(width),
            }
        }
    }
    widths
}

fn clean(text: &str) -> String {
    if !text.contains(['\t', '\n', '\r']) {
        return text.to_string();
    }
    text.split(['\t', '\n', '\r'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn shorten(text: &str, width: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut short: String = text.chars().take(width - 1).collect();
    short.truncate(short.trim_end().len());
    short.push('…');
    short
}

#[cfg(test)]
#[path = "../tests/unit/reply_tests.rs"]
mod tests;
