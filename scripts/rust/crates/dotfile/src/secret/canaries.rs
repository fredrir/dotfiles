use super::vault;
use crate::context::Context;
use aho_corasick::AhoCorasick;
use std::collections::BTreeSet;
use std::io::{BufRead, Read, Write};
use std::ops::Range;
use zeroize::Zeroize;

pub struct Canary {
    pub label: String,
    pub needle: String,
}
impl Drop for Canary {
    fn drop(&mut self) {
        self.needle.zeroize();
    }
}

pub fn load(context: &Context) -> Result<(Vec<Canary>, Vec<String>), String> {
    let mut values = Vec::new();
    let mut notes = Vec::new();
    let path = context.root_config.join("canaries");
    if path.is_file() {
        if vault::mode_of(&path)? & 0o077 != 0 {
            notes.push(format!(
                "canaries file is readable beyond you: chmod 600 {}",
                path.display()
            ));
        }
        let text = zeroize::Zeroizing::new(
            String::from_utf8(vault::read_source(&path, super::sops::MAX_SECRET_BYTES)?)
                .map_err(|_| "canaries file is not text")?,
        );
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let (label, value) = line
                .split_once('=')
                .map(|(a, b)| (a.trim(), b.trim()))
                .unwrap_or(("private", line));
            if value.chars().count() < 6 {
                notes.push(format!("canary too short to match usefully: {label}"));
                continue;
            }
            values.push(Canary {
                label: label.to_string(),
                needle: value.to_lowercase(),
            });
        }
    }
    let variables = vault::load_variables(context);
    if !variables.note.is_empty() {
        notes.push(variables.note.clone());
    }
    let mut seen: BTreeSet<_> = values.iter().map(|v| v.needle.clone()).collect();
    for (name, value) in &variables.values {
        let lowered = value.to_lowercase();
        if !name.starts_with("open.") && value.chars().count() >= 6 && seen.insert(lowered.clone())
        {
            values.push(Canary {
                label: name.clone(),
                needle: lowered,
            });
        }
    }
    Ok((values, notes))
}

pub fn matcher(values: &[Canary]) -> Result<Option<AhoCorasick>, String> {
    if values.is_empty() {
        return Ok(None);
    }
    AhoCorasick::new(values.iter().map(|v| &v.needle))
        .map(Some)
        .map_err(|e| e.to_string())
}

pub(super) struct Matcher {
    matcher: Option<AhoCorasick>,
}

impl Matcher {
    pub(super) fn new(values: &[Canary]) -> Result<Self, String> {
        let needles: BTreeSet<_> = values.iter().map(|value| value.needle.as_str()).collect();
        if needles.contains("") {
            return Err("empty private-value matcher".into());
        }
        Ok(Self {
            matcher: if needles.is_empty() {
                None
            } else {
                Some(AhoCorasick::new(needles).map_err(|error| error.to_string())?)
            },
        })
    }

    pub(super) fn ranges(&self, text: &str) -> Result<Vec<Range<usize>>, String> {
        let Some(matcher) = &self.matcher else {
            return Ok(Vec::new());
        };
        let lowered = zeroize::Zeroizing::new(text.to_lowercase());
        let mut ranges = Vec::new();
        for found in matcher.find_overlapping_iter(lowered.as_bytes()) {
            if ranges.len() == super::patterns::MAX_REDACTION_MATCHES {
                return Err("too many private-value matches to redact safely".into());
            }
            ranges.push(found.range());
        }
        if text.is_ascii() || ranges.is_empty() {
            return Ok(ranges);
        }

        // Project only matched endpoints, so metadata stays bounded even for large Unicode input.
        // Full-string lowercasing has contextual sigma forms, but their byte lengths agree with
        // char::to_lowercase. Expanded characters map back to the entire original character.
        let mut endpoints = ranges
            .iter()
            .enumerate()
            .flat_map(|(index, range)| [(range.start, index, false), (range.end, index, true)])
            .collect::<Vec<_>>();
        endpoints.sort_unstable_by_key(|endpoint| endpoint.0);
        let mut next = 0;
        let mut lowered_start = 0;
        for (original_start, character) in text.char_indices() {
            let original_end = original_start + character.len_utf8();
            let lowered_end =
                lowered_start + character.to_lowercase().map(char::len_utf8).sum::<usize>();
            while let Some(&(offset, index, is_end)) = endpoints.get(next) {
                if offset > lowered_end {
                    break;
                }
                if offset < lowered_start {
                    return Err("cannot safely project private-value match".into());
                }
                let original = if offset == lowered_start {
                    original_start
                } else if offset == lowered_end || is_end {
                    original_end
                } else {
                    original_start
                };
                if is_end {
                    ranges[index].end = original;
                } else {
                    ranges[index].start = original;
                }
                next += 1;
            }
            if next == endpoints.len() {
                return Ok(ranges);
            }
            lowered_start = lowered_end;
        }
        Err("cannot safely project private-value match".into())
    }

