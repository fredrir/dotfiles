use std::fmt;

use vte::Params;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Foreground {
    #[default]
    Default,
    Basic(u16),
    Indexed(u16),
    Rgb(u16, u16, u16),
}

impl Foreground {
    // None when the SGR sequence leaves the foreground as it was.
    pub fn set_by(sgr: &Params) -> Option<Self> {
        if sgr.is_empty() {
            return Some(Self::Default);
        }
        let mut foreground = None;
        let mut params = sgr.iter();
        while let Some(param) = params.next() {
            foreground = match param {
                [0] | [39] => Some(Self::Default),
                [code @ (30..=37 | 90..=97)] => Some(Self::Basic(*code)),
                [38, color @ ..] => extended(color, &mut params).or(foreground),
                [48 | 58, color @ ..] => {
                    extended(color, &mut params);
                    foreground
                }
                _ => foreground,
            };
        }
        foreground
    }
}

// The color after 38, 48 or 58, in either `38;2;r;g;b` or `38:2::r:g:b` form.
fn extended<'a>(
    colon: &[u16],
    semicolon: &mut impl Iterator<Item = &'a [u16]>,
) -> Option<Foreground> {
    match colon {
        [5, index] => Some(Foreground::Indexed(*index)),
        [2, r, g, b] | [2, _, r, g, b] => Some(Foreground::Rgb(*r, *g, *b)),
        [] => {
            let mut next = || semicolon.next()?.first().copied();
            match next()? {
                5 => Some(Foreground::Indexed(next()?)),
                2 => Some(Foreground::Rgb(next()?, next()?, next()?)),
                _ => None,
            }
        }
        _ => None,
    }
}

impl fmt::Display for Foreground {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => write!(formatter, "\x1b[39m"),
            Self::Basic(code) => write!(formatter, "\x1b[{code}m"),
            Self::Indexed(index) => write!(formatter, "\x1b[38;5;{index}m"),
            Self::Rgb(r, g, b) => write!(formatter, "\x1b[38;2;{r};{g};{b}m"),
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/foreground_tests.rs"]
mod tests;
