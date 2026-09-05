use crate::{Binding, Package, humanize};
use serde::Deserialize;
use std::collections::BTreeMap;

fn row(
    source: &str,
    line: usize,
    key: String,
    action: String,
    context: String,
    description: String,
) -> Binding {
    Binding {
        source: source.into(),
        line,
        key,
        action,
        context,
        description,
    }
}

#[derive(Debug)]
struct Word {
    value: String,
    start: usize,
}

fn words(s: &str) -> Result<Vec<Word>, String> {
    let mut result = Vec::new();
    let mut value = String::new();
    let mut quote = None;
    let mut start = None;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if quote.is_none() && c.is_whitespace() {
            if let Some(start) = start.take() {
                result.push(Word {
                    value: std::mem::take(&mut value),
                    start,
                });
            }
            continue;
        }
        if quote.is_none() && c == '#' && start.is_none() {
            break;
        }
        start.get_or_insert(i);
        if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && matches!(c, '\'' | '"') {
            quote = Some(c);
        } else if c == '\\' && quote != Some('\'') {
            if let Some((_, next)) = chars.next() {
                if matches!(next, '\\' | '\'' | '"' | ' ') {
                    value.push(next);
                } else {
                    value.push('\\');
                    value.push(next);
                }
            } else {
                return Err("unfinished escape".into());
            }
        } else {
            value.push(c);
        }
    }
    if quote.is_some() {
        return Err("unclosed quote".into());
    }
    if let Some(start) = start {
        result.push(Word { value, start });
    }
    Ok(result)
}

fn lines(body: &str) -> Vec<(usize, String)> {
    let mut result = Vec::new();
    let mut pending = String::new();
    let mut start = 1;
    for (i, line) in body.lines().enumerate() {
        if pending.is_empty() {
            start = i + 1;
        }
        let trimmed = line.trim_end();
        let slashes = trimmed.chars().rev().take_while(|c| *c == '\\').count();
        if slashes % 2 == 1 {
            pending.push_str(&trimmed[..trimmed.len() - 1]);
            pending.push(' ');
        } else {
            pending.push_str(line);
            result.push((start, std::mem::take(&mut pending)));
        }
    }
    if !pending.is_empty() {
        result.push((start, pending));
    }
    result
}

pub fn tmux(source: &str, body: &str, package: &mut Package) -> Result<(), String> {
    let mut conditions = Vec::new();
    for (line, text) in lines(body) {
        let text = text.trim();
        if let Some(condition) = text.strip_prefix("%if ") {
            conditions.push(condition.to_string());
            continue;
        }
        if text == "%else" {
            let previous = conditions
                .pop()
                .ok_or_else(|| format!("{line}: unmatched %else"))?;
            conditions.push(format!("not ({previous})"));
            continue;
        }
        if let Some(condition) = text.strip_prefix("%elif ") {
            let previous = conditions
                .pop()
                .ok_or_else(|| format!("{line}: unmatched %elif"))?;
            conditions.push(format!("not ({previous}) and ({condition})"));
            continue;
        }
        if text == "%endif" {
            conditions
                .pop()
                .ok_or_else(|| format!("{line}: unmatched %endif"))?;
            continue;
        }
        let command = text.split_whitespace().next().unwrap_or("");
        if !matches!(
            command,
            "bind"
                | "bind-key"
                | "unbind"
                | "unbind-key"
                | "set"
                | "set-option"
                | "setw"
                | "set-window-option"
        ) {
            continue;
        }
        let words = words(text).map_err(|e| format!("{line}: {e}"))?;
        if command.starts_with("set") {
            if let Some(i) = words.iter().position(|w| {
                matches!(
                    w.value.as_str(),
                    "prefix" | "prefix2" | "mode-keys" | "status-keys"
                ) || w.value.starts_with("user-keys[")
            }) {
                let value = words
                    .get(i + 1)
                    .ok_or_else(|| format!("{line}: missing setting value"))?;
                package.settings.push(row(
                    source,
                    line,
                    words[i].value.clone(),
                    value.value.clone(),
                    conditions.join("; "),
                    String::new(),
                ));
            }
            continue;
        }
        let mut table = "prefix".to_string();
        let mut description = String::new();
        let mut flags = Vec::new();
        let mut all = false;
        let mut i = 1;
        while let Some(word) = words.get(i) {
            if word.value == "--" {
                i += 1;
                break;
            }
            if !word.value.starts_with('-') || word.value == "-" {
                break;
            }
            match word.value.as_str() {
                "-T" | "-N" => {
                    let value = words
                        .get(i + 1)
                        .ok_or_else(|| format!("{line}: missing {} value", word.value))?;
                    if word.value == "-T" {
                        table = value.value.clone();
                    } else {
                        description = value.value.clone();
                    }
                    i += 1;
                }
                "-n" => table = "root".into(),
                "-r" => flags.push("repeat"),
                "-a" => all = true,
                "-q" => {}
                other => return Err(format!("{line}: unsupported binding flag {other}")),
            }
            i += 1;
        }
        let key = if all {
            "All keys".into()
        } else {
            words
                .get(i)
                .ok_or_else(|| format!("{line}: missing key"))?
                .value
                .clone()
        };
        let action = if command.starts_with("unbind") {
            "Unbind".into()
        } else {
            text.get(
                words
                    .get(i + 1)
                    .ok_or_else(|| format!("{line}: missing action"))?
                    .start..,
            )
            .unwrap()
            .to_string()
        };
        if description.is_empty() {
            description = humanize(action.split(" ").next().unwrap_or(""));
        }
        let mut context = vec![table];
        context.extend(flags.into_iter().map(str::to_string));
        context.extend(conditions.clone());
        package.bindings.push(row(
            source,
            line,
            key,
            action,
            context.join("; "),
            description,
        ));
    }
    if !conditions.is_empty() {
        return Err("unclosed %if".into());
    }
    Ok(())
}

