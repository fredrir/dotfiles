
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Plain,
    Hypr,
    Kitty,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Plain => "plain",
            Mode::Hypr => "hypr",
            Mode::Kitty => "kitty",
        }
    }
}

const MODES: &[(&str, Mode)] = &[
    ("*/hypr/*", Mode::Hypr),
    ("*/hypr-local.conf", Mode::Hypr),
    ("hypr*.conf", Mode::Hypr),
    ("*/kitty/colors*.conf", Mode::Plain),
    ("*/colors*.conf", Mode::Plain),
    ("*/kitty/conf.d/fonts.conf", Mode::Plain),
    ("*/kitty/*.conf", Mode::Kitty),
    ("*/kitty.conf", Mode::Kitty),
];

pub fn lines(text: &str) -> Vec<&str> {
    let mut found: Vec<&str> = text.split('\n').collect();
    if found.last() == Some(&"") {
        found.pop();
    }
    for line in &mut found {
        *line = line.strip_suffix('\r').unwrap_or(line);
    }
    found
}

pub fn format(text: &str, mode: Mode) -> String {
    let raw = lines(text);
    let formatted = format_lines(&raw, mode);
    if formatted.is_empty() {
        return text.to_string();
    }
    let mut out = formatted.join("\n");
    out.push('\n');
    out
}

pub fn mode(path: &str) -> Mode {
    for wanted in [Mode::Hypr, Mode::Plain, Mode::Kitty] {
        let matched = MODES
            .iter()
            .any(|(pattern, mode)| *mode == wanted && matches(pattern, path));
        if matched {
            return wanted;
        }
    }
    Mode::Plain
}

pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = path.chars().collect();
    let (mut at, mut here) = (0, 0);
    let (mut star, mut resume) = (None, 0);
    while here < text.len() {
        if at < pattern.len() {
            if pattern[at] == '*' {
                star = Some(at);
                resume = here;
                at += 1;
                continue;
            }
            if let Some(next) = step(&pattern, at, text[here]) {
                at = next;
                here += 1;
                continue;
            }
        }
        // Nothing matched here, so the last `*` swallows one more character
        // and the pattern after it is tried again.
        let Some(back) = star else {
            return false;
        };
        at = back + 1;
        resume += 1;
        here = resume;
    }
    pattern[at..].iter().all(|ch| *ch == '*')
}

fn step(pattern: &[char], at: usize, ch: char) -> Option<usize> {
    match pattern[at] {
        '?' => Some(at + 1),
        '[' => match class(pattern, at, ch) {
            Some((end, true)) => Some(end),
            Some((_, false)) => None,
            // An unclosed `[` is a literal `[`, which is what fnmatch does.
            None => (ch == '[').then_some(at + 1),
        },
        literal => (literal == ch).then_some(at + 1),
    }
}

fn class(pattern: &[char], at: usize, ch: char) -> Option<(usize, bool)> {
    let mut here = at + 1;
    let negated = pattern.get(here) == Some(&'!');
    if negated {
        here += 1;
    }
    let mut hit = false;
    let mut first = true;
    while here < pattern.len() {
        // A `]` in the first position is a literal, so `[]]` is the class of
        // one bracket rather than an empty one.
        if pattern[here] == ']' && !first {
            return Some((here + 1, hit != negated));
        }
        first = false;
        let ranged = pattern.get(here + 1) == Some(&'-')
            && pattern.get(here + 2).is_some_and(|end| *end != ']');
        if ranged {
            hit |= pattern[here] <= ch && ch <= pattern[here + 2];
            here += 3;
            continue;
        }
        hit |= pattern[here] == ch;
        here += 1;
    }
    None
}

fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut space = false;
    for ch in text.chars() {
        if let Some(mark) = quote {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == mark {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            quote = Some(ch);
            out.push(ch);
        } else if ch == ' ' || ch == '\t' {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(ch);
        }
    }
    out
}

