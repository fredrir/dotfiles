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
        return Ok("Withheld\nContents withheld: encrypt or remove this file.".into());
    }
    if source.sha256.is_empty() {
        return Ok("Unavailable\nNot text or over 2 MiB.".into());
    }
    let bytes = zeroize::Zeroizing::new(source_bytes(context, source)?);
    if bytes.len() > MAX_BYTES || bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Ok("Unavailable\nBinary content or over 2 MiB.".into());
    }
    let text = zeroize::Zeroizing::new(String::from_utf8_lossy(&bytes).into_owned());
    let masked = zeroize::Zeroizing::new(patterns::redact_with_private(
        &text,
        &canaries::ranges(&text, canaries)?,
    )?);
    let lines = masked.lines().collect::<Vec<_>>();
    let mut selected = BTreeSet::new();
    let mut matched = BTreeSet::new();
    for finding in findings {
        let line = finding.line.max(1);
        matched.insert(line);
        selected.extend(line.saturating_sub(2).max(1)..=(line + 2).min(lines.len()));
    }
    let pages = selected.len().div_ceil(60).max(1);
    let page = page % pages;
    let mut output = format!("Inspection {}/{} masked\n", page + 1, pages);
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
        output.push_str("i next page\n");
    }
    Ok(output)
}
