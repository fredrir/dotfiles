use dotfmt_core::repair::{Edit, Failure, Repair, Repaired, separator, settle};
use toml_edit::ImDocument;

pub fn repair(input: &str) -> Repaired {
    settle(input, failure, fix)
}

fn failure(text: &str) -> Option<Failure> {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom = text.len() - body.len();
    let error = ImDocument::parse(body).err()?;
    Some(Failure {
        at: bom + error.span().map_or(body.len(), |span| span.start),
        message: error.message().to_string(),
    })
}

fn fix(text: &str, failure: &Failure) -> Vec<(Repair, Edit)> {
    let closer = if failure.message.starts_with("invalid array") {
        ']'
    } else if failure.message.starts_with("invalid inline table") {
        '}'
    } else {
        return Vec::new();
    };
    let at = failure.at;
    let rest = &text[at..];
    if let Some(after) = rest.strip_prefix(',') {
        if after.trim_start().starts_with(closer) {
            return vec![(Repair::Comma, Edit::delete(at, at + 1))];
        }
        return Vec::new();
    }
    if rest.trim().is_empty() || rest.starts_with(closer) {
        return Vec::new();
    }
    vec![(
        Repair::MissingComma,
        Edit::insert(separator(text, at, "#"), ","),
    )]
}
