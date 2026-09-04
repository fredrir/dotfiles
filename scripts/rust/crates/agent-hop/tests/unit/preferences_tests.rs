use super::*;

#[test]
fn picker_view_round_trips() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("view.json");
    let view = PickerView {
        origin: OriginFilter::Remote,
        agent: AgentFilter::Claude,
        scope: ScopeFilter::Favorites,
        preview: PreviewDensity::Compact,
    };
    Preferences::load_from(path.clone()).save(view).unwrap();
    let loaded = Preferences::load_from(path);
    assert_eq!(loaded.view(), view);
    assert!(loaded.warning().is_none());
}

#[test]
fn malformed_preferences_fall_back_with_a_warning() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("view.json");
    fs::write(&path, r#"{"version":1,"origin":"somewhere"}"#).unwrap();
    let loaded = Preferences::load_from(path);
    assert_eq!(loaded.view(), PickerView::default());
    assert!(loaded.warning().unwrap().contains("could not read"));
}
