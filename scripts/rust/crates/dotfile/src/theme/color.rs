use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Color(pub [u8; 3]);

impl Color {
    pub fn parse(value: &str) -> Result<Self, String> {
        let digits = value.strip_prefix('#').unwrap_or(value);
        if digits.len() != 6 || !digits.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("invalid color: {value}"));
        }
        let mut channels = [0; 3];
        for (i, channel) in channels.iter_mut().enumerate() {
            *channel =
                u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
        }
        Ok(Self(channels))
    }
    pub fn csv(self) -> String {
        let [r, g, b] = self.0;
        format!("{r},{g},{b}")
    }
    pub fn ansi(self) -> String {
        let [r, g, b] = self.0;
        format!("38;2;{r};{g};{b}")
    }
    pub fn luminance(self) -> f64 {
        let [r, g, b] = self.0.map(|x| linear(f64::from(x) / 255.));
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }
    pub fn lab(self) -> [f64; 3] {
        let [r, g, b] = self.0.map(|x| linear(f64::from(x) / 255.));
        // Match Python's signed power; cbrt's last-bit differences affect gamut boundaries.
        let root = |v: f64| v.abs().powf(1. / 3.).copysign(v);
        let l = root(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
        let m = root(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
        let s = root(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
        [
            0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
            1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
            0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
        ]
    }
    pub fn from_lab([l, a, b]: [f64; 3]) -> Self {
        let mut rgb = srgb(l, a, b);
        if !in_gamut(rgb) {
            let (mut low, mut high) = (0., 1.);
            for _ in 0..16 {
                let scale = (low + high) / 2.;
                if in_gamut(srgb(l, a * scale, b * scale)) {
                    low = scale;
                } else {
                    high = scale;
                }
            }
            rgb = srgb(l, a * low, b * low);
        }
        Self(rgb.map(|v| (v.clamp(0., 1.) * 255.).round_ties_even() as u8))
    }
    pub fn mix(self, other: Self, amount: f64) -> Self {
        let a = self.lab();
        let b = other.lab();
        Self::from_lab(std::array::from_fn(|i| {
            a[i] * (1. - amount) + b[i] * amount
        }))
    }
    pub fn contrast(self, other: Self) -> f64 {
        let a = self.luminance();
        let b = other.luminance();
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    pub fn on(self, foreground: Self, background: Self, floor: f64) -> Result<Self, String> {
        for c in [foreground, background] {
            if c.contrast(self) >= floor {
                return Ok(c);
            }
        }
        let c = if Self::BLACK.contrast(self) >= Self::WHITE.contrast(self) {
            Self::BLACK
        } else {
            Self::WHITE
        };
        if c.contrast(self) < floor {
            Err(format!("cannot reach {floor}:1 on {self}"))
        } else {
            Ok(c)
        }
    }
    pub const BLACK: Self = Self([0, 0, 0]);
    pub const WHITE: Self = Self([255, 255, 255]);
    pub fn readable(self, background: Self, floor: f64) -> Result<Self, String> {
        if self.contrast(background) >= floor {
            return Ok(self);
        }
        let [l, a, b] = self.lab();
        let candidate = |light| Self::from_lab([light, a, b]);
        let mut choices = Vec::new();
        if candidate(0.).contrast(background) >= floor {
            let (mut low, mut high) = (0., l);
            for _ in 0..24 {
                let mid = (low + high) / 2.;
                if candidate(mid).contrast(background) >= floor {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            let c = candidate(low);
            choices.push(((c.lab()[0] - l).abs(), c));
        }
        if candidate(1.).contrast(background) >= floor {
            let (mut low, mut high) = (l, 1.);
            for _ in 0..24 {
                let mid = (low + high) / 2.;
                if candidate(mid).contrast(background) >= floor {
                    high = mid;
                } else {
                    low = mid;
                }
            }
            let c = candidate(high);
            choices.push(((c.lab()[0] - l).abs(), c));
        }
        choices.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        if let Some((_, c)) = choices.first() {
            return Ok(*c);
        }
        background.on(Self::BLACK, Self::WHITE, floor)
    }
    pub fn readable_many(self, backgrounds: &[Self], floor: f64) -> Result<Self, String> {
        let valid = |c: Self| backgrounds.iter().all(|b| c.contrast(*b) >= floor);
        if valid(self) {
            return Ok(self);
        }
        let [l, a, b] = self.lab();
        let mut best: Option<(f64, Self)> = None;
        for step in 0..=1000 {
            let c = Self::from_lab([f64::from(step) / 1000., a, b]);
            if valid(c) {
                let next = ((c.lab()[0] - l).abs(), c);
                if best.is_none_or(|old| next < old) {
                    best = Some(next);
                }
            }
        }
        if let Some((_, c)) = best {
            return Ok(c);
        }
        [Self::BLACK, Self::WHITE]
            .into_iter()
            .filter(|c| valid(*c))
            .max_by(|a, b| {
                let minimum = |c: Self| {
                    backgrounds
                        .iter()
                        .map(|bg| c.contrast(*bg))
                        .fold(f64::INFINITY, f64::min)
                };
                minimum(*a).total_cmp(&minimum(*b))
            })
            .ok_or_else(|| {
                format!(
                    "cannot reach {floor}:1 against {}",
                    backgrounds
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }
}
impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b] = self.0;
        write!(f, "#{r:02x}{g:02x}{b:02x}")
    }
}
fn linear(x: f64) -> f64 {
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}
fn nonlinear(x: f64) -> f64 {
    if x <= 0.0031308 {
        x * 12.92
    } else {
        1.055 * x.powf(1. / 2.4) - 0.055
    }
}
fn srgb(l: f64, a: f64, b: f64) -> [f64; 3] {
    let ll = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        nonlinear(4.0767416621 * ll - 3.3077115913 * m + 0.2309699292 * s),
        nonlinear(-1.2684380046 * ll + 2.6097574011 * m - 0.3413193965 * s),
        nonlinear(-0.0041960863 * ll - 0.7034186147 * m + 1.7076147010 * s),
    ]
}
fn in_gamut(rgb: [f64; 3]) -> bool {
    rgb.into_iter().all(|v| (-1e-6..=1. + 1e-6).contains(&v))
}
