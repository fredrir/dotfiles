use ui_terminal::text::width;

pub fn render(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths = headers
        .iter()
        .enumerate()
        .map(|(column, title)| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| width(cell))
                .max()
                .unwrap_or(0)
                .max(width(title))
        })
        .collect::<Vec<_>>();
    let line = |cells: Vec<&str>| {
        let text = cells
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                format!(
                    "{cell}{}",
                    " ".repeat(widths[column].saturating_sub(width(cell)))
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
