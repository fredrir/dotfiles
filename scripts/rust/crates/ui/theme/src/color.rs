use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn enabled(self, terminal: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => auto_enabled(
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorDepth {
    Ansi16,
    Ansi256,
    #[default]
    TrueColor,
}

impl ColorDepth {
    pub fn detect() -> Self {
        Self::from_signals(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    }

    pub fn from_signals(colorterm: Option<&str>, term: Option<&str>) -> Self {
        if colorterm.is_some_and(|value| {
            value.eq_ignore_ascii_case("truecolor") || value.eq_ignore_ascii_case("24bit")
        }) || term.is_some_and(|value| value.contains("direct") || value.contains("truecolor"))
        {
            Self::TrueColor
        } else if term.is_some_and(|value| value.contains("256color")) {
            Self::Ansi256
        } else if term.is_some_and(|value| !value.is_empty()) {
            Self::Ansi16
        } else {
            Self::TrueColor
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Terminal,
    Ansi(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    pub fn parse(value: &str) -> Result<Self, String> {
        let Some(hex) = value.strip_prefix('#') else {
            return Err(format!("invalid palette color: {value}"));
        };
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("invalid palette color: {value}"));
        }
        let number =
            u32::from_str_radix(hex, 16).map_err(|_| format!("invalid palette color: {value}"))?;
        Ok(Self::Rgb(
            (number >> 16) as u8,
            (number >> 8) as u8,
            number as u8,
        ))
    }

    pub fn at_depth(self, depth: ColorDepth) -> Self {
        let Self::Rgb(red, green, blue) = self else {
            return self;
        };
        match depth {
            ColorDepth::TrueColor => self,
            ColorDepth::Ansi16 => Self::Ansi(nearest([red, green, blue], 0..16)),
            ColorDepth::Ansi256 => Self::Ansi(nearest([red, green, blue], 16..256)),
        }
    }

    pub fn rgb(self) -> Option<[u8; 3]> {
        match self {
            Self::Terminal => None,
            Self::Ansi(index) => Some(indexed(index)),
            Self::Rgb(red, green, blue) => Some([red, green, blue]),
        }
    }

    pub fn sgr(self, background: bool) -> String {
        let group = if background { 48 } else { 38 };
        match self {
            Self::Terminal => if background { "49" } else { "39" }.into(),
            Self::Ansi(index) if index < 8 => {
                (u16::from(index) + if background { 40 } else { 30 }).to_string()
            }
            Self::Ansi(index) if index < 16 => {
                (u16::from(index) - 8 + if background { 100 } else { 90 }).to_string()
            }
            Self::Ansi(index) => format!("{group};5;{index}"),
            Self::Rgb(red, green, blue) => format!("{group};2;{red};{green};{blue}"),
        }
    }

    #[cfg(feature = "ratatui")]
    pub fn ratatui(self) -> ratatui::style::Color {
        use ratatui::style::Color as R;
        match self {
            Self::Terminal => R::Reset,
            Self::Ansi(index) if index < 16 => [
                R::Black,
                R::Red,
                R::Green,
                R::Yellow,
                R::Blue,
                R::Magenta,
                R::Cyan,
                R::Gray,
                R::DarkGray,
                R::LightRed,
                R::LightGreen,
                R::LightYellow,
                R::LightBlue,
                R::LightMagenta,
                R::LightCyan,
                R::White,
            ][usize::from(index)],
            Self::Ansi(index) => R::Indexed(index),
            Self::Rgb(red, green, blue) => R::Rgb(red, green, blue),
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.rgb() {
            Some([red, green, blue]) => write!(output, "#{red:02x}{green:02x}{blue:02x}"),
            None => output.write_str("default"),
        }
    }
}

fn nearest(color: [u8; 3], range: std::ops::Range<u16>) -> u8 {
    range
        .min_by_key(|index| {
            indexed(*index as u8)
                .into_iter()
                .zip(color)
                .map(|(left, right)| (i32::from(left) - i32::from(right)).pow(2))
                .sum::<i32>()
        })
        .unwrap_or(0) as u8
}

fn indexed(index: u8) -> [u8; 3] {
    const ANSI: [[u8; 3]; 16] = [
        [0, 0, 0],
        [128, 0, 0],
        [0, 128, 0],
        [128, 128, 0],
        [0, 0, 128],
        [128, 0, 128],
        [0, 128, 128],
        [192, 192, 192],
        [128, 128, 128],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [0, 0, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    if index < 16 {
        ANSI[usize::from(index)]
    } else if index >= 232 {
        [8 + 10 * (index - 232); 3]
    } else {
        let ramp = [0, 95, 135, 175, 215, 255];
        let index = usize::from(index - 16);
        [ramp[index / 36], ramp[index / 6 % 6], ramp[index % 6]]
    }
}

#[cfg(test)]
#[path = "../tests/unit/color.rs"]
mod tests;
