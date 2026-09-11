use super::*;

#[test]
fn colour_keeps_the_foreground() {
    let style = ui_style(true, Color::Rgb(51, 65, 85), Modifier::empty());
    assert_eq!(style.fg, Some(Color::Rgb(51, 65, 85)));
    assert_eq!(style.add_modifier, Modifier::empty());
}

#[test]
fn no_colour_drops_the_foreground() {
    let style = ui_style(false, Color::Rgb(51, 65, 85), Modifier::empty());
    assert_eq!(style.fg, None);
}

#[test]
fn the_modifier_survives_either_way() {
    assert!(
        ui_style(true, Color::Reset, Modifier::BOLD)
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        ui_style(false, Color::Reset, Modifier::BOLD)
            .add_modifier
            .contains(Modifier::BOLD)
    );
}
