use dotfmt_core::repair::{Edit, Failure, Repair, Repaired, settle};
use saphyr_parser::Parser;

/// Tab indentation, when no indented line uses spaces to say otherwise.
pub fn repair(input: &str) -> Repaired {
    settle(input, failure, fix)
}

fn failure(text: &str) -> Option<Failure> {
    let mut parser = Parser::new_from_str(text);
    while let Some(next) = parser.next_event() {
        if let Err(error) = next {
            return Some(Failure {
                at: error.marker().index(),
                message: error.info().to_string(),
            });
        }
    }
    None
}

fn fix(text: &str, failure: &Failure) -> Vec<(Repair, Edit)> {
    if !failure.message.contains("tab") {
        return Vec::new();
    }
    let mut fixes = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_start_matches([' ', '\t']);
        let indent = &line[..line.len() - body.len()];
        if !body.trim().is_empty() {
            // Spaces anywhere in the indentation leave the width of a tab a guess.
            if indent.contains(' ') {
                return Vec::new();
            }
            if !indent.is_empty() {
                fixes.push((
                    Repair::Indent,
                    Edit::replace(start, start + indent.len(), "  ".repeat(indent.len())),
                ));
            }
        }
        start += line.len();
    }
    fixes
}
