use dotfmt_core::repair::{Edit, Failure, Repair, Repaired, Repairs, separator, settle};

use crate::dialect::Dialect;

pub fn repair(input: &str, dialect: Dialect) -> Repaired {
    if !matches!(dialect, Dialect::Luau | Dialect::All) && input.contains('`') {
        return Repaired {
            text: input.to_string(),
            repairs: Repairs::default(),
        };
    }
    let version = dialect.syntax().into();
    settle(input, |text| failure(text, version), fix)
}

fn failure(text: &str, version: full_moon::LuaVersion) -> Option<Failure> {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom = text.len() - body.len();
    let parsed = full_moon::parse_fallible(body, version);
    let error = parsed.errors().first()?;
    Some(Failure {
        at: bom + error.range().0.bytes(),
        message: error.error_message().into_owned(),
    })
}

fn fix(text: &str, failure: &Failure) -> Vec<(Repair, Edit)> {
    if !failure.message.contains("expected `}` after last field") {
        return Vec::new();
    }
    vec![(
        Repair::MissingComma,
        Edit::insert(separator(text, failure.at, "--"), ","),
    )]
}
