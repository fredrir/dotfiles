use std::borrow::Cow;

use toml_edit::RawString;

/// One comment on its own line, with the blank lines that precede it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Gap<'a> {
    pub blank: usize,
    pub comment: &'a str,
}

/// Whitespace and comments between two elements, parsed from the decor that
/// `toml_edit` keeps for them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Trivia<'a> {
    /// A comment on the line of the element before this one.
    pub inline: Option<&'a str>,
    /// Comments on their own lines, in source order.
    pub gaps: Vec<Gap<'a>>,
    /// Blank lines between the last comment and the following element.
    pub blank: usize,
}

impl<'a> Trivia<'a> {
    pub(crate) fn parse(text: &'a str, raw: Option<&RawString>) -> Self {
        let mut trivia = Self::default();
        let Some(range) = raw.and_then(RawString::span) else {
            return trivia;
        };
        let Some(slice) = text.get(range.clone()) else {
            return trivia;
        };
        let at_line_start =
            range.start == 0 || text.as_bytes().get(range.start - 1) == Some(&b'\n');
        let mut rest = slice;
        if !at_line_start {
            // The decor starts on the line of the element before it, so a
            // comment here trails that element.
            let (line, tail) = rest.split_once('\n').unwrap_or((rest, ""));
            trivia.inline = comment(line);
            rest = tail;
        }
        let mut blank = 0;
        while let Some((line, tail)) = rest.split_once('\n') {
            line_gap(&mut trivia, &mut blank, line);
            rest = tail;
        }
        match comment(rest) {
            Some(line) => trivia.gaps.push(Gap {
                blank,
                comment: line,
            }),
            None => trivia.blank = blank,
        }
        trivia
    }

    /// Adds blank lines in front of whatever this trivia describes next.
    pub(crate) fn prepend(&mut self, blank: usize) {
        match self.gaps.first_mut() {
            Some(gap) => gap.blank += blank,
            None => self.blank += blank,
        }
    }

    pub(crate) fn has_comments(&self) -> bool {
        self.inline.is_some() || !self.gaps.is_empty()
    }

    pub(crate) fn comments(&self) -> impl Iterator<Item = &'a str> {
        self.inline
            .into_iter()
            .chain(self.gaps.iter().map(|gap| gap.comment))
    }
}

fn line_gap<'a>(trivia: &mut Trivia<'a>, blank: &mut usize, line: &'a str) {
    match comment(line) {
        Some(comment) => {
            trivia.gaps.push(Gap {
                blank: *blank,
                comment,
            });
            *blank = 0;
        }
        None => *blank += 1,
    }
}

/// The comment on one decor line, without trailing whitespace.
fn comment(line: &str) -> Option<&str> {
    let text = line
        .trim_start_matches([' ', '\t'])
        .trim_end_matches([' ', '\t']);
    text.starts_with('#').then_some(text)
}

/// Comment text as written, optionally with one space inserted after `#`.
pub(crate) fn format_comment(text: &str, enabled: bool) -> Cow<'_, str> {
    match text.strip_prefix('#') {
        Some(rest) if enabled && !rest.is_empty() && !rest.starts_with([' ', '\t', '#']) => {
            Cow::Owned(format!("# {rest}"))
        }
        _ => Cow::Borrowed(text),
    }
}
