use std::time::Duration;

use super::*;

#[derive(Clone, Copy, Debug)]
pub struct FolderAnimation {
    pub row: usize,
    pub elapsed: Duration,
}

impl FolderAnimation {
    pub const DURATION: Duration = Duration::from_millis(160);

    fn frame(self, expanded: bool) -> (bool, f64) {
        let progress = (self.elapsed.as_secs_f64() / Self::DURATION.as_secs_f64()).min(1.0);
        let visibility = (2.0 * progress - 1.0).abs();
        let visibility = visibility * visibility * (3.0 - 2.0 * visibility);
        (
            if progress < 0.5 { !expanded } else { expanded },
            visibility,
        )
    }
}

pub(super) fn appearance(
    expanded: bool,
    animation: Option<FolderAnimation>,
    appearance: Style,
) -> (&'static str, Style) {
    let (expanded, visibility) =
        animation.map_or((expanded, 1.0), |animation| animation.frame(expanded));
    let icon = if visibility < 0.03 {
        " "
    } else if expanded {
        "\u{f07c}"
    } else {
        "▸"
    };
    if visibility >= 1.0 {
        return (icon, appearance);
    }
    if color_depth() == ColorDepth::Ansi16 {
        return (
            icon,
            if visibility < 0.6 {
                appearance.add_modifier(Modifier::DIM)
            } else {
                appearance
            },
        );
    }
    let background = match appearance.bg {
        Some(Color::Rgb(r, g, b)) => Some([r, g, b]),
        Some(Color::Indexed(index)) => ui_theme::Color::Ansi(index).rgb(),
        _ => Palette::current().background(Role::Background).rgb(),
    }
    .unwrap_or([0; 3]);
    let source = match appearance.fg {
        Some(Color::Rgb(r, g, b)) => Some([r, g, b]),
        Some(Color::Indexed(index)) => ui_theme::Color::Ansi(index).rgb(),
        _ => palette().color(Role::Plain).rgb(),
    }
    .unwrap_or([192; 3]);
    let blended: [u8; 3] = std::array::from_fn(|index| {
        (f64::from(background[index])
            + (f64::from(source[index]) - f64::from(background[index])) * visibility)
            .round() as u8
    });
    let color = ui_theme::Color::Rgb(blended[0], blended[1], blended[2])
        .at_depth(color_depth())
        .ratatui();
    (icon, appearance.fg(color))
}
