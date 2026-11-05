use std::borrow::Cow;

use toml_edit::RawString;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Gap<'a> {
    pub blank: usize,
    pub comment: &'a str,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Trivia<'a> {
    pub inline: Option<&'a str>,
    pub gaps: Vec<Gap<'a>>,
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

fn comment(line: &str) -> Option<&str> {
    let text = line
        .trim_start_matches([' ', '\t'])
        .trim_end_matches([' ', '\t']);
    text.starts_with('#').then_some(text)
}

pub(crate) fn format_comment(text: &str, enabled: bool) -> Cow<'_, str> {
    match text.strip_prefix('#') {
        Some(rest) if enabled && !rest.is_empty() && !rest.starts_with([' ', '\t', '#']) => {
            Cow::Owned(format!("# {rest}"))
        }
        _ => Cow::Borrowed(text),
    }
}
