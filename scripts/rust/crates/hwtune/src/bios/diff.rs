use similar::TextDiff;

use crate::bios::export::{Change, Export};

pub fn unified(before: &str, after: &str, names: (&str, &str)) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(1)
        .header(names.0, names.1)
        .to_string()
}

pub fn summary(changes: &[Change], before: &Export) -> Vec<Vec<String>> {
    changes
        .iter()
        .map(|change| {
            let duplicates = before.occurrences(&change.name).len().max(1);
            let name = if duplicates > 1 || change.occurrence > 1 {
                format!("{}#{}", change.name, change.occurrence)
            } else {
                change.name.clone()
            };
            vec![
                name,
                change.from.clone().unwrap_or_else(|| "(absent)".into()),
                change.to.clone().unwrap_or_else(|| "(absent)".into()),
            ]
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/unit/bios/diff_tests.rs"]
mod tests;
