use crate::bios::export::Export;
use crate::bios::spec::Spec;
use crate::rows::Row;

pub fn compare(spec: &Spec, export: &Export) -> Vec<Row> {
    spec.expectations
        .iter()
        .map(|expectation| {
            let label = match expectation.occurrence {
                Some(occurrence) => format!("{}#{occurrence}", expectation.name),
                None => expectation.name.clone(),
            };
            let found = export.occurrences(&expectation.name);
            if found.is_empty() {
                return Row::bad(label, "not in export");
            }
            match expectation.occurrence {
                Some(occurrence) => match found.get(occurrence - 1) {
                    None => Row::bad(
                        label,
                        format!(
                            "occurrence {occurrence} missing, export has {}",
                            found.len()
                        ),
                    ),
                    Some(setting) if setting.value == expectation.value => {
                        Row::ok(label, expectation.value.clone())
                    }
                    Some(setting) => Row::bad(
                        label,
                        format!(
                            "{} (line {}), spec says {}",
                            setting.value, setting.line, expectation.value
                        ),
                    ),
                },
                None => {
                    let mismatches = found
                        .iter()
                        .filter(|setting| setting.value != expectation.value)
                        .map(|setting| format!("line {}: {}", setting.line, setting.value))
                        .collect::<Vec<_>>();
                    if mismatches.is_empty() {
                        Row::ok(label, expectation.value.clone())
                    } else {
                        Row::bad(label, format!("spec says {}", expectation.value))
                            .with_details(mismatches)
                    }
                }
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/unit/bios/check_tests.rs"]
mod tests;
