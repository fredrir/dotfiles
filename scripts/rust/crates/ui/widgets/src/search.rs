#[derive(Clone, Debug, Default)]
pub struct SearchText {
    normalized: String,
}

impl SearchText {
    pub fn new(text: &str) -> Self {
        let normalized = text.to_lowercase();
        Self { normalized }
    }
    pub fn normalized(&self) -> &str {
        &self.normalized
    }
    pub fn ascii(text: &str) -> Self {
        let normalized = text.to_ascii_lowercase();
        Self { normalized }
    }
    pub fn score_tokens(&self, tokens: &[String]) -> Option<i64> {
        let mut total = 0;
        for token in tokens {
            let mut characters = self.normalized.char_indices().enumerate();
            let mut previous = None;
            let mut first = None;
            let mut contiguous = 0;
            for needle in token.chars() {
                let (position, (byte, _)) =
                    characters.find(|(_, (_, candidate))| *candidate == needle)?;
                first.get_or_insert(byte);
                if previous.is_some_and(|last| last + 1 == position) {
                    contiguous += 8;
                }
                previous = Some(position);
            }
            total += 1000 + contiguous - i64::try_from(first.unwrap_or(0)).unwrap_or(i64::MAX) / 4;
        }
        Some(total)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchMode {
    #[default]
    Contains,
    Fuzzy,
}

#[derive(Clone, Debug, Default)]
pub struct SearchIndex {
    entries: Vec<SearchText>,
}

impl SearchIndex {
    pub fn new_ascii<T: AsRef<str>>(text: impl IntoIterator<Item = T>) -> Self {
        Self {
            entries: text
                .into_iter()
                .map(|text| SearchText::ascii(text.as_ref()))
                .collect(),
        }
    }
    pub fn replace_ascii(&mut self, index: usize, text: &str) {
        if let Some(entry) = self.entries.get_mut(index) {
            *entry = SearchText::ascii(text);
        }
    }
    pub fn score_tokens(&self, index: usize, tokens: &[String]) -> Option<i64> {
        self.entries.get(index)?.score_tokens(tokens)
    }
    pub fn new<T: AsRef<str>>(text: impl IntoIterator<Item = T>) -> Self {
        Self {
            entries: text
                .into_iter()
                .map(|text| SearchText::new(text.as_ref()))
                .collect(),
        }
    }
    pub fn entry(&self, index: usize) -> Option<&SearchText> {
        self.entries.get(index)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn filter(&self, query: &str) -> Vec<usize> {
        self.search(query, MatchMode::Contains)
    }
    pub fn filter_into(&self, query: &str, rows: &mut Vec<usize>) {
        let query = query.to_lowercase();
        rows.clear();
        rows.extend(
            self.entries
                .iter()
                .enumerate()
                .filter_map(|(index, text)| text.normalized.contains(&query).then_some(index)),
        );
    }
    pub fn search(&self, query: &str, mode: MatchMode) -> Vec<usize> {
        let query = query.to_lowercase();
        if query.is_empty() {
            return (0..self.entries.len()).collect();
        }
        if mode == MatchMode::Contains {
            return self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(index, text)| text.normalized.contains(&query).then_some(index))
                .collect();
        }
        let tokens: Vec<String> = query.split_whitespace().map(str::to_owned).collect();
        let mut matches = Vec::new();
        for (index, text) in self.entries.iter().enumerate() {
            if let Some(score) = text.score_tokens(&tokens) {
                matches.push((std::cmp::Reverse(score), index));
            }
        }
        matches.sort_unstable();
        matches.into_iter().map(|(_, index)| index).collect()
    }
}