pub fn zsh(source: &str, body: &str, package: &mut Package) -> Result<(), String> {
    let mut conditions = Vec::new();
    let mut guards = Vec::new();
    for (line, text) in lines(body) {
        let text = text.trim();
        if let Some(condition) = text
            .strip_prefix("if ")
            .and_then(|s| s.strip_suffix("; then"))
        {
            conditions.push(condition.to_string());
            continue;
        }
        if let Some(condition) = text
            .strip_prefix("elif ")
            .and_then(|s| s.strip_suffix("; then"))
        {
            if let Some(previous) = conditions.pop() {
                conditions.push(format!("not ({previous}); {condition}"));
            }
            continue;
        }
        if text == "else" {
            if let Some(previous) = conditions.pop() {
                conditions.push(format!("not ({previous})"));
            }
            continue;
        }
        if text == "fi" {
            conditions.pop();
            continue;
        }
        if let Some(guard) = text.strip_suffix(" || return 0") {
            guards.push(guard.to_string());
        }
        if text.starts_with("zstyle ") && text.contains("switch-group") {
            let words = words(text).map_err(|e| format!("{line}: {e}"))?;
            if words.get(2).is_some_and(|w| w.value == "switch-group") {
                for (i, key) in words.iter().skip(3).enumerate() {
                    let action = if i == 0 {
                        "Previous completion group"
                    } else {
                        "Next completion group"
                    };
                    let mut context = vec![words[1].value.clone()];
                    context.extend(guards.clone());
                    context.extend(conditions.clone());
                    package.bindings.push(row(
                        source,
                        line,
                        key.value.clone(),
                        action.into(),
                        context.join("; "),
                        action.into(),
                    ));
                }
            }
        }
        if !text.starts_with("bindkey ") {
            continue;
        }
        let words = words(text).map_err(|e| format!("{line}: {e}"))?;
        let mut mode = "main".to_string();
        let mut i = 1;
        let mut remove = false;
        let mut string = false;
        while let Some(word) = words.get(i) {
            match word.value.as_str() {
                "-M" => {
                    i += 1;
                    mode = words
                        .get(i)
                        .ok_or_else(|| format!("{line}: missing keymap"))?
                        .value
                        .clone();
                }
                "-e" | "-v" => {
                    package.settings.push(row(
                        source,
                        line,
                        "keymap".into(),
                        if word.value == "-e" { "emacs" } else { "viins" }.into(),
                        String::new(),
                        String::new(),
                    ));
                }
                "-r" => remove = true,
                "-s" => string = true,
                "--" => {
                    i += 1;
                    break;
                }
                option if option.starts_with('-') => {
                    return Err(format!("{line}: unsupported bindkey flag {option}"));
                }
                _ => break,
            }
            i += 1;
        }
        if i == words.len() {
            continue;
        }
        let key = words[i]
            .value
            .strip_prefix('$')
            .unwrap_or(&words[i].value)
            .to_string();
        let action = if remove {
            "Unbind".into()
        } else {
            words
                .get(i + 1)
                .ok_or_else(|| format!("{line}: missing widget"))?
                .value
                .clone()
        };
        let mut context = vec![mode];
        if string {
            context.push("send string".into());
        }
        context.extend(guards.clone());
        context.extend(conditions.clone());
        let description = humanize(&action);
        package.bindings.push(row(
            source,
            line,
            key,
            action,
            context.join("; "),
            description,
        ));
    }
    Ok(())
}

