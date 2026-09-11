use super::*;
use serde_json::json;

fn legacy() -> serde_json::Value {
    json!({
        "version": 1, "profile": "fixture", "dark": false,
        "colors": {"fg": "#202020", "red": "#aa0000", "green": "#006600", "muted": "#505050"},
        "roles": {"section_system": "#0000aa"}
    })
}

#[test]
fn existing_palette_documents_keep_legacy_roles() {
    let value = legacy();
    let palette = Palette::from_value(&value).unwrap();
    assert_eq!(palette.profile, "fixture");
    assert!(!palette.dark);
    assert_eq!(palette.color(Role::Plain), Color::Rgb(32, 32, 32));
    assert_eq!(palette.color(Role::DiffRemoved), Color::Rgb(170, 0, 0));
    assert_eq!(
        palette.named_color("roles", "section_system").unwrap(),
        Color::Rgb(0, 0, 170)
    );
    let restored = Palette::from_json(&palette.to_json().unwrap()).unwrap();
    assert_eq!(restored, palette);
}

#[test]
fn semantic_colors_override_legacy_palette_without_changing_exports() {
    let mut value = legacy();
    value["ui"] = json!({
        "foreground": "#303030", "danger": "#bb0000",
        "selection_foreground": "#ffffff", "selection_background": "#550099"
    });
    let palette = Palette::from_value(&value)
        .unwrap()
        .with_depth(ColorDepth::TrueColor);
    assert_eq!(palette.color(Role::Danger), Color::Rgb(187, 0, 0));
    assert_eq!(palette.background(Role::Selection), Color::Rgb(85, 0, 153));
    assert_eq!(
        palette.named_color("colors", "red").unwrap(),
        Color::Rgb(170, 0, 0)
    );
    assert_eq!(palette.document().colors["fg"], "#202020");
}

#[test]
fn invalid_palette_data_fails_with_the_field_name() {
    let mut value = legacy();
    value["version"] = json!(2);
    assert!(
        Palette::from_value(&value)
            .unwrap_err()
            .contains("unsupported")
    );
    value["version"] = json!(1);
    value["ui"] = json!({"accent": "\x1b[31m"});
    assert!(
        Palette::from_value(&value)
            .unwrap_err()
            .contains("ui.accent")
    );
    value["ui"] = json!({});
    value["profile"] = json!("bad\x1b[2J");
    assert!(Palette::from_value(&value).unwrap_err().contains("profile"));
    assert!(
        Palette::from_value(&json!({"version":1}))
            .unwrap_err()
            .contains("missing")
    );
}

#[test]
fn indexed_terminal_uses_prevalidated_colors_from_the_generator() {
    let mut value = legacy();
    value["ui"] = json!({"foreground":"#303030","selection_foreground":"#ffffff","selection_background":"#550099"});
    value["ui_indexed"] =
        json!({"foreground":238,"selection_foreground":231,"selection_background":55});
    let palette = Palette::from_value(&value)
        .unwrap()
        .with_depth(ColorDepth::Ansi256);
    assert_eq!(palette.foreground(Role::Plain), Color::Ansi(238));
    assert_eq!(palette.foreground(Role::Selection), Color::Ansi(231));
    assert_eq!(palette.background(Role::Selection), Color::Ansi(55));
    assert_eq!(palette.color(Role::Plain), Color::Rgb(48, 48, 48));
}

#[test]
fn fallback_preserves_terminal_foreground_and_background() {
    let palette = Palette::default();
    assert_eq!(palette.color(Role::Plain), Color::Terminal);
    assert_eq!(palette.color(Role::Background), Color::Terminal);
    assert_ne!(
        palette.color(Role::DiffAdded),
        palette.color(Role::DiffRemoved)
    );
}

#[cfg(feature = "ratatui")]
#[test]
fn monochrome_selection_remains_visible_without_color() {
    use ratatui::style::Modifier;
    let palette = Palette::default();
    let selected = palette.ratatui(crate::ColorMode::Never, true, Role::Selection);
    assert!(selected.fg.is_none());
    assert!(selected.bg.is_none());
    assert!(selected.add_modifier.contains(Modifier::REVERSED));
    assert!(selected.add_modifier.contains(Modifier::BOLD));
}
