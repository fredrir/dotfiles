use super::*;

#[test]
fn favorites_round_trip_in_stable_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("favorites.json");
    let mut favorites = Favorites::load_from(path.clone());
    favorites.set("macie:codex:z", true).unwrap();
    favorites.set("archie:claude:a", true).unwrap();

    let loaded = Favorites::load_from(path);
    assert!(loaded.contains("macie:codex:z"));
    assert!(loaded.contains("archie:claude:a"));
    assert!(loaded.warning().is_none());
}

#[test]
fn removing_a_favorite_is_persistent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("favorites.json");
    let mut favorites = Favorites::load_from(path.clone());
    favorites.set("macie:codex:id", true).unwrap();
    favorites.set("macie:codex:id", false).unwrap();
    assert!(!Favorites::load_from(path).contains("macie:codex:id"));
}

#[test]
fn an_invalid_file_becomes_a_warning_instead_of_blocking_the_picker() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("favorites.json");
    fs::write(&path, "not json").unwrap();
    let favorites = Favorites::load_from(path);
    assert!(
        favorites
            .warning()
            .unwrap()
            .contains("could not read favorites")
    );
}

#[test]
fn control_characters_are_never_persisted() {
    let directory = tempfile::tempdir().unwrap();
    let mut favorites = Favorites::load_from(directory.path().join("favorites.json"));
    assert!(favorites.set("bad\u{1b}key", true).is_err());
}

#[test]
fn failed_saves_roll_back_in_memory_state() {
    let directory = tempfile::tempdir().unwrap();
    let blocked = directory.path().join("blocked");
    fs::write(&blocked, "not a directory").unwrap();
    let mut favorites = Favorites::load_from(blocked.join("favorites.json"));
    assert!(favorites.set("macie:codex:id", true).is_err());
    assert!(!favorites.contains("macie:codex:id"));
}
