use ui_widgets::{Line, MatchMode, Navigation, PromptBuffer, Role, SearchIndex, Viewport};

#[test]
fn filtering_and_fuzzy_matching_keep_deterministic_item_identity() {
    let index = SearchIndex::new(["Alpha Beta", "Alphabet", "日本"]);
    assert_eq!(index.filter("BETA"), [0]);
    assert_eq!(index.search("abt", MatchMode::Fuzzy), [0, 1]);
    assert_eq!(index.filter("日"), [2]);
}

#[test]
fn viewport_handles_wrap_resize_and_empty_results() {
    let mut viewport = Viewport::default();
    viewport.move_by(-1, 10, Navigation::Wrap);
    viewport.settle(10, 3);
    assert_eq!(viewport.visible(10, 3), 7..10);
    viewport.settle(0, 0);
    assert_eq!(viewport, Viewport::default());
}

#[test]
fn semantic_text_fits_cells_and_prompt_edits_paths() {
    let line = Line::styled("東京abc", Role::Accent).fitted(5);
    assert_eq!(line.width(), 5);
    let mut prompt = PromptBuffer::new("/one/two ");
    prompt.word_back();
    assert_eq!(prompt.as_str(), "/one");
    prompt.word_back();
    assert_eq!(prompt.as_str(), "/");
}
