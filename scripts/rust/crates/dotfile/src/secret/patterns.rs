use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;

pub(super) const MAX_REDACTION_MATCHES: usize = 32_768;

pub static TOKENS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("github-token", r"\bgh[pousr]_[A-Za-z0-9]{20,}\b"),
        ("github-token", r"\bgithub_pat_[A-Za-z0-9_]{20,}\b"),
        ("gitlab-token", r"\bglpat-[A-Za-z0-9_-]{20,}\b"),
        ("npm-token", r"\bnpm_[A-Za-z0-9]{36}\b"),
        ("api-key", r"\bsk-[A-Za-z0-9_-]{16,}\b"),
        ("aws-key", r"\bAKIA[0-9A-Z]{16}\b"),
        ("slack-token", r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b"),
        (
            "jwt",
            r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b",
        ),
        ("age-identity", r"\bAGE-SECRET-KEY-1[0-9A-Z]{50,}\b"),
        (
            "private-key",
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|\z)",
        ),
    ]
    .into_iter()
    .map(|(label, pattern)| (label, Regex::new(pattern).expect("valid token pattern")))
    .collect()
});

pub static VALUE: LazyLock<Regex> = LazyLock::new(|| {
    // Sensitive key names provide the signal; short values still need review.
    Regex::new(
    r#"(?i)\b(api[_-]?key|access[_-]?token|auth[_-]?token|refresh[_-]?token|client[_-]?secret|secret[_-]?key|password|passwd)\b([ \t]*[:=][ \t]*)("[^"\r\n]+"?|'[^'\r\n]+'?|[^\s"'][^\s]*)"#
).expect("valid value pattern")
});

pub(super) fn redact_with_private(text: &str, private: &[Range<usize>]) -> Result<String, String> {
    let mut ranges: Vec<(Range<usize>, &str, u8)> = Vec::new();
    let mut add = |range: Range<usize>, label, priority| -> Result<(), String> {
        if ranges.len() == MAX_REDACTION_MATCHES {
            return Err("too many matches to redact safely".into());
        }
        if range.start >= range.end || text.get(range.clone()).is_none() {
            return Err("invalid redaction match".into());
        }
        ranges.push((range, label, priority));
        Ok(())
    };
    for range in private {
        add(range.clone(), "private", 3)?;
    }
    for (label, pattern) in TOKENS.iter() {
        for found in pattern.find_iter(text) {
            add(
                found.range(),
                label,
                if *label == "private-key" { 2 } else { 0 },
            )?;
        }
    }
    for captures in VALUE.captures_iter(text) {
        if let Some(value) = captures.get(3) {
            add(value.range(), "value", 1)?;
        }
    }
    ranges.sort_unstable_by_key(|(range, _, priority)| (range.start, range.end, *priority));
    let mut merged: Vec<(Range<usize>, &str, u8)> = Vec::new();
    for (range, label, priority) in ranges {
        if let Some((previous, previous_label, previous_priority)) = merged.last_mut()
            && range.start < previous.end
        {
            previous.end = previous.end.max(range.end);
            if priority > *previous_priority {
                *previous_label = label;
                *previous_priority = priority;
            }
        } else {
            merged.push((range, label, priority));
        }
    }
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for (range, label, _) in merged {
        output.push_str(&text[cursor..range.start]);
        output.push_str("[redacted:");
        output.push_str(label);
        output.push(']');
        cursor = range.end;
    }
    output.push_str(&text[cursor..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigned_secrets_have_no_minimum_length_and_are_fully_redacted() {
        for key in ["API_KEY", "password", "client_secret"] {
            for value in [
                "7", "123", "abcdefg", "abcdefgh", "'x'", "\"42\"", "\"x", "'42",
            ] {
                let input = format!("{key}={value}");
                let matched = VALUE.captures(&input).expect("nonempty secret assignment");
                assert_eq!(&matched[3], value);
                assert_eq!(
                    redact_with_private(&input, &[]).unwrap(),
                    format!("{key}=[redacted:value]")
                );
            }
        }
    }

    #[test]
    fn empty_assignments_do_not_consume_quotes_or_the_next_line() {
        for input in [
            "API_KEY=",
            "API_KEY=\"\"",
            "password=''",
            "API_KEY=  \nordinary text",
            "password=\r\nordinary text",
        ] {
            assert!(!VALUE.is_match(input), "{input:?}");
            assert_eq!(redact_with_private(input, &[]).unwrap(), input);
        }
    }

    #[test]
    fn replacement_labels_and_value_prefixes_remain_stable() {
        for (text, expected) in [
            (
                "token ghp_abcdefghijklmnopqrstuv1234567890 done",
                "token [redacted:github-token] done",
            ),
            (
                "export API_KEY=ghp_abcdefghijklmnopqrstuv1234567890",
                "export API_KEY=[redacted:value]",
            ),
            (
                "-----BEGIN RSA PRIVATE KEY-----\nghp_abcdefghijklmnopqrstuv1234567890\n-----END RSA PRIVATE KEY-----",
                "[redacted:private-key]",
            ),
        ] {
            assert_eq!(redact_with_private(text, &[]).unwrap(), expected);
        }
    }

    #[test]
    fn private_spans_merge_with_patterns_and_keep_adjacent_plaintext() {
        let text = "x ghp_abcdefghijklmnopqrstuv1234567890 tail y";
        let private_start = text.find("qrst").unwrap();
        let private_end = text.find(" y").unwrap();
        assert_eq!(
            redact_with_private(text, std::slice::from_ref(&(private_start..private_end))).unwrap(),
            "x [redacted:private] y"
        );
        assert_eq!(
            redact_with_private("password=private-fixture", std::slice::from_ref(&(9..24)))
                .unwrap(),
            "password=[redacted:private]"
        );
    }

    #[test]
    fn excessive_and_invalid_spans_fail_closed() {
        let text = "ghp_abcdefghijklmnopqrstuv1234567890 ".repeat(MAX_REDACTION_MATCHES + 1);
        assert!(
            redact_with_private(&text, &[])
                .unwrap_err()
                .contains("too many")
        );
        assert!(redact_with_private("İ", std::slice::from_ref(&(1..2))).is_err());
        assert!(redact_with_private("fixture", std::slice::from_ref(&(0..8))).is_err());
    }
}
