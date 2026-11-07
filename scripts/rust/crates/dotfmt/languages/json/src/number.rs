
pub fn canonical(literal: &str) -> Option<String> {
    let digits = Digits::read(literal)?;
    match digits.write() {
        Some(written) => Some(written),
        // An exponent too large to place on a point is kept as it was written:
        // guessing at it would change the value and break the promise that
        // laying the text out again leaves it alone.
        None if strict(literal.as_bytes()) => Some(literal.to_string()),
        None => None,
    }
}

struct Digits {
    negative: bool,
    digits: String,
    /// `None` when the literal named an exponent wider than this type holds.
    exponent: Option<i128>,
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
            Some(b'.') => {
                at += 1;
                digits_at(literal, &mut at)
            }
            _ => String::new(),
        };
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }

        let mut power = Some(0);
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
            power = power_of(&exponent).map(|value| if behind { -value } else { value });
        }
        if at != bytes.len() {
            return None;
        }

        let mut digits = String::with_capacity(whole.len() + fraction.len());
        digits.push_str(&whole);
        digits.push_str(&fraction);
        let coefficient = digits.trim_start_matches('0');
        digits = if coefficient.is_empty() {
            "0".to_string()
        } else {
            coefficient.to_string()
        };

        Some(Digits {
            negative,
            digits,
            exponent: power.and_then(|power| power.checked_sub(fraction.len() as i128)),
        })
    }

    fn write(&self) -> Option<String> {
        let exponent = self.exponent?;
        let sign = if self.negative { "-" } else { "" };
        // Widened on purpose: the exponent is whatever the literal named, all
        // the way past `i64::MAX`, and `places + exponent` must not overflow.
        let places = self.digits.len() as i128;
        let point = places.checked_add(exponent)?;
        let adjusted = point.checked_sub(1)?;
        if exponent <= 0 && adjusted >= -6 {
            if exponent == 0 {
                return Some(format!("{sign}{}", self.digits));
            }
            if point > 0 {
                let (whole, fraction) = self.digits.split_at(point as usize);
                return Some(format!("{sign}{whole}.{fraction}"));
            }
            let zeros = "0".repeat((-point) as usize);
            return Some(format!("{sign}0.{zeros}{}", self.digits));
        }

        let (lead, rest) = self.digits.split_at(1);
        let coefficient = if rest.is_empty() {
            lead.to_string()
        } else {
            format!("{lead}.{rest}")
        };
        let marker = if adjusted < 0 { '-' } else { '+' };
        Some(format!("{sign}{coefficient}E{marker}{}", adjusted.abs()))
    }
}

pub fn strict(mut bytes: &[u8]) -> bool {
    if bytes.first() == Some(&b'-') {
        bytes = &bytes[1..];
    }
    match bytes.first() {
        Some(b'0') => bytes = &bytes[1..],
        Some(b'1'..=b'9') => {
            bytes = &bytes[bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count()..];
        }
        _ => return false,
    }
    if bytes.first() == Some(&b'.') {
        bytes = &bytes[1..];
        let count = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if count == 0 {
            return false;
        }
        bytes = &bytes[count..];
    }
    if matches!(bytes.first(), Some(b'e' | b'E')) {
        bytes = &bytes[1..];
        if matches!(bytes.first(), Some(b'+' | b'-')) {
            bytes = &bytes[1..];
        }
        let count = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if count == 0 {
            return false;
        }
        bytes = &bytes[count..];
    }
    bytes.is_empty()
}

fn digits_at(literal: &str, at: &mut usize) -> String {
    let bytes = literal.as_bytes();
    let start = *at;
    while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
        *at += 1;
    }
    literal[start..*at].to_string()
}

fn power_of(digits: &str) -> Option<i128> {
    let mut value: i128 = 0;
    for byte in digits.bytes() {
        value = value
            .checked_mul(10)?
            .checked_add(i128::from(byte - b'0'))?;
    }
    Some(value)
}
