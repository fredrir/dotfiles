//! jq's numbers.
//!
//! jq reads a literal with decNumber and writes it back with that library's
//! `to-scientific-string`, which is neither the literal nor a float: `1e2` is
//! `1E+2`, `10e-7` is `0.0000010`, `1.10` keeps its trailing zero, and a forty
//! digit integer keeps every digit. Reformatting a repository with any other
//! answer rewrites numbers nobody asked about, so the rule is reproduced here.

/// jq's grammar is JSON's with the shape of a number relaxed: `+1`, `.5` and
/// `01` all read, and nothing else does. Answers with the literal as jq would
/// print it, or `None` when the text is not a number at all.
pub fn canonical(literal: &str) -> Option<String> {
    let digits = Digits::read(literal)?;
    Some(digits.write())
}

struct Digits {
    negative: bool,
    /// Coefficient digits, leading zeros dropped, never empty.
    digits: String,
    /// Power of ten the coefficient is scaled by, once the fraction is folded
    /// into it.
    exponent: i64,
}

impl Digits {
    fn read(literal: &str) -> Option<Digits> {
        let bytes = literal.as_bytes();
        let mut at = 0;
        let negative = match bytes.first()? {
            b'-' => {
                at += 1;
                true
            }
            b'+' => {
                at += 1;
                false
            }
            _ => false,
        };

        let whole = digits_at(literal, &mut at);
        let fraction = match bytes.get(at) {
            // `1.` is a number jq reads, and writes back as `1`.
            Some(b'.') => {
                at += 1;
                digits_at(literal, &mut at)
            }
            _ => String::new(),
        };
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }

        let mut power = 0;
        if matches!(bytes.get(at), Some(b'e' | b'E')) {
            at += 1;
            let mut behind = false;
            if matches!(bytes.get(at), Some(b'+' | b'-')) {
                behind = bytes[at] == b'-';
                at += 1;
            }
            let exponent = digits_at(literal, &mut at);
            if exponent.is_empty() {
                return None;
            }
            power = if behind {
                -power_of(&exponent)
            } else {
                power_of(&exponent)
            };
        }
        if at != bytes.len() {
            return None;
        }

        let mut digits = String::with_capacity(whole.len() + fraction.len());
        digits.push_str(&whole);
        digits.push_str(&fraction);
        let coefficient = digits.trim_start_matches('0');
        digits = if coefficient.is_empty() {
            // Zero carries no coefficient, but its exponent still places the
            // point: `0.000` is `0.000` and `0e5` is `0E+5`.
            "0".to_string()
        } else {
            coefficient.to_string()
        };

        Some(Digits {
            negative,
            digits,
            exponent: power.saturating_sub(fraction.len() as i64),
        })
    }

    /// decNumber's `to-scientific-string`: plain notation while the point lands
    /// within six places of the coefficient, scientific otherwise.
    fn write(&self) -> String {
        let sign = if self.negative { "-" } else { "" };
        let places = self.digits.len() as i64;
        let point = places + self.exponent;
        // The exponent of the leading digit is what decNumber measures, which
        // is why `1e-7` is scientific and `10e-7` — the same point, one more
        // digit — is not.
        let adjusted = point - 1;
        if self.exponent <= 0 && adjusted >= -6 {
            if self.exponent == 0 {
                return format!("{sign}{}", self.digits);
            }
            if point > 0 {
                let (whole, fraction) = self.digits.split_at(point as usize);
                return format!("{sign}{whole}.{fraction}");
            }
            let zeros = "0".repeat((-point) as usize);
            return format!("{sign}0.{zeros}{}", self.digits);
        }

        let (lead, rest) = self.digits.split_at(1);
        let coefficient = if rest.is_empty() {
            lead.to_string()
        } else {
            format!("{lead}.{rest}")
        };
        let marker = if adjusted < 0 { '-' } else { '+' };
        format!("{sign}{coefficient}E{marker}{}", adjusted.abs())
    }
}

fn digits_at(literal: &str, at: &mut usize) -> String {
    let bytes = literal.as_bytes();
    let start = *at;
    while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
        *at += 1;
    }
    literal[start..*at].to_string()
}

/// The exponent is read into an i64 whatever it takes: `1e-1000000` is a number
/// jq answers, and there is nothing below it worth refusing a file over.
fn power_of(digits: &str) -> i64 {
    let mut value: i64 = 0;
    for byte in digits.bytes() {
        value = value
            .saturating_mul(10)
            .saturating_add(i64::from(byte - b'0'));
    }
    value
}
