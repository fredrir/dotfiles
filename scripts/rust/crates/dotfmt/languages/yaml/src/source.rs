pub(crate) struct Source<'a> {
    pub text: &'a str,
    offsets: Vec<usize>,
    starts: Vec<usize>,
}

impl<'a> Source<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut offsets: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
        offsets.push(text.len());
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .filter(|start| *start < text.len())
            .collect();
        Self {
            text,
            offsets,
            starts,
        }
    }

    pub fn byte(&self, index: usize) -> usize {
        self.offsets[index.min(self.offsets.len() - 1)]
    }

    pub fn lines(&self) -> usize {
        self.starts.len()
    }

    pub fn line(&self, at: usize) -> usize {
        self.starts
            .partition_point(|start| *start <= at)
            .saturating_sub(1)
    }

    pub fn start(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(self.text.len())
    }

    pub fn end(&self, line: usize) -> usize {
        let start = self.start(line);
        self.text[start..]
            .find('\n')
            .map_or(self.text.len(), |length| start + length)
    }

    pub fn line_text(&self, line: usize) -> &'a str {
        &self.text[self.start(line)..self.end(line)]
    }

    pub fn column(&self, at: usize) -> usize {
        self.text[self.start(self.line(at))..at].chars().count()
    }

    pub fn indent(&self, line: usize) -> usize {
        self.line_text(line)
            .bytes()
            .take_while(|byte| *byte == b' ')
            .count()
    }

    pub fn blank(&self, line: usize) -> bool {
        self.line_text(line).trim_matches([' ', '\t']).is_empty()
    }
}
