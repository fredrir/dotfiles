use super::{Finding, MAX_BYTES, Source, source_bytes};
use crate::context::Context;
use crate::secret::{canaries, patterns};
use std::collections::BTreeSet;

pub(super) fn render(
    context: &Context,
    source: &Source,
    findings: &[&Finding],
    canaries: &[canaries::Canary],
    page: usize,
) -> Result<String, String> {
    if findings.iter().any(|finding| finding.tier == 1) {
        return Ok(
            "Contents withheld: this file must be encrypted or removed from the repository.".into(),
        );
    }
    if source.sha256.is_empty() {
        return Ok("Source is not text or exceeds the 2 MiB inspection limit.".into());
    }
    let bytes = zeroize::Zeroizing::new(source_bytes(context, source)?);
    if bytes.len() > MAX_BYTES || bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Ok("Source is binary or exceeds the 2 MiB inspection limit.".into());
    }
    let text = zeroize::Zeroizing::new(String::from_utf8_lossy(&bytes).into_owned());
    let redacted = redact(&text, canaries)?;
    let lines = redacted.lines().collect::<Vec<_>>();
    let mut selected = BTreeSet::new();
    let mut matched = BTreeSet::new();
    for finding in findings {
        let line = finding.line.max(1);
        matched.insert(line);
        selected.extend(line.saturating_sub(2).max(1)..=(line + 2).min(lines.len()));
    }
    let pages = selected.len().div_ceil(60).max(1);
    let page = page % pages;
    let mut output = format!(
        "Matched values are masked. Context {}/{}.\n",
        page + 1,
        pages
    );
    let mut previous = 0;
    for number in selected.iter().copied().skip(page * 60).take(60) {
        if previous != 0 && number > previous + 1 {
            output.push_str("       …\n");
        }
        let line = lines[number - 1];
        let preview = line.chars().take(240).collect::<String>();
        output.push_str(&format!(
            "{} {number:>5} | {preview}{}\n",
            if matched.contains(&number) { '>' } else { ' ' },
            if line.chars().count() > 240 {
                "…"
            } else {
                ""
            },
        ));
        previous = number;
    }
    if pages > 1 {
        output.push_str("Press i again for the next context page.\n");
    }
    Ok(output)
}

fn redact(text: &str, canaries: &[canaries::Canary]) -> Result<String, String> {
    let mut ranges = Vec::new();
    let mut add = |range| {
        if ranges.len() == patterns::MAX_REDACTION_MATCHES {
            return Err(
                "inspection match limit reached; narrow the source before reviewing".to_string(),
            );
        }
        ranges.push(range);
        Ok(())
    };
    for (_, pattern) in patterns::TOKENS.iter() {
        for matched in pattern.find_iter(text) {
            add(matched.range())?;
        }
    }
    for matched in patterns::VALUE.captures_iter(text) {
        if let Some(value) = matched.get(3) {
            add(value.range())?;
        }
    }
    for range in canaries::ranges(text, canaries)? {
        add(range)?;
    }
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged = Vec::<std::ops::Range<usize>>::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    let mut output = String::new();
    let mut cursor = 0;
    for range in merged {
        output.push_str(&text[cursor..range.start]);
        output.push_str("[redacted]");
        // Preserve source line numbers even when a private-key block spans lines.
        output.extend(
            text[range.clone()]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .map(|_| '\n'),
        );
        cursor = range.end;
    }
    output.push_str(&text[cursor..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking_preserves_lines_and_covers_overlapping_and_unicode_private_values() {
        let token = format!("ghp_{}", "x".repeat(24));
        let key = [
            "-----BEGIN RSA PRIVATE KEY-----",
            "private material",
            "-----END RSA PRIVATE KEY-----",
        ]
        .join("\n");
        let text = format!("API_KEY=\"{token}\"\n{key}\nHOST=PrIvAtE-Æ.Example\nend\n");
        let canaries = vec![canaries::Canary {
            label: "host".into(),
            needle: "private-æ.example".into(),
        }];
        let redacted = redact(&text, &canaries).unwrap();
        assert!(!redacted.contains(&token));
        assert!(!redacted.contains("private material"));
        assert!(!redacted.contains("PrIvAtE-Æ.Example"));
        assert_eq!(redacted.lines().count(), text.lines().count());
        assert_eq!(redacted.lines().last(), Some("end"));
        assert!(redacted.starts_with("API_KEY=[redacted]\n"));
    }
    #[test]
    fn masking_bounds_match_metadata_before_collecting_unlimited_occurrences() {
        let token = format!("ghp_{}\n", "x".repeat(24));
        let text = token.repeat(32_769);
        assert!(redact(&text, &[]).unwrap_err().contains("match limit"));
    }
}
