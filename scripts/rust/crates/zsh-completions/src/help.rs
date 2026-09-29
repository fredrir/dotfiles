use serde::{Deserialize, Serialize};

const METAVARS: &[&str] = &[
    "path",
    "id",
    "file",
    "dir",
    "directory",
    "name",
    "pkg",
    "package",
    "glob",
    "source",
    "url",
    "value",
    "val",
    "key",
    "text",
    "number",
    "n",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Help {
    pub commands: Vec<Command>,
    pub flags: Vec<Flag>,
    pub aliases: Vec<String>,
    pub positionals: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flag {
    pub names: Vec<String>,
    pub value: Option<Value>,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub placeholder: String,
    pub optional: bool,
    pub choices: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Other,
    Usage,
    Commands,
    CommandList,
    Examples,
}

impl Flag {
    pub fn has(&self, name: &str) -> bool {
        self.names.iter().any(|known| known == name)
    }

    pub fn long(&self) -> Option<&str> {
        self.names
            .iter()
            .find_map(|name| name.strip_prefix("--"))
            .or_else(|| self.names.first().map(|name| name.trim_start_matches('-')))
    }

    // A value that has to follow as the next word, as opposed to only after `=`.
    pub fn takes_separate_value(&self) -> bool {
        self.value.as_ref().is_some_and(|value| !value.optional)
    }
}

impl Value {
    fn new(placeholder: &str, optional: bool) -> Value {
        let placeholder = placeholder.trim_end_matches("...").to_string();
        let choices = placeholder_choices(&placeholder);
        Value {
            placeholder,
            optional,
            choices,
        }
    }

    pub fn names_a_path(&self) -> bool {
        let placeholder = self.placeholder.to_ascii_lowercase();
        ["path", "file", "dir", "folder", "cafile"]
            .iter()
            .any(|word| placeholder.contains(word))
    }

    pub fn names_a_directory(&self) -> bool {
        let placeholder = self.placeholder.to_ascii_lowercase();
        placeholder.contains("dir") || placeholder.contains("folder")
    }
}

impl Help {
    pub fn parse(text: &str, program: &[&str]) -> Help {
        Parser::new(program).run(text)
    }

    pub fn flag(&self, name: &str) -> Option<&Flag> {
        let name = name.split_once('=').map_or(name, |(name, _)| name);
        self.flags.iter().find(|flag| flag.has(name))
    }

    pub fn command(&self, name: &str) -> Option<&Command> {
        self.commands
            .iter()
            .find(|command| command.name == name || command.aliases.iter().any(|a| a == name))
    }

    pub fn add_flag(&mut self, flag: Flag) {
        let Some(known) = self
            .flags
            .iter_mut()
            .find(|known| flag.names.iter().any(|name| known.has(name)))
        else {
            self.flags.push(flag);
            return;
        };
        for name in flag.names {
            if !known.has(&name) {
                known.names.push(name);
            }
        }
        if known.value.is_none() {
            known.value = flag.value;
        } else if let (Some(value), Some(extra)) = (known.value.as_mut(), flag.value)
            && value.choices.is_empty()
        {
            value.choices = extra.choices;
        }
        if known.description.is_empty() {
            known.description = flag.description;
        }
    }

    fn add_command(&mut self, command: Command) {
        if let Some(known) = self
            .commands
            .iter_mut()
            .find(|known| known.name == command.name)
        {
            for alias in command.aliases {
                if !known.aliases.contains(&alias) {
                    known.aliases.push(alias);
                }
            }
            if known.description.is_empty() {
                known.description = command.description;
            }
            return;
        }
        self.commands.push(command);
    }
}

struct Parser<'a> {
    program: &'a [&'a str],
    help: Help,
    usage_flags: Vec<Flag>,
    section: Section,
    commands_indent: Option<usize>,
    open_flag: Option<(usize, usize)>,
    // A description written below its flag continues until a blank line.
    joining: bool,
}

