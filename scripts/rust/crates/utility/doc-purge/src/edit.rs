use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Comment,
    Doc,
    Glyph,
}

pub struct Cut {
    pub span: Range<usize>,
    pub with: Option<String>,
    pub kind: Kind,
    pub lines: bool,
}

#[derive(Default)]
pub struct Edited {
    pub content: Vec<u8>,
    pub minus: usize,
    pub plus: usize,
    pub comments: usize,
    pub glyphs: usize,
    pub docs: usize,
}

impl Edited {
    pub fn touched(&self) -> bool {
        self.minus > 0 || self.plus > 0
    }
}

const BOM: &[u8] = b"\xef\xbb\xbf";
const LS: &[u8] = "\u{2028}".as_bytes();
const PS: &[u8] = "\u{2029}".as_bytes();

fn line_break(source: &[u8], at: usize) -> Option<usize> {
    match source.get(at).copied()? {
        b'\n' => Some(1),
        b'\r' => Some(if source.get(at + 1) == Some(&b'\n') {
            2
        } else {
            1
        }),
        _ if source[at..].starts_with(LS) => Some(LS.len()),
        _ if source[at..].starts_with(PS) => Some(PS.len()),
        _ => None,
    }
}

fn blank_bytes(bytes: &[u8]) -> bool {
    let mut at = 0usize;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if byte < 0x80 {
            return false;
        }
        let width = crate::scan::char_width(bytes, at);
        let Ok(text) = std::str::from_utf8(&bytes[at..at + width]) else {
            return false;
        };
        let Some(letter) = text.chars().next() else {
            return false;
        };
        if !(letter.is_whitespace() || letter == '\u{feff}') {
            return false;
        }
        at += width;
    }
    true
}

pub fn line_start(source: &[u8], at: usize) -> usize {
    let mut cursor = at.min(source.len());
    while cursor > 0 {
        if source[cursor - 1] == b'\n' || source[cursor - 1] == b'\r' {
            return cursor;
        }
        if cursor >= 3 && (source[cursor - 3..cursor] == *LS || source[cursor - 3..cursor] == *PS) {
            return cursor;
        }
        cursor -= 1;
    }
    0
}

pub fn whole_lines(source: &[u8], span: &Range<usize>) -> Option<Range<usize>> {
    let begin = line_start(source, span.start);
    let cut_begin = if begin == 0 && source.starts_with(BOM) {
        BOM.len()
    } else {
        begin
    };
    if !blank_bytes(&source[cut_begin..span.start]) {
        return None;
    }
    let mut content_end = span.end;
    while content_end < source.len() && line_break(source, content_end).is_none() {
        content_end += 1;
    }
    if !blank_bytes(&source[span.end..content_end]) {
        return None;
    }
    let stop = content_end + line_break(source, content_end).unwrap_or_default();
    Some(cut_begin..stop.min(source.len()))
}

fn line_starts(source: &[u8]) -> Vec<usize> {
    let mut starts = vec![if source.starts_with(BOM) {
        BOM.len()
    } else {
        0
    }];
    let mut at = 0usize;
    while at < source.len() {
        if let Some(width) = line_break(source, at) {
            at += width;
            if at < source.len() {
                starts.push(at);
            }
            continue;
        }
        at += crate::scan::char_width(source, at);
    }
    starts
}

fn added_lines(with: &str) -> usize {
    let bytes = with.as_bytes();
    let mut count = 0usize;
    let mut at = 0usize;
    while at < bytes.len() {
        if let Some(width) = line_break(bytes, at) {
            count += 1;
            at += width;
        } else {
            at += crate::scan::char_width(bytes, at);
        }
    }
    count
}

pub fn apply(source: &[u8], mut cuts: Vec<Cut>) -> Edited {
    if cuts.is_empty() {
        return Edited {
            content: source.to_vec(),
            ..Edited::default()
        };
    }
    for cut in &mut cuts {
        if cut.lines
            && let Some(widened) = whole_lines(source, &cut.span)
        {
            cut.span = widened;
        }
    }
    cuts.sort_by(|left, right| {
        left.span
            .start
            .cmp(&right.span.start)
            .then(right.span.end.cmp(&left.span.end))
    });
    let mut kept: Vec<Cut> = Vec::with_capacity(cuts.len());
    for cut in cuts {
        if let Some(last) = kept.last()
            && cut.span.start >= last.span.start
            && cut.span.end <= last.span.end
        {
            continue;
        }
        kept.push(cut);
    }

    let mut edited = Edited::default();
    let mut gone = vec![false; source.len()];
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for cut in &kept {
        match cut.kind {
            Kind::Comment => edited.comments += 1,
            Kind::Glyph => edited.glyphs += 1,
            Kind::Doc => edited.docs += 1,
        }
        for byte in cut.span.clone() {
            gone[byte] = true;
        }
        if let Some(with) = &cut.with {
            inserts.push((cut.span.start, with.clone()));
        }
    }

    let starts = line_starts(source);
    let bounds = |index: usize| -> Range<usize> {
        let from = starts[index];
        let upto = starts.get(index + 1).copied().unwrap_or(source.len());
        from..upto
    };
    let mut erased = vec![false; starts.len()];
    let mut blank = vec![false; starts.len()];
    for index in 0..starts.len() {
        let range = bounds(index);
        erased[index] = !range.is_empty() && range.clone().all(|byte| gone[byte]);
        blank[index] = blank_bytes(&source[range]);
    }

    let mut index = 0usize;
    while index < starts.len() {
        if !erased[index] {
            index += 1;
            continue;
        }
        let mut last = index;
        while last + 1 < starts.len() && erased[last + 1] {
            last += 1;
        }
        let above = index > 0 && !erased[index - 1] && blank[index - 1];
        let after = last + 1;
        let below = after < starts.len() && !erased[after] && blank[after];
        if above && below {
            for byte in bounds(after) {
                gone[byte] = true;
            }
            erased[after] = true;
            index = after + 1;
            continue;
        }
        index = after;
    }

    for index in 0..starts.len() {
        let range = bounds(index);
        if range.is_empty() {
            continue;
        }
        let removed = range.clone().filter(|byte| gone[*byte]).count();
        if removed == 0 {
            continue;
        }
        if removed == range.len() {
            edited.minus += 1;
        } else {
            edited.minus += 1;
            edited.plus += 1;
        }
    }
    for (_, with) in &inserts {
        edited.plus += added_lines(with);
    }

    inserts.sort_by_key(|(at, _)| *at);
    let mut content = Vec::with_capacity(source.len());
    let mut pending = inserts.into_iter().peekable();
    for at in 0..source.len() {
        while pending.peek().is_some_and(|(spot, _)| *spot == at) {
            let (_, with) = pending.next().expect("peeked");
            content.extend_from_slice(with.as_bytes());
        }
        if !gone[at] {
            content.push(source[at]);
        }
    }
    for (_, with) in pending {
        content.extend_from_slice(with.as_bytes());
    }
    edited.content = content;
    edited
}
