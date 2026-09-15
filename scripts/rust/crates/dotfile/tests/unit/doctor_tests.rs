use super::*;

#[test]
fn font_family_and_weights_are_distinguished() {
    let fonts = [
        font_key("FiraCode Nerd Font Regular"),
        font_key("FiraCode ExtraBold Italic"),
    ]
    .into_iter()
    .collect();
    assert!(!font_missing("FiraCode", &fonts));
    assert!(font_missing("Fira", &fonts));
}

#[test]
fn brew_inventory_matches_versioned_formulae() {
    let installed = ["python@3.14", "rsync"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(inventory_has(&installed, "python", Manager::Brew));
    assert!(inventory_has(&installed, "rsync", Manager::Brew));
    assert!(!inventory_has(&installed, "py", Manager::Brew));
    assert!(!inventory_has(&installed, "python", Manager::Pacman));
}
