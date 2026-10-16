pub(crate) struct Subject<'a> {
    pub prefixes: Vec<&'a str>,
    pub message: &'a str,
}

impl<'a> Subject<'a> {
    pub fn parse(subject: &'a str) -> Self {
        let mut message = subject.trim();
        let mut prefixes = Vec::new();
        while prefixes.len() < 3 {
            let Some((prefix, rest)) = leading_prefix(message) else {
                break;
            };
            let rest = rest.trim_start();
            let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
            let rest = rest.strip_prefix("- ").unwrap_or(rest).trim_start();
            if rest.is_empty() || rest.starts_with("//") {
                break;
            }
            prefixes.push(prefix);
            message = rest;
        }
        Self { prefixes, message }
    }
}

fn leading_prefix(subject: &str) -> Option<(&str, &str)> {
    for marker in ["fixup!", "squash!", "amend!"] {
        if let Some(rest) = subject.strip_prefix(marker)
            && rest.starts_with(char::is_whitespace)
        {
            return Some((&subject[..marker.len()], rest));
        }
    }
    if let Some((prefix, rest)) = subject.strip_prefix('[').and_then(|s| s.split_once(']'))
        && (valid_prefix(prefix) || issue_key(prefix))
    {
        return Some((prefix, rest));
    }
    if let Some((prefix, rest)) = subject.split_once(':')
        && (valid_prefix(prefix) || issue_key(prefix))
    {
        return Some((prefix, rest));
    }
    let (prefix, rest) = subject.split_once(char::is_whitespace)?;
    issue_key(prefix).then_some((prefix, rest))
}

fn valid_prefix(prefix: &str) -> bool {
    if prefix.len() > 64 {
        return false;
    }
    let prefix = prefix.strip_suffix('!').unwrap_or(prefix);
    let (kind, scope) = prefix
        .split_once('(')
        .map_or((prefix, None), |(kind, scope)| (kind, Some(scope)));
    kind.starts_with(char::is_alphabetic)
        && kind
            .chars()
            .all(|c| c.is_alphanumeric() || "-_.+/".contains(c))
        && scope.is_none_or(|scope| {
            scope.strip_suffix(')').is_some_and(|scope| {
                !scope.is_empty()
                    && scope
                        .chars()
                        .all(|c| c.is_alphanumeric() || "-_.+/, ".contains(c))
            })
        })
}

fn issue_key(prefix: &str) -> bool {
    let digits = |value: &str| !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit());
    if let Some(number) = prefix.strip_prefix('#') {
        return digits(number);
    }
    let Some((project, number)) = prefix.rsplit_once('-') else {
        return false;
    };
    project.starts_with(|c: char| c.is_ascii_uppercase())
        && project
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        && digits(number)
}
