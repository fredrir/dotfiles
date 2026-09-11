use super::{Result, color::Color};
#[derive(Clone, Debug)]
pub enum Expr {
    Named(String),
    Mix(String, String, i64),
    Ladder(String, i64),
    Function(String, Vec<Expr>, f64),
    Alpha(Box<Expr>, f64),
}
#[derive(Clone, Copy, Debug)]
pub struct Resolved {
    pub color: Color,
    pub alpha: Option<f64>,
}
impl Expr {
    pub fn parse(source: &str) -> Result<Self> {
        Self::parse_depth(source, 0)
    }
    fn parse_depth(source: &str, depth: usize) -> Result<Self> {
        if depth > 64 {
            return Err("color expression nesting exceeds 64".into());
        }
        let source = source.trim();
        if source.is_empty() {
            return Err("empty color expression".into());
        }
        let parts = split(source, '/')?;
        if parts.len() > 3 {
            return Err(format!("too many '/' parts: {source}"));
        }
        let mut tail = &parts[1..];
        let mut alpha = None;
        if let Some(last) = tail.last().and_then(|s| s.strip_suffix('%')) {
            let number = last.trim();
            let valid_percent = match number.split_once('.') {
                Some((whole, fractional)) => {
                    !whole.is_empty()
                        && !fractional.is_empty()
                        && whole.bytes().all(|c| c.is_ascii_digit())
                        && fractional.bytes().all(|c| c.is_ascii_digit())
                }
                None => !number.is_empty() && number.bytes().all(|c| c.is_ascii_digit()),
            };
            if !valid_percent {
                return Err(format!("alpha must be a percent from 0 to 100: {source}"));
            }
            let value: f64 = number
                .parse::<f64>()
                .map_err(|_| format!("invalid alpha: {source}"))?;
            if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                return Err(format!("alpha must be a percent from 0 to 100: {source}"));
            }
            alpha = Some(value / 100.);
            tail = &tail[..tail.len() - 1];
        }
        if tail.len() > 1 {
            return Err(format!("alpha must come last: {source}"));
        }
        let step = tail
            .first()
            .map(|s| {
                s.parse::<i64>()
                    .map_err(|_| format!("step must be a whole per-mille: {source}"))
            })
            .transpose()?;
        let head = parts[0];
        let expr = if let Some(open) = head.find('(') {
            if !head.ends_with(')') {
                return Err(format!("unbalanced parentheses: {source}"));
            }
            let name = head[..open].trim();
            if step.is_some() {
                return Err(format!("{name}() takes no ladder step: {source}"));
            }
            let args = split(&head[open + 1..head.len() - 1], ',')?;
            let (count, default) = match name {
                "contrast" => (1, 4.5),
                "on" => (1, 4.5),
                "readable" | "visible" => (2, 4.5),
                _ => return Err(format!("unknown color function '{name}': {source}")),
            };
            if args.len() < count
                || args.len() > count + usize::from(name != "contrast")
                || args.iter().any(|s| s.is_empty())
            {
                return Err(format!("invalid {name}() arguments: {source}"));
            }
            let floor = if args.len() > count {
                args[count]
                    .parse::<f64>()
                    .map_err(|_| format!("contrast floor must be a number: {source}"))?
            } else {
                default
            };
            if !floor.is_finite() || !(1.0..=21.0).contains(&floor) {
                return Err(format!("contrast floor must be from 1 to 21: {source}"));
            }
            Self::Function(
                name.into(),
                args[..count]
                    .iter()
                    .map(|a| Self::parse_depth(a, depth + 1))
                    .collect::<Result<_>>()?,
                floor,
            )
        } else if head.contains('~') {
            let pair = head.split('~').map(str::trim).collect::<Vec<_>>();
            if pair.len() != 2 || pair.iter().any(|s| s.is_empty()) {
                return Err(format!("mix takes two colors: {source}"));
            }
            Self::Mix(
                pair[0].into(),
                pair[1].into(),
                step.ok_or_else(|| format!("mix needs a step: {source}"))?,
            )
        } else if let Some(step) = step {
            if !anchor(head) {
                return Err(format!("ladder needs background or foreground: {source}"));
            }
            Self::Ladder(head.into(), step)
        } else {
            Self::Named(head.into())
        };
        Ok(if let Some(alpha) = alpha {
            Self::Alpha(Box::new(expr), alpha)
        } else {
            expr
        })
    }
    pub fn evaluate(
        &self,
        lookup: &mut impl FnMut(&str) -> Result<Color>,
        bg: Color,
        fg: Color,
    ) -> Result<Resolved> {
        let mut named = |name: &str| match name {
            "bg" | "background" | "ui.background" => Ok(bg),
            "fg" | "foreground" | "ui.foreground" => Ok(fg),
            _ if name.starts_with('#') => Color::parse(name),
            _ => lookup(name),
        };
        let color = match self {
            Self::Named(name) => named(name)?,
            Self::Mix(a, b, step) => named(a)?.mix(named(b)?, *step as f64 / 1000.),
            Self::Ladder(name, step) => {
                let background = matches!(name.as_str(), "bg" | "background" | "ui.background");
                let (a, b) = if background { (bg, fg) } else { (fg, bg) };
                let ahead = (fg.luminance() > bg.luminance()) != background;
                let amount = *step as f64 / 1000.;
                a.mix(
                    b,
                    if amount < 0. && ahead {
                        -amount
                    } else {
                        amount
                    },
                )
            }
            Self::Function(name, args, floor) => {
                let mut values = Vec::new();
                for arg in args {
                    let value = arg.evaluate(lookup, bg, fg)?;
                    if value.alpha.is_some() {
                        return Err("alpha is not allowed inside a color function".into());
                    }
                    values.push(value.color);
                }
                if matches!(name.as_str(), "contrast" | "on") {
                    values[0].on(fg, bg, *floor)?
                } else {
                    values[0].readable(values[1], *floor)?
                }
            }
            Self::Alpha(expr, alpha) => {
                let mut r = expr.evaluate(lookup, bg, fg)?;
                r.alpha = Some(*alpha);
                return Ok(r);
            }
        };
        Ok(Resolved { color, alpha: None })
    }
}
fn anchor(name: &str) -> bool {
    matches!(
        name,
        "bg" | "background" | "ui.background" | "fg" | "foreground" | "ui.foreground"
    )
}
fn split(text: &str, delimiter: char) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| format!("unbalanced parentheses: {text}"))?
            }
            _ => {}
        }
        if ch == delimiter && depth == 0 {
            parts.push(text[start..i].trim());
            start = i + ch.len_utf8();
        }
    }
    if depth != 0 {
        return Err(format!("unbalanced parentheses: {text}"));
    }
    parts.push(text[start..].trim());
    Ok(parts)
}
