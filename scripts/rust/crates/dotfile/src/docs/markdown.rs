pub fn block(name: &str, body: &str) -> String {
    format!("<!-- {name}:start -->\n{body}\n<!-- {name}:end -->")
}

pub fn replace_block(text: &str, name: &str, body: &str) -> Result<String, String> {
    let start = format!("<!-- {name}:start -->");
    let end = format!("<!-- {name}:end -->");
    if text.matches(&start).count() != 1 || text.matches(&end).count() != 1 {
        return Err(format!("{name}: expected one start/end marker pair"));
    }
    let (head, rest) = text.split_once(&start).unwrap();
    let (_, tail) = rest
        .split_once(&end)
        .ok_or_else(|| format!("{name}: end marker precedes start"))?;
    Ok(format!("{head}{}{tail}", block(name, body)))
}

pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let rows = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| cell.replace('|', "\\|").replace(['\n', '\r'], " "))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let widths = (0..headers.len())
        .map(|i| {
            rows.iter()
                .map(|row| row[i].chars().count())
                .chain(std::iter::once(headers[i].chars().count().max(3)))
                .max()
                .unwrap_or(3)
        })
        .collect::<Vec<_>>();
    let line = |row: &[String]| {
        format!(
            "| {} |",
            row.iter()
                .zip(&widths)
                .map(|(cell, width)| format!(
                    "{cell}{}",
                    " ".repeat(width.saturating_sub(cell.chars().count()))
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };
    let mut lines = vec![
        line(&headers.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        line(&widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>()),
    ];
    lines.extend(rows.iter().map(|row| line(row)));
    lines.join("\n")
}

#[cfg(test)]
#[path = "../../tests/unit/docs/markdown_tests.rs"]
mod tests;