impl<'a> Parser<'a> {
    fn new(program: &'a [&'a str]) -> Parser<'a> {
        Parser {
            program,
            help: Help::default(),
            usage_flags: Vec::new(),
            section: Section::Other,
            commands_indent: None,
            open_flag: None,
            joining: false,
        }
    }

    fn run(mut self, text: &str) -> Help {
        for raw in text.lines() {
            self.line(raw.trim_end());
        }
        for flag in std::mem::take(&mut self.usage_flags) {
            if flag.names.iter().any(|name| self.help.flag(name).is_some()) {
                self.help.add_flag(flag);
            } else {
                self.help.flags.push(flag);
            }
        }
        for flag in &mut self.help.flags {
            if let Some(value) = flag.value.as_mut().filter(|value| value.choices.is_empty()) {
                value.choices = description_choices(&flag.description);
            }
        }
        self.help
    }

    fn line(&mut self, line: &str) {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.is_empty() {
            self.joining = false;
            return;
        }
        if let Some((section, rest)) = header(trimmed, indent) {
            self.section = section;
            self.commands_indent = None;
            self.open_flag = None;
            if !rest.is_empty() {
                self.usage_line(rest);
            }
            return;
        }
        if self.alias_line(trimmed) {
            return;
        }
        if trimmed.starts_with('-')
            && self.section != Section::Examples
            && let Some(flag) = flag_line(trimmed)
        {
            self.help.add_flag(flag.clone());
            let index = self
                .help
                .flags
                .iter()
                .position(|known| flag.names.iter().any(|name| known.has(name)));
            self.open_flag = index.map(|index| (index, indent));
            return;
        }
        if let Some((index, flag_indent)) = self.open_flag {
            if indent > flag_indent {
                self.continuation(index, trimmed);
                return;
            }
            self.open_flag = None;
        }
        match self.section {
            Section::Commands => self.command_line(trimmed, indent),
            Section::CommandList => {
                for name in trimmed.split(',').map(str::trim) {
                    if is_word(name) {
                        self.help.add_command(Command {
                            name: name.into(),
                            ..Command::default()
                        });
                    }
                }
            }
            Section::Usage => self.usage_line(trimmed),
            Section::Other if trimmed.starts_with('[') => self.usage_line(trimmed),
            _ => {}
        }
    }

    fn continuation(&mut self, index: usize, trimmed: &str) {
        let flag = &mut self.help.flags[index];
        if trimmed.starts_with("[possible values:") {
            if let Some(value) = flag.value.as_mut() {
                value.choices = description_choices(trimmed);
            }
        } else if let Some(aliases) = bracket_aliases(trimmed) {
            for alias in aliases.iter().filter(|alias| alias.starts_with('-')) {
                if !flag.has(alias) {
                    flag.names.push(alias.clone());
                }
            }
        } else if flag.description.is_empty() {
            flag.description = collapse(trimmed);
            self.joining = true;
        } else if self.joining {
            flag.description = format!("{} {}", flag.description, collapse(trimmed));
        }
    }

    fn alias_line(&mut self, trimmed: &str) -> bool {
        let lower = trimmed.to_ascii_lowercase();
        let Some(rest) = ["aliases:", "alias:"]
            .iter()
            .find_map(|label| lower.starts_with(label).then(|| &trimmed[label.len()..]))
        else {
            return false;
        };
        for alias in rest.split(',') {
            let mut words = alias.split_whitespace();
            let mut word = words.next();
            if word == self.program.first().copied() {
                word = words.next();
            }
            if let Some(word) = word.filter(|word| is_word(word))
                && !self.help.aliases.iter().any(|known| known == word)
            {
                self.help.aliases.push(word.into());
            }
        }
        true
    }

    fn command_line(&mut self, trimmed: &str, indent: usize) {
        let expected = *self.commands_indent.get_or_insert(indent);
        if indent > expected {
            return;
        }
        let mut text = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        for word in self.program {
            text = text
                .strip_prefix(word)
                .and_then(|rest| rest.strip_prefix(' '))
                .unwrap_or(text);
        }
        let name = text.split_whitespace().next().unwrap_or("");
        if !is_word(name) {
            return;
        }
        let columns = columns(text);
        let mut description = if columns.len() > 1 {
            columns.last().copied().unwrap_or("").to_string()
        } else {
            String::new()
        };
        let mut aliases = Vec::new();
        if let Some(start) = description.find("[alias") {
            if let Some(found) = bracket_aliases(&description[start..]) {
                aliases = found;
            }
            description.truncate(start);
        }
        if let Some(program) = self.program.first() {
            let marker = format!("({program} ");
            if let Some(start) = description.rfind(&marker) {
                let inner = description[start + marker.len()..].trim_end_matches(')');
                if is_word(inner) {
                    aliases.push(inner.to_string());
                    description.truncate(start);
                }
            }
        }
        self.help.add_command(Command {
            name: name.into(),
            aliases,
            description: collapse(&description),
        });
    }

    fn usage_line(&mut self, trimmed: &str) {
        for group in top_level_brackets(trimmed) {
            let alternatives = split_top_level(group, '|');
            for alternative in &alternatives {
                if let Some(flag) = usage_flag(alternative.trim()) {
                    self.usage_flags.push(flag);
                }
            }
            if alternatives.len() > 1 && self.section == Section::Usage {
                for word in alternatives.iter().map(|word| word.trim()) {
                    let metavar = METAVARS.contains(&word.to_ascii_lowercase().as_str());
                    if is_word(word)
                        && !metavar
                        && !self.help.positionals.iter().any(|known| known == word)
                    {
                        self.help.positionals.push(word.into());
                    }
                }
            }
        }
        let mut words = trimmed.split_whitespace();
        for expected in self.program {
            if words.next() != Some(expected) {
                return;
            }
        }
        let Some(name) = words.next().filter(|word| is_word(word)) else {
            return;
        };
        let columns = columns(trimmed);
        let description = if columns.len() > 1 {
            collapse(columns.last().copied().unwrap_or(""))
        } else {
            String::new()
        };
        self.help.add_command(Command {
            name: name.into(),
            aliases: Vec::new(),
            description,
        });
    }
}

