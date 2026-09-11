use ratatui::style::{Color, Modifier, Style};

pub fn ui_style(color: bool, foreground: Color, modifier: Modifier) -> Style {
    let style = if color {
        Style::default().fg(foreground)
    } else {
        Style::default()
    };
    style.add_modifier(modifier)
}

#[cfg(test)]
#[path = "../tests/unit/style_tests.rs"]
mod tests;
