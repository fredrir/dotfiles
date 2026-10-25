// Just enough zsh syntax to see what feeds a command's standard input, without running anything.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Word(String),
    Op(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    File(String),
    Text(String),
}

const OPERATORS: [&str; 18] = [
    "<<<", "&&", "||", "|&", "&|", "&!", "&>", ">>", ">|", ">&", "<&", "<>", "<<", "|", "&", ";",
    "<", ">",
];
const SEPARATORS: [&str; 8] = ["|", "|&", "||", "&&", ";", "&", "&|", "&!"];
const PRECOMMANDS: [&str; 6] = ["command", "builtin", "noglob", "nocorrect", "exec", "-"];
const REDIRECTIONS: [&str; 10] = ["<", "<>", "<<<", "<<", "<&", ">", ">>", ">|", ">&", "&>"];

pub fn tokens(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let finish = |tokens: &mut Vec<Token>, word: &mut String, quoted: &mut bool| {
        if !word.is_empty() || *quoted {
            tokens.push(Token::Word(std::mem::take(word)));
        }
        *quoted = false;
    };
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => finish(&mut tokens, &mut word, &mut quoted),
            '\n' => {
                finish(&mut tokens, &mut word, &mut quoted);
                tokens.push(Token::Op(";"));
            }
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(next) => word.push(next),
            },
            '\'' => {
                quoted = true;
                for next in chars.by_ref() {
                    if next == '\'' {
                        break;
                    }
                    word.push(next);
                }
            }
            '"' => {
                quoted = true;
                while let Some(next) = chars.next() {
                    match next {
                        '"' => break,
                        '\\' => match chars.peek() {
                            Some(&escaped @ ('$' | '`' | '"' | '\\')) => {
                                word.push(escaped);
                                chars.next();
                            }
                            Some('\n') => {
                                chars.next();
                            }
                            _ => word.push('\\'),
                        },
                        _ => word.push(next),
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                quoted = true;
                ansi_c(&mut chars, &mut word);
            }
            '$' if chars.peek() == Some(&'(') => {
                word.push('$');
                substitution(&mut chars, &mut word);
            }
            '`' => {
                word.push('`');
                for next in chars.by_ref() {
                    word.push(next);
                    if next == '`' {
                        break;
                    }
                }
            }
            '(' | ')' if word.is_empty() && !quoted => {
                tokens.push(Token::Op(if c == '(' { "(" } else { ")" }));
            }
            '|' | '&' | ';' | '<' | '>' => {
                let rest: String = std::iter::once(c).chain(chars.clone().take(2)).collect();
                let op = OPERATORS
                    .iter()
                    .find(|op| rest.starts_with(**op))
                    .copied()
                    .unwrap_or(";");
                // A number right before a redirection names a file descriptor.
                if matches!(c, '<' | '>') && !quoted && word.chars().all(|c| c.is_ascii_digit()) {
                    word.clear();
                }
                finish(&mut tokens, &mut word, &mut quoted);
                for _ in 1..op.len() {
                    chars.next();
                }
                tokens.push(Token::Op(op));
            }
            _ => word.push(c),
        }
    }
    finish(&mut tokens, &mut word, &mut quoted);
    tokens
}

