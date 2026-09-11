use regex::Regex;
use std::sync::LazyLock;

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
    Regex::new(
    r#"(?i)\b(api[_-]?key|access[_-]?token|auth[_-]?token|refresh[_-]?token|client[_-]?secret|secret[_-]?key|password|passwd)\b(\s*[:=]\s*)("[^"\n]{8,}"|'[^'\n]{8,}'|[^\s]{8,})"#
).expect("valid value pattern")
});

pub fn redact_patterns(text: &str) -> String {
    let mut output = text.to_string();
    for (label, pattern) in TOKENS.iter() {
        output = pattern
            .replace_all(&output, format!("[redacted:{label}]"))
            .into_owned();
    }
    VALUE
        .replace_all(&output, "${1}${2}[redacted:value]")
        .into_owned()
}