pub fn hypr_variables<'a>(bodies: impl Iterator<Item = &'a str>) -> BTreeMap<String, String> {
    let mut variables = BTreeMap::new();
    for body in bodies {
        for line in body.lines().map(str::trim) {
            if line.starts_with('$')
                && let Some((name, value)) = line.split_once('=')
            {
                variables.insert(name.trim().into(), value.trim().into());
            }
        }
    }
    variables
}

fn substitute(s: &str, variables: &BTreeMap<String, String>) -> String {
    let mut result = s.to_string();
    for _ in 0..16 {
        let mut next = String::new();
        let mut chars = result.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '$' {
                next.push(c);
                continue;
            }
            let mut name = "$".to_string();
            while let Some(c) = chars
                .peek()
                .copied()
                .filter(|c| c.is_alphanumeric() || *c == '_')
            {
                name.push(c);
                chars.next();
            }
            next.push_str(variables.get(&name).unwrap_or(&name));
        }
        if next == result {
            break;
        }
        result = next;
    }
    result
}

pub fn hyprland(
    source: &str,
    body: &str,
    variables: &BTreeMap<String, String>,
    package: &mut Package,
) -> Result<(), String> {
    let mut submap = "global".to_string();
    for (line, text) in lines(body) {
        let Some((name, value)) = text.trim().split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name == "submap" {
            submap = value.trim().into();
            continue;
        }
        if !(name == "unbind"
            || name.starts_with("bind") && name[4..].chars().all(|c| "leomrnstdipcu".contains(c)))
        {
            continue;
        }
        let count = if name.contains('d')
            && name != "bind"
            && name != "unbind"
            && name[4..].contains('d')
        {
            5
        } else {
            4
        };
        let values = substitute(value, variables);
        let fields: Vec<_> = values.splitn(count, ',').map(str::trim).collect();
        let minimum = if name == "unbind" {
            2
        } else if count == 5 {
            4
        } else {
            3
        };
        if fields.len() < minimum {
            return Err(format!("{line}: incomplete {name}"));
        }
        let key = [
            fields[0].split_whitespace().collect::<Vec<_>>().join("+"),
            fields[1].into(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("+");
        let action = if name == "unbind" {
            "Unbind".into()
        } else {
            fields[if count == 5 { 3 } else { 2 }..].join(", ")
        };
        let description = if count == 5 {
            fields[2].to_string()
        } else if let Some((dispatcher, args)) = action.split_once(", ") {
            format!("{}: {args}", humanize(dispatcher))
        } else {
            humanize(&action)
        };
        package.bindings.push(row(
            source,
            line,
            key,
            action,
            format!("{submap}; {name}"),
            description,
        ));
    }
    Ok(())
}

pub fn kde(source: &str, body: &str, package: &mut Package) -> Result<(), String> {
    let mut group = String::new();
    for (i, text) in body.lines().enumerate() {
        let text = text.trim();
        if text.starts_with('[') && text.ends_with(']') {
            group = text[1..text.len() - 1].replace("][", " / ");
            continue;
        }
        if text.starts_with('#') || text.starts_with("_k_") || text.is_empty() {
            continue;
        }
        let Some((action, value)) = text.split_once('=') else {
            continue;
        };
        let mut fields = value.splitn(3, ',');
        let key = fields.next().unwrap_or("");
        let default = fields.next();
        let description = fields
            .next()
            .map(str::to_string)
            .unwrap_or_else(|| humanize(action.trim_start_matches('_')));
        if default.is_none() && !group.starts_with("services / ") {
            return Err(format!("{}: missing default shortcut", i + 1));
        }
        let disabled = key.is_empty() || key == "none";
        package.bindings.push(row(
            source,
            i + 1,
            if disabled {
                "Unbound".into()
            } else {
                key.replace("\\t", " / ")
            },
            action.into(),
            if disabled {
                format!("{group}; disabled")
            } else {
                group.clone()
            },
            description,
        ));
    }
    Ok(())
}

fn jsonc(body: &str) -> Result<String, String> {
    let mut bytes = body.as_bytes().to_vec();
    let mut i = 0;
    let mut string = false;
    while i < bytes.len() {
        if string {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                string = false;
            }
        } else if bytes[i] == b'"' {
            string = true;
        } else if bytes.get(i..i + 2) == Some(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                bytes[i] = b' ';
                i += 1;
            }
            continue;
        } else if bytes.get(i..i + 2) == Some(b"/*") {
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            while i < bytes.len() && bytes.get(i..i + 2) != Some(b"*/") {
                if bytes[i] != b'\n' {
                    bytes[i] = b' ';
                }
                i += 1;
            }
            if i + 1 >= bytes.len() {
                return Err("unclosed JSON comment".into());
            }
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            continue;
        }
        i += 1;
    }
    i = 0;
    string = false;
    while i < bytes.len() {
        if string {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                string = false;
            }
        } else if bytes[i] == b'"' {
            string = true;
        } else if bytes[i] == b','
            && bytes[i + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| matches!(b, b'}' | b']'))
        {
            bytes[i] = b' ';
        }
        i += 1;
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

pub fn vscode(source: &str, body: &str, package: &mut Package) -> Result<(), String> {
    let cleaned = jsonc(body)?;
    let values: Vec<serde_json::Value> =
        serde_json::from_str(&cleaned).map_err(|e| e.to_string())?;
    let mut starts = Vec::new();
    let mut depth = 0;
    let mut string = false;
    let mut escaped = false;
    let mut line = 1;
    for c in cleaned.chars() {
        if c == '\n' {
            line += 1;
        }
        if string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                string = false;
            }
        } else {
            match c {
                '"' => string = true,
                '[' | '{' => {
                    if depth == 1 && c == '{' {
                        starts.push(line);
                    }
                    depth += 1;
                }
                ']' | '}' => depth -= 1,
                _ => {}
            }
        }
    }
    for (index, value) in values.iter().enumerate() {
        let field = |name| {
            value
                .get(name)
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("entry {}: missing {name}", index + 1))
        };
        let key = field("key")?;
        let command = field("command")?;
        let mut action = command.to_string();
        if let Some(args) = value.get("args") {
            action.push(' ');
            action.push_str(&args.to_string());
        }
        let mut context = value
            .get("when")
            .and_then(|v| v.as_str())
            .unwrap_or("global")
            .to_string();
        if command.starts_with('-') {
            context.push_str("; remove binding");
        }
        let description = humanize(
            command
                .trim_start_matches('-')
                .strip_prefix("workbench.action.")
                .unwrap_or(command.trim_start_matches('-')),
        );
        package.bindings.push(row(
            source,
            starts.get(index).copied().unwrap_or(1),
            key.into(),
            action,
            context,
            description,
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Strings {
    One(String),
    Many(Vec<String>),
}
impl Strings {
    fn join(&self, separator: &str) -> String {
        match self {
            Self::One(s) => s.clone(),
            Self::Many(ss) => ss.join(separator),
        }
    }
}

#[derive(Deserialize)]
struct YaziBinding {
    on: Strings,
    run: Strings,
    desc: Option<String>,
}

pub fn yazi(source: &str, body: &str, package: &mut Package) -> Result<(), String> {
    type Keymap = BTreeMap<String, BTreeMap<String, Vec<toml::Spanned<YaziBinding>>>>;
    let keymap: Keymap = toml::from_str(body).map_err(|e| e.to_string())?;
    for (mode, tables) in keymap {
        for (table, values) in tables {
            for value in values {
                let line = body[..value.span().start]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
                    + 1;
                let value = value.into_inner();
                let action = value.run.join("; ");
                let description = value.desc.unwrap_or_else(|| humanize(&action));
                package.bindings.push(row(
                    source,
                    line,
                    value.on.join(" → "),
                    action,
                    format!("{mode}; {table}"),
                    description,
                ));
            }
        }
    }
    Ok(())
}