fn format_lines(lines: &[&str], mode: Mode) -> Vec<String> {
    if mode == Mode::Kitty {
        return format_kitty(lines);
    }
    let mut out: Vec<String> = Vec::new();
    let mut printed = false;
    let mut blank = false;
    let mut indent = 0;
    for raw in lines {
        let line = raw.trim_end_matches([' ', '\t']);
        if line.is_empty() {
            if printed {
                blank = true;
            }
            continue;
        }
        let closing = line.trim_start_matches([' ', '\t']);
        // The gap above a `}` is the gap at the end of a block, which is not
        // a gap between two things.
        if blank && !(mode == Mode::Hypr && closing == "}") {
            out.push(String::new());
        }
        blank = false;
        if mode == Mode::Hypr {
            let (indented, depth) = hypr_line(line, indent);
            indent = depth;
            out.push(indented);
        } else {
            out.push(line.to_string());
        }
        printed = true;
    }
    out
}

fn hypr_key(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '$' | '_' | '.' | ':' | '-'))
}

fn hypr_line(line: &str, indent: usize) -> (String, usize) {
    let line = line.trim_start_matches([' ', '\t']);
    let mut indent = if line == "}" {
        indent.saturating_sub(1)
    } else {
        indent
    };
    let mut text = line.to_string();
    if !line.starts_with('#')
        && let Some((left, right)) = line.split_once('=')
    {
        let key = left.trim_end_matches([' ', '\t']);
        let value = right.trim_start_matches([' ', '\t']);
        if hypr_key(key) {
            text = format!("{key} =");
            if !value.is_empty() {
                text.push(' ');
                text.push_str(value);
            }
        }
    }
    let text = format!("{}{text}", "    ".repeat(indent));
    if opens(&text) {
        indent += 1;
    }
    (text, indent)
}

fn opens(line: &str) -> bool {
    let body = line.trim_start_matches([' ', '\t']);
    if body.starts_with('#') {
        return false;
    }
    body.strip_suffix('{')
        .is_some_and(|before| !before.contains('='))
}

fn format_kitty(lines: &[&str]) -> Vec<String> {
    let mut stored: Vec<String> = Vec::new();
    let mut blank_pending = false;
    let mut key_width = 0;
    let mut map_width = 0;
    for raw in lines {
        let line = raw.trim_end_matches([' ', '\t']);
        if line.is_empty() {
            if !stored.is_empty() {
                blank_pending = true;
            }
            continue;
        }
        // Deviation 3: a comment is never compacted. `format.py` compacts
        // first and asks about `#` afterwards, so an apostrophe in a comment
        // opens a quote that never closes and swallows the rest of the line.
        let bare = line.trim_start_matches([' ', '\t']);
        let comment = bare.starts_with('#');
        let entry = if comment {
            bare.to_string()
        } else {
            compact(line)
        };
        if blank_pending && !stored.is_empty() {
            stored.push(String::new());
        }
        blank_pending = false;
        stored.push(entry.clone());
        if comment {
            continue;
        }
        let Some((key, tail)) = entry.split_once(' ') else {
            continue;
        };
        if key == "map" {
            // `map <shortcut>` with no action is not a shortcut line yet, and
            // measuring it would push the column out for nothing.
            let Some((shortcut, _)) = tail.split_once(' ') else {
                continue;
            };
            map_width = map_width.max(shortcut.chars().count());
        } else {
            key_width = key_width.max(key.chars().count());
        }
    }

    let mut out = Vec::with_capacity(stored.len());
    for entry in stored {
        if entry.starts_with('#') || !entry.contains(' ') {
            out.push(entry);
            continue;
        }
        let Some((key, value)) = entry.split_once(' ') else {
            out.push(entry);
            continue;
        };
        if key != "map" {
            let pad = key_width + 2 - key.chars().count();
            out.push(format!("{key}{}{value}", " ".repeat(pad)));
            continue;
        }
        let Some((shortcut, action)) = value.split_once(' ') else {
            out.push(entry);
            continue;
        };
        let pad = map_width + 2 - shortcut.chars().count();
        out.push(format!("map {shortcut}{}{action}", " ".repeat(pad)));
    }
    out
}