fn header(trimmed: &str, indent: usize) -> Option<(Section, &str)> {
    if indent > 4 {
        return None;
    }
    let (name, rest) = trimmed.split_once(':')?;
    let rest = rest.trim();
    let starts_upper = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    let plain = name.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ');
    if !starts_upper || !plain || name.split_whitespace().count() > 4 {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let section = if lower.starts_with("usage") {
        Section::Usage
    } else if lower.contains("example") || lower.contains("short forms") {
        Section::Examples
    } else if lower.starts_with("all commands") {
        Section::CommandList
    } else if lower.ends_with("commands") {
        Section::Commands
    } else {
        Section::Other
    };
    if !rest.is_empty() && section != Section::Usage {
        return None;
    }
    Some((section, rest))
}

fn flag_line(trimmed: &str) -> Option<Flag> {
    let mut rest = trimmed;
    let mut names = Vec::new();
    let mut value = None;
    while rest.starts_with('-') {
        let end = rest
            .find([' ', ',', '=', '|', '<', '[', '\t'])
            .unwrap_or(rest.len());
        let name = &rest[..end];
        if !is_flag_name(name) {
            break;
        }
        names.push(name.to_string());
        rest = &rest[end..];
        if let Some(after) = rest.strip_prefix('=') {
            let (placeholder, remaining) = placeholder_token(after);
            value = Some(Value::new(unwrap_placeholder(placeholder), false));
            rest = remaining;
        } else if let Some(after) = rest.strip_prefix(' ')
            && (after.starts_with('<') || (after.starts_with('[') && !after.starts_with("[alias")))
        {
            let (placeholder, remaining) = placeholder_token(after);
            value = Some(Value::new(
                unwrap_placeholder(placeholder),
                placeholder.starts_with('['),
            ));
            rest = remaining;
        }
        if let Some(after) = rest.strip_prefix(',') {
            let after = after.trim_start();
            if after.starts_with('-') {
                rest = after;
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('|')
            && after.starts_with('-')
        {
            rest = after;
            continue;
        }
        break;
    }
    if names.is_empty() {
        return None;
    }
    Some(Flag {
        names,
        value,
        description: collapse(rest),
    })
}

fn usage_flag(alternative: &str) -> Option<Flag> {
    if !alternative.starts_with('-') {
        return None;
    }
    let end = alternative.find([' ', '=']).unwrap_or(alternative.len());
    let name = &alternative[..end];
    if !is_flag_name(name) {
        return None;
    }
    let rest = alternative[end..].trim_start_matches(['=', ' ']);
    let value = rest.starts_with('<').then(|| {
        let (placeholder, _) = placeholder_token(rest);
        Value::new(unwrap_placeholder(placeholder), false)
    });
    Some(Flag {
        names: vec![name.to_string()],
        value,
        description: String::new(),
    })
}

fn placeholder_token(text: &str) -> (&str, &str) {
    let mut depth = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '<' | '[' => depth += 1,
            '>' | ']' => depth = depth.saturating_sub(1),
            ' ' | ',' | '\t' if depth == 0 => return (&text[..index], &text[index..]),
            _ => {}
        }
    }
    (text, "")
}

fn unwrap_placeholder(token: &str) -> &str {
    let token = token.trim_end_matches("...");
    let inner = token
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
        .or_else(|| {
            token
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
        })
        .unwrap_or(token);
    inner.trim_start_matches('<').trim_end_matches('>')
}

fn placeholder_choices(placeholder: &str) -> Vec<String> {
    let parts: Vec<&str> = placeholder.split('|').map(str::trim).collect();
    let simple = parts.len() > 1
        && parts.iter().all(|part| is_word(part))
        && !parts
            .iter()
            .any(|part| METAVARS.contains(&part.to_ascii_lowercase().as_str()));
    if simple {
        parts.into_iter().map(String::from).collect()
    } else {
        Vec::new()
    }
}

fn description_choices(description: &str) -> Vec<String> {
    let lower = description.to_ascii_lowercase();
    if let Some(start) = lower.find("[possible values:") {
        let rest = &description[start + "[possible values:".len()..];
        let rest = rest.split(']').next().unwrap_or("");
        return word_list(rest);
    }
    if lower.contains("one of") || lower.contains("possible values") {
        let quoted = quoted_words(description);
        if quoted.len() > 1 {
            return quoted;
        }
    }
    let Some((_, tail)) = description.rsplit_once(": ") else {
        return Vec::new();
    };
    word_list(tail)
}

fn word_list(text: &str) -> Vec<String> {
    let text = text.replace("(default)", "");
    let mut words = Vec::new();
    for part in text.split(',').flat_map(|part| part.split(" or ")) {
        let word = part.trim().trim_start_matches("or ").trim();
        if word.is_empty() {
            continue;
        }
        if !is_word(word) {
            return Vec::new();
        }
        words.push(word.to_string());
    }
    if words.len() > 1 { words } else { Vec::new() }
}

fn quoted_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    for quote in ['"', '\''] {
        let parts: Vec<&str> = text.split(quote).collect();
        for word in parts.iter().skip(1).step_by(2) {
            if is_word(word) && !words.iter().any(|known| known == word) {
                words.push((*word).to_string());
            }
        }
    }
    words
}

