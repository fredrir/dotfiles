
pub fn canonical(literal: &str) -> Option<String> {
    let digits = Digits::read(literal)?;
    Some(digits.write())
}

struct Digits {
    negative: bool,
    digits: String,
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

    fn write(&self) -> String {
        let sign = if self.negative { "-" } else { "" };
        let places = self.digits.len() as i64;
        let point = places + self.exponent;
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

fn power_of(digits: &str) -> i64 {
    let mut value: i64 = 0;
    for byte in digits.bytes() {
        value = value
            .saturating_mul(10)
            .saturating_add(i64::from(byte - b'0'));
    }
    value
}
