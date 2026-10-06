use std::ops::Range;

use comrak::{Arena, Options, nodes::NodeValue, parse_document};

// Keep syntax outside CommonMark opaque. Table alignment measures the restored
// text, so token lengths cannot affect the layout.
pub struct Protected {
    pub text: String,
    marker: char,
    originals: Vec<String>,
}

impl Protected {
    pub fn new(input: &str, options: &Options<'_>, obsidian: bool) -> Result<Self, String> {
        let marker = (0xe000..=0xf8ff)
            .filter_map(char::from_u32)
            .chain((0xf0000..=0xffffd).filter_map(char::from_u32))
            .find(|marker| !input.contains(*marker))
            .ok_or("could not reserve a marker to preserve Obsidian syntax")?;
        let mut result = Self {
            text: String::with_capacity(input.len()),
            marker,
            originals: Vec::new(),
        };
        let arena = Arena::new();
        let root = parse_document(&arena, input, options);
        let mut starts = vec![0];
        starts.extend(input.match_indices('\n').map(|(offset, _)| offset + 1));
        let mut excluded = Vec::new();
        let mut blocks = Vec::new();
        for node in root.descendants() {
            let data = node.data.borrow();
            if data.sourcepos.start.line == 0 || data.sourcepos.end.line == 0 {
                continue;
            }
            let start = starts[data.sourcepos.start.line - 1]
                + data.sourcepos.start.column.saturating_sub(1);
            let end =
                (starts[data.sourcepos.end.line - 1] + data.sourcepos.end.column).min(input.len());
            if start > end || !input.is_char_boundary(start) || !input.is_char_boundary(end) {
                continue;
            }
            let range = start..end;
            match &data.value {
                NodeValue::Code(_)
                | NodeValue::CodeBlock(_)
                | NodeValue::HtmlInline(_)
                | NodeValue::HtmlBlock(_)
                | NodeValue::FrontMatter(_)
                | NodeValue::Link(_)
                | NodeValue::Image(_) => excluded.push(range),
                // Obsidian callouts have arbitrary types, folding markers and
                // titles. Preserve the entire container, including nested ones.
                NodeValue::BlockQuote
                    if obsidian
                        && input[range.clone()].lines().next().is_some_and(|line| {
                            line.trim_start_matches(['>', ' ', '\t']).starts_with("[!")
                        }) =>
                {
                    blocks.push(range)
                }
                NodeValue::Paragraph
                    if obsidian
                        && input[range.clone()]
                            .lines()
                            .any(|line| block_id(line.trim())) =>
                {
                    blocks.push(range)
                }
                _ => {}
            }
        }
        excluded.sort_by_key(|range| range.start);
        blocks.sort_by_key(|range| range.start);
        let mut excluded = excluded.into_iter().peekable();
        let mut blocks = blocks.into_iter().peekable();
        let mut at = 0;
        while at < input.len() {
            while excluded.peek().is_some_and(|r| r.end <= at) {
                excluded.next();
            }
            while blocks.peek().is_some_and(|r| r.end <= at) {
                blocks.next();
            }
            if let Some(range) = covering(&mut blocks, at) {
                result.keep(&input[at..range.end]);
                at = range.end;
                continue;
            }
            if let Some(range) = covering(&mut excluded, at) {
                result.text.push_str(&input[at..range.end]);
                at = range.end;
                continue;
            }
            let rest = &input[at..];
            if rest.starts_with('\\') {
                let count = rest.chars().take(2).map(char::len_utf8).sum::<usize>();
                result.text.push_str(&rest[..count]);
                at += count;
                continue;
            }
            let length = if obsidian && rest.starts_with("![[") {
                delimited(rest, 3, "]]", false)
            } else if obsidian && rest.starts_with("[[") {
                delimited(rest, 2, "]]", false)
            } else if obsidian && rest.starts_with("%%") {
                Some(delimited(rest, 2, "%%", true).unwrap_or(rest.len()))
            } else if rest.starts_with("$$") {
                delimited(rest, 2, "$$", true)
            } else if rest.starts_with('$')
                && rest.chars().nth(1).is_some_and(|c| !c.is_whitespace())
            {
                delimited(rest, 1, "$", false)
            } else if obsidian && rest.starts_with("^[") {
                delimited(rest, 2, "]", false)
            } else if obsidian && (rest.starts_with('#') || rest.starts_with('^')) {
                let end = rest
                    .char_indices()
                    .skip(1)
                    .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
                    .last()
                    .map(|(i, c)| i + c.len_utf8());
                end.filter(|_| {
                    at == 0
                        || input[..at]
                            .chars()
                            .next_back()
                            .is_some_and(|c| c.is_whitespace())
                })
            } else {
                None
            };
            if let Some(length) = length {
                result.keep(&rest[..length]);
                at += length;
            } else {
                let c = rest.chars().next().expect("nonempty remainder");
                result.text.push(c);
                at += c.len_utf8();
            }
        }
        Ok(result)
    }

    fn keep(&mut self, source: &str) {
        let index = self.originals.len().to_string();
        self.text.push(self.marker);
        self.text.push_str(&index);
        self.text.push(self.marker);
        self.originals.push(source.to_owned());
    }

    pub fn restore(&self, output: &str) -> Result<String, String> {
        let mut restored = String::with_capacity(output.len());
        let mut remaining = output;
        while let Some(start) = remaining.find(self.marker) {
            restored.push_str(&remaining[..start]);
            remaining = &remaining[start + self.marker.len_utf8()..];
            let end = remaining
                .find(self.marker)
                .ok_or("could not preserve Obsidian syntax")?;
            let index: usize = remaining[..end]
                .parse()
                .map_err(|_| "could not preserve Obsidian syntax")?;
            restored.push_str(
                self.originals
                    .get(index)
                    .ok_or("could not preserve Obsidian syntax")?,
            );
            remaining = &remaining[end + self.marker.len_utf8()..];
        }
        restored.push_str(remaining);
        Ok(restored)
    }
}

fn covering<I: Iterator<Item = Range<usize>>>(
    ranges: &mut std::iter::Peekable<I>,
    at: usize,
) -> Option<Range<usize>> {
    ranges
        .peek()
        .filter(|r| r.start <= at && at < r.end)
        .cloned()
}

fn block_id(line: &str) -> bool {
    line.strip_prefix('^').is_some_and(|id| {
        !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

fn delimited(text: &str, opening: usize, closing: &str, multiline: bool) -> Option<usize> {
    let mut at = opening;
    while at < text.len() {
        let rest = &text[at..];
        if rest.starts_with(closing) {
            return Some(at + closing.len());
        }
        if !multiline && rest.starts_with('\n') {
            return None;
        }
        let count = if rest.starts_with('\\') { 2 } else { 1 };
        at += rest.chars().take(count).map(char::len_utf8).sum::<usize>();
    }
    None
}