fn bracket_aliases(text: &str) -> Option<Vec<String>> {
    let inner = text.trim().strip_prefix('[')?;
    let inner = inner.split(']').next()?;
    let (label, list) = inner.split_once(':')?;
    if !label.trim().starts_with("alias") {
        return None;
    }
    Some(
        list.split(',')
            .map(|alias| alias.trim().to_string())
            .filter(|alias| !alias.is_empty())
            .collect(),
    )
}

fn top_level_brackets(text: &str) -> Vec<&str> {
    let mut groups = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut angle = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '[' if angle == 0 => {
                if depth == 0 {
                    start = index + 1;
                }
                depth += 1;
            }
            ']' if angle == 0 && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    groups.push(&text[start..index]);
                }
            }
            _ => {}
        }
    }
    groups
}

fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, c) in text.char_indices() {
        match c {
            '<' | '[' => depth += 1,
            '>' | ']' => depth = depth.saturating_sub(1),
            c if c == separator && depth == 0 => {
                parts.push(&text[start..index]);
                start = index + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn columns(text: &str) -> Vec<&str> {
    text.split("  ")
        .map(str::trim)
        .filter(|column| !column.is_empty())
        .collect()
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_flag_name(name: &str) -> bool {
    let body = name.trim_start_matches('-');
    let dashes = name.len() - body.len();
    (1..=2).contains(&dashes)
        && body
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

pub fn is_word(word: &str) -> bool {
    word.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
#[path = "../tests/unit/help_tests.rs"]
mod tests;
