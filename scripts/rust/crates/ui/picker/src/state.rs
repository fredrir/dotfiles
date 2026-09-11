use ui_widgets::{MatchMode, Navigation, SearchIndex, Viewport};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Single,
    Multiple,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item<K> {
    pub id: K,
    pub label: String,
    pub detail: String,
    pub disabled: bool,
}

impl<K> Item<K> {
    pub fn new(id: K, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            detail: String::new(),
            disabled: false,
        }
    }
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

pub struct SelectionState<K> {
    items: Vec<Item<K>>,
    search: SearchIndex,
    rows: Vec<usize>,
    checked: Vec<bool>,
    selected_count: usize,
    focus_anchor: Option<K>,
    query: String,
    mode: Mode,
    pub viewport: Viewport,
    pub navigation: Navigation,
    pub matching: MatchMode,
}

impl<K: Clone + Eq> SelectionState<K> {
    pub fn new(items: Vec<Item<K>>, mode: Mode) -> Self {
        let search = SearchIndex::new(
            items
                .iter()
                .map(|item| format!("{} {}", item.label, item.detail)),
        );
        let rows = (0..items.len()).collect();
        let checked = vec![false; items.len()];
        let focus_anchor = items.first().map(|item| item.id.clone());
        Self {
            items,
            search,
            rows,
            checked,
            selected_count: 0,
            focus_anchor,
            query: String::new(),
            mode,
            viewport: Viewport::default(),
            navigation: Navigation::Clamp,
            matching: MatchMode::Contains,
        }
    }
    pub fn items(&self) -> &[Item<K>] {
        &self.items
    }
    pub fn rows(&self) -> &[usize] {
        &self.rows
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn focused(&self) -> Option<&Item<K>> {
        self.rows
            .get(self.viewport.cursor)
            .and_then(|index| self.items.get(*index))
    }
    pub fn checked(&self, index: usize) -> bool {
        self.checked.get(index).copied().unwrap_or(false)
    }
    pub fn selected_count(&self) -> usize {
        self.selected_count
    }
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        if mode == Mode::Single {
            self.checked.fill(false);
            self.selected_count = 0;
        }
    }
    pub fn set_query(&mut self, query: &str) {
        if self.query == query {
            return;
        }
        let focused = self.focus_anchor.clone();
        self.query = query.to_owned();
        self.rows = self.search.search(query, self.matching);
        self.viewport.cursor = focused
            .and_then(|id| {
                self.rows
                    .iter()
                    .position(|index| self.items[*index].id == id)
            })
            .unwrap_or(0);
        self.viewport.offset = 0;
    }
    pub fn move_by(&mut self, amount: isize) {
        self.viewport
            .move_by(amount, self.rows.len(), self.navigation);
        if let Some(item) = self.focused() {
            self.focus_anchor = Some(item.id.clone());
        }
    }
    pub fn first(&mut self) {
        self.viewport.cursor = 0;
        if let Some(item) = self.focused() {
            self.focus_anchor = Some(item.id.clone());
        }
    }
    pub fn last(&mut self) {
        self.viewport.cursor = self.rows.len().saturating_sub(1);
        if let Some(item) = self.focused() {
            self.focus_anchor = Some(item.id.clone());
        }
    }
    pub fn focus(&mut self, id: &K) -> bool {
        if let Some(cursor) = self
            .rows
            .iter()
            .position(|index| &self.items[*index].id == id)
        {
            self.viewport.cursor = cursor;
            self.focus_anchor = Some(id.clone());
            true
        } else {
            false
        }
    }
    pub fn toggle(&mut self) {
        if self.mode != Mode::Multiple {
            return;
        }
        if let Some(index) = self
            .rows
            .get(self.viewport.cursor)
            .copied()
            .filter(|index| !self.items[*index].disabled)
        {
            self.checked[index] = !self.checked[index];
            if self.checked[index] {
                self.selected_count += 1;
            } else {
                self.selected_count -= 1;
            }
        }
    }
    pub fn toggle_visible(&mut self) {
        if self.mode != Mode::Multiple {
            return;
        }
        let selected = !self
            .rows
            .iter()
            .filter(|index| !self.items[**index].disabled)
            .all(|index| self.checked[*index]);
        for index in &self.rows {
            if !self.items[*index].disabled && self.checked[*index] != selected {
                self.checked[*index] = selected;
                if selected {
                    self.selected_count += 1;
                } else {
                    self.selected_count -= 1;
                }
            }
        }
    }
    pub fn selected(&self) -> Vec<K> {
        let marked: Vec<_> = self
            .items
            .iter()
            .zip(&self.checked)
            .filter(|(_, checked)| **checked)
            .map(|(item, _)| item.id.clone())
            .collect();
        if self.mode == Mode::Multiple && !marked.is_empty() {
            return marked;
        }
        self.focused()
            .filter(|item| !item.disabled)
            .map(|item| vec![item.id.clone()])
            .unwrap_or_default()
    }
}
