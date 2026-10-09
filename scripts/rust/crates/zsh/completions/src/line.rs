use crate::help::{Flag, Help};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub words: Vec<String>,
    // Index of the word under the cursor.
    pub current: usize,
    // The current word up to the cursor.
    pub prefix: String,
}

#[derive(Debug, Default)]
pub struct Scan<'a> {
    pub positionals: Vec<&'a str>,
    pub first_positional: Option<usize>,
    pub flags: Vec<&'a str>,
    pub pending: Option<&'a Flag>,
    pub after_separator: bool,
}

impl Line {
    // `current` counts from one, as `$CURRENT` does in zsh.
    pub fn new(mut words: Vec<String>, current: usize, prefix: Option<String>) -> Line {
        if words.is_empty() {
            words.push(String::new());
        }
        let current = current.clamp(1, words.len() + 1) - 1;
        if current == words.len() {
            words.push(String::new());
        }
        let prefix = prefix.unwrap_or_else(|| words[current].clone());
        Line {
            words,
            current,
            prefix,
        }
    }

    pub fn before(&self) -> &[String] {
        self.words.get(1..self.current).unwrap_or_default()
    }

    pub fn previous(&self) -> Option<&str> {
        self.current
            .checked_sub(1)
            .filter(|index| *index > 0)
            .map(|index| self.words[index].as_str())
    }

    pub fn has_flag(&self, names: &[&str]) -> bool {
        self.words.iter().enumerate().any(|(index, word)| {
            index != self.current
                && names
                    .iter()
                    .any(|name| word == name || word.starts_with(&format!("{name}=")))
        })
    }

    // The value of `--flag value` or `--flag=value` anywhere on the line.
    pub fn flag_value(&self, names: &[&str]) -> Option<&str> {
        for (index, word) in self.words.iter().enumerate() {
            if index == self.current {
                continue;
            }
            for name in names {
                if word == name {
                    return self.words.get(index + 1).map(String::as_str);
                }
                if let Some(value) = word.strip_prefix(&format!("{name}=")) {
                    return Some(value);
                }
            }
        }
        None
    }
}

pub fn scan<'a>(words: &'a [String], helps: &[&'a Help]) -> Scan<'a> {
    scan_words(words, helps, false)
}

// Top-level flags stop applying once a subcommand is reached.
pub fn scan_command<'a>(words: &'a [String], helps: &[&'a Help]) -> Scan<'a> {
    scan_words(words, helps, true)
}

fn scan_words<'a>(words: &'a [String], helps: &[&'a Help], command: bool) -> Scan<'a> {
    let lookup = |name: &str| helps.iter().find_map(|help| help.flag(name));
    let mut scan = Scan::default();
    let mut iter = words.iter().enumerate();
    while let Some((index, word)) = iter.next() {
        if scan.after_separator {
            scan.first_positional.get_or_insert(index);
            scan.positionals.push(word);
            if command {
                break;
            }
            continue;
        }
        if word == "--" {
            scan.after_separator = true;
            continue;
        }
        if word.starts_with('-') && word.len() > 1 {
            let (name, inline) = match word.split_once('=') {
                Some((name, _)) => (name, true),
                None => (word.as_str(), false),
            };
            scan.flags.push(name);
            let flag = lookup(name).filter(|flag| flag.takes_separate_value() && !inline);
            if let Some(flag) = flag
                && iter.next().is_none()
            {
                scan.pending = Some(flag);
            }
            continue;
        }
        scan.first_positional.get_or_insert(index);
        scan.positionals.push(word);
        if command {
            break;
        }
    }
    scan
}

#[cfg(test)]
#[path = "../tests/unit/line_tests.rs"]
mod tests;
