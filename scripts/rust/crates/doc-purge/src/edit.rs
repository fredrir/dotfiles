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

pub fn whole_lines(source: &[u8], span: &Range<usize>) -> Option<Range<usize>> {
    let mut begin = span.start;
    while begin > 0 && source[begin - 1] != b'\n' {
        begin -= 1;
    }
    if source[begin..span.start]
        .iter()
        .any(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }
    let mut stop = span.end;
    while stop < source.len() && source[stop] != b'\n' {
        stop += 1;
    }
    if source[span.end..stop]
        .iter()
        .any(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }
    Some(begin..(stop + 1).min(source.len()))
}

fn line_starts(source: &[u8]) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (at, byte) in source.iter().enumerate() {
        if *byte == b'\n' && at + 1 < source.len() {
            starts.push(at + 1);
        }
    }
    starts
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
        blank[index] = source[range].iter().all(|byte| byte.is_ascii_whitespace());
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
        edited.plus += with.bytes().filter(|byte| *byte == b'\n').count();
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