    fn redact(&self, text: &str) -> Result<String, String> {
        super::patterns::redact_with_private(text, &self.ranges(text)?)
    }
}

pub(super) fn ranges(text: &str, values: &[Canary]) -> Result<Vec<Range<usize>>, String> {
    Matcher::new(values)?.ranges(text)
}

pub fn stream(context: &Context) -> Result<(), String> {
    let (values, notes) = load(context)?;
    for note in notes {
        eprintln!("dotfile: {note}");
    }
    let private = Matcher::new(&values)?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut line = zeroize::Zeroizing::new(Vec::new());
        let count = (&mut input)
            .take(16 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if line.len() > 16 * 1024 * 1024 {
            return Err("redaction input exceeds 16 MiB".into());
        }
        let text: zeroize::Zeroizing<String> = zeroize::Zeroizing::new(
            serde_json::from_slice::<String>(&line)
                .map_err(|_| "redaction input must be a JSON string")?,
        );
        let redacted = private.redact(&text)?;
        serde_json::to_writer(&mut output, &redacted).map_err(|e| e.to_string())?;
        writeln!(output)
            .and_then(|()| output.flush())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(needles: &[&str]) -> Vec<Canary> {
        needles
            .iter()
            .map(|needle| Canary {
                label: "fixture".into(),
                needle: needle.to_lowercase(),
            })
            .collect()
    }

    #[test]
    fn lowercase_expansions_and_contractions_project_to_original_bytes() {
        let matcher =
            Matcher::new(&values(&["prİvate-fixture.example", "kelvin-fixture"])).unwrap();
        for text in [
            "PRİVATE-fixture.example",
            "pri\u{307}vate-fixture.example",
            "KELVIN-fixture",
        ] {
            let input = format!("İK before {text} after");
            let found = matcher.ranges(&input).unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(&input[found[0].clone()], text);
            assert_eq!(
                matcher.redact(&input).unwrap(),
                "İK before [redacted:private] after"
            );
        }
    }

    #[test]
    fn partial_lowercase_expansion_redacts_entire_original_character() {
        let matcher = Matcher::new(&values(&["i", "\u{307}"])).unwrap();
        assert_eq!(matcher.ranges("İ").unwrap(), vec![0..2, 0..2]);
        assert_eq!(matcher.redact("İ").unwrap(), "[redacted:private]");
    }

    #[test]
    fn contextual_lowercase_matches_scanner_semantics() {
        let matcher = Matcher::new(&values(&["ΟΣ"])).unwrap();
        assert_eq!(matcher.redact("İ ΟΣ K").unwrap(), "İ [redacted:private] K");
        assert!(matcher.ranges("ΟΣΑ").unwrap().is_empty());
    }

    #[test]
    fn overlapping_canaries_and_token_patterns_hide_the_full_private_value() {
        let text = "prefix ghp_abcdefghijklmnopqrstuv1234567890 suffix";
        let matcher = Matcher::new(&values(&[
            "prefix ghp_abcdefghijklmnopqrstuv1234567890",
            "ghp_abcdefghijklmnopqrstuv1234567890 suffix",
        ]))
        .unwrap();
        assert_eq!(matcher.redact(text).unwrap(), "[redacted:private]");
    }

    #[test]
    fn duplicate_canaries_do_not_multiply_matches() {
        let matcher = Matcher::new(&values(&["fixture", "fixture"])).unwrap();
        assert_eq!(
            matcher.ranges("fixture fixture").unwrap(),
            vec![0..7, 8..15]
        );
    }

    #[test]
    fn excessive_canary_matches_fail_closed() {
        let matcher = Matcher::new(&values(&["fixture"])).unwrap();
        let text = "fixture ".repeat(super::super::patterns::MAX_REDACTION_MATCHES + 1);
        assert!(matcher.ranges(&text).unwrap_err().contains("too many"));
    }
}
