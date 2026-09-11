use unicode_width::UnicodeWidthStr;

pub fn render(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths = headers
        .iter()
        .enumerate()
        .map(|(column, title)| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.width())
                .max()
                .unwrap_or(0)
                .max(title.width())
        })
        .collect::<Vec<_>>();
    let line = |cells: Vec<&str>| {
        let text = cells
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                format!(
                    "{cell}{}",
                    " ".repeat(widths[column].saturating_sub(cell.width()))
                )
            })
            .collect::<Vec<_>>()
            .join("  ");
        format!("{}\n", text.trim_end())
    };
    let mut out = line(headers.to_vec());
    for row in rows {
        out.push_str(&line(row.iter().map(String::as_str).collect()));
    }
    out
}

#[cfg(test)]
#[path = "../tests/unit/table_tests.rs"]
mod tests;
