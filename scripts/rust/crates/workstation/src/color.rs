use clap::ValueEnum;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn enabled(self, terminal: bool) -> bool {
        match self {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => auto_enabled(
                terminal,
                std::env::var_os("NO_COLOR").is_some(),
                std::env::var("CLICOLOR").ok().as_deref(),
                std::env::var("TERM").ok().as_deref(),
            ),
        }
    }
}

pub fn auto_enabled(
    terminal: bool,
    no_color: bool,
    clicolor: Option<&str>,
    term: Option<&str>,
) -> bool {
    terminal
        && !no_color
        && clicolor != Some("0")
        && term.is_none_or(|value| !value.eq_ignore_ascii_case("dumb"))
}

#[cfg(test)]
#[path = "../tests/unit/color_tests.rs"]
mod tests;