// One shell word with its quoting removed; an unterminated quote ends with the word.
pub fn unquote(raw: &str) -> String {
    tokens(raw)
        .into_iter()
        .filter_map(|token| match token {
            Token::Word(word) => Some(word),
            Token::Op(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// Inputs piped or redirected into the command whose words end `before` and continue in `after`.
pub fn stdin(before: &str, after: &str) -> Vec<Input> {
    let before = tokens(before);
    let mut stages: Vec<&[Token]> = before
        .split(|token| matches!(token, Token::Op(op) if SEPARATORS.contains(op)))
        .collect();
    let own = stages.pop().unwrap_or_default();
    let piped = before
        .iter()
        .rev()
        .find_map(|token| match token {
            Token::Op(op) if SEPARATORS.contains(op) => Some(*op),
            _ => None,
        })
        .is_some_and(|op| op == "|" || op == "|&");
    let mut inputs = Vec::new();
    if piped && let Some(upstream) = stages.pop() {
        inputs.extend(producer(upstream));
    }
    let after = tokens(after);
    let rest = after
        .split(|token| matches!(token, Token::Op(op) if SEPARATORS.contains(op)))
        .next()
        .unwrap_or_default();
    inputs.extend(redirections(own));
    inputs.extend(redirections(rest));
    inputs
}

fn producer(stage: &[Token]) -> Vec<Input> {
    let (words, redirected) = split_redirections(stage);
    let mut words = words
        .into_iter()
        .skip_while(|word| is_assignment(word) || PRECOMMANDS.contains(&word.as_str()));
    let Some(command) = words.next() else {
        return redirected;
    };
    let args: Vec<String> = words.collect();
    match command.rsplit('/').next().unwrap_or(&command) {
        "cat" => {
            let mut options = true;
            let files: Vec<Input> = args
                .into_iter()
                .filter(|arg| {
                    if options && arg == "--" {
                        options = false;
                        return false;
                    }
                    !(options && arg.starts_with('-'))
                })
                .map(Input::File)
                .collect();
            if files.is_empty() { redirected } else { files }
        }
        "echo" | "print" => {
            let text: Vec<String> = args
                .into_iter()
                .skip_while(|arg| matches!(arg.as_str(), "-n" | "-e" | "-E" | "-r"))
                .collect();
            vec![Input::Text(format!("{}\n", text.join(" ")))]
        }
        _ => Vec::new(),
    }
}

fn redirections(stage: &[Token]) -> Vec<Input> {
    split_redirections(stage).1
}

fn split_redirections(stage: &[Token]) -> (Vec<String>, Vec<Input>) {
    let mut words = Vec::new();
    let mut inputs = Vec::new();
    let mut iter = stage.iter();
    while let Some(token) = iter.next() {
        match token {
            Token::Word(word) => words.push(word.clone()),
            Token::Op(op) if REDIRECTIONS.contains(op) => {
                let Some(Token::Word(target)) = iter.next() else {
                    continue;
                };
                match *op {
                    "<" | "<>" => inputs.push(Input::File(target.clone())),
                    "<<<" => inputs.push(Input::Text(format!("{target}\n"))),
                    _ => {}
                }
            }
            Token::Op(_) => {}
        }
    }
    (words, inputs)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn ansi_c(chars: &mut std::iter::Peekable<std::str::Chars>, word: &mut String) {
    while let Some(next) = chars.next() {
        match next {
            '\'' => return,
            '\\' => {
                let Some(escaped) = chars.next() else { return };
                match escaped {
                    'n' => word.push('\n'),
                    't' => word.push('\t'),
                    'r' => word.push('\r'),
                    'e' | 'E' => word.push('\x1b'),
                    'a' => word.push('\x07'),
                    'b' => word.push('\x08'),
                    'f' => word.push('\x0c'),
                    'v' => word.push('\x0b'),
                    'x' | 'u' | 'U' => {
                        let width = match escaped {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let mut digits = String::new();
                        while digits.len() < width
                            && let Some(digit) = chars.peek().filter(|c| c.is_ascii_hexdigit())
                        {
                            digits.push(*digit);
                            chars.next();
                        }
                        match u32::from_str_radix(&digits, 16)
                            .ok()
                            .and_then(char::from_u32)
                        {
                            Some(decoded) => word.push(decoded),
                            None => {
                                word.push('\\');
                                word.push(escaped);
                                word.push_str(&digits);
                            }
                        }
                    }
                    '\\' | '\'' | '"' => word.push(escaped),
                    other => {
                        word.push('\\');
                        word.push(other);
                    }
                }
            }
            _ => word.push(next),
        }
    }
}

fn substitution(chars: &mut std::iter::Peekable<std::str::Chars>, word: &mut String) {
    let mut depth = 0usize;
    for next in chars.by_ref() {
        word.push(next);
        match next {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/jq/shell_tests.rs"]
mod tests;
