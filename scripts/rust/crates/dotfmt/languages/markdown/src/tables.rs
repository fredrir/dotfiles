use comrak::nodes::TableAlignment;
use unicode_width::UnicodeWidthStr;

use crate::config::{Config, TableStyle};
use crate::obsidian::Protected;

pub fn format(
    lines: &mut [String],
    columns: &[usize],
    alignments: &[TableAlignment],
    config: &Config,
    protected: Option<&Protected>,
) -> Result<(), String> {
    let mut rows: Vec<_> = lines
        .iter()
        .zip(columns)
        .map(|(line, column)| split_row(line, *column))
        .collect::<Result<_, _>>()?;
    if rows.len() < 2 || rows[0].1.len() != alignments.len() {
        // Uncertain source positions must never cause a partial rewrite.
        return Ok(());
    }
    let count = rows.iter().map(|(_, cells)| cells.len()).max().unwrap_or(0);
    for (_, cells) in &mut rows {
        cells.resize(cells.len().max(alignments.len()), "");
    }
    let mut widths = vec![3; count];
    for (width, alignment) in widths.iter_mut().zip(alignments) {
        *width = separator(*alignment, 3).len();
    }
    let measured: Vec<Vec<usize>> = rows
        .iter()
        .map(|(_, cells)| {
            cells
                .iter()
                .map(|cell| match protected {
                    Some(protected) => protected
                        .restore(cell)
                        .map(|text| UnicodeWidthStr::width(text.as_str())),
                    None => Ok(UnicodeWidthStr::width(*cell)),
                })
                .collect()
        })
        .collect::<Result<_, String>>()?;
    for (row, cells) in measured.iter().enumerate() {
        if row == 1 {
            continue;
        }
        for (width, cell) in widths.iter_mut().zip(cells) {
            *width = (*width).max(*cell);
        }
    }
    let prefix_width = rows
        .iter()
        .map(|(p, _)| UnicodeWidthStr::width(*p))
        .max()
        .unwrap_or(0);
    let aligned_width = prefix_width + 1 + widths.iter().map(|width| width + 3).sum::<usize>();
    let align = match config.table_style {
        TableStyle::Aligned => true,
        TableStyle::Compact => false,
        TableStyle::Auto => config.width == 0 || aligned_width <= config.width,
    };
    let formatted: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(row, (prefix, cells))| {
            let mut output = format!("{prefix}|");
            for (column, cell) in cells.iter().enumerate() {
                output.push(' ');
                if row == 1 {
                    let alignment = alignments
                        .get(column)
                        .copied()
                        .unwrap_or(TableAlignment::None);
                    let minimum = separator(alignment, 3).len();
                    let width = if align && config.autosize_table {
                        widths[column]
                    } else {
                        minimum
                    };
                    output.push_str(&separator(alignment, width));
                } else {
                    output.push_str(cell);
                    if align {
                        output.extend(std::iter::repeat_n(
                            ' ',
                            widths[column] - measured[row][column],
                        ));
                    }
                }
                output.push_str(" |");
            }
            output
        })
        .collect();
    for (line, replacement) in lines.iter_mut().zip(formatted) {
        *line = replacement;
    }
    Ok(())
}

fn separator(alignment: TableAlignment, width: usize) -> String {
    let left = matches!(alignment, TableAlignment::Left | TableAlignment::Center);
    let right = matches!(alignment, TableAlignment::Right | TableAlignment::Center);
    let dashes = width
        .saturating_sub(usize::from(left) + usize::from(right))
        .max(3);
    format!(
        "{}{}{}",
        if left { ":" } else { "" },
        "-".repeat(dashes),
        if right { ":" } else { "" }
    )
}

fn split_row(line: &str, column: usize) -> Result<(&str, Vec<&str>), String> {
    let prefix = line.get(..column).ok_or("invalid table source position")?;
    let text = line
        .get(column..)
        .ok_or("invalid table source position")?
        .trim();
    let mut cells = Vec::new();
    let mut beginning = usize::from(text.starts_with('|'));
    let mut escaped = false;
    let mut final_pipe = false;
    for (offset, byte) in text.bytes().enumerate().skip(beginning) {
        final_pipe = byte == b'|' && !escaped;
        if final_pipe {
            cells.push(text[beginning..offset].trim());
            beginning = offset + 1;
        }
        // GFM's table scanner treats a pipe immediately following a
        // backslash as cell content, including an even backslash run.
        escaped = byte == b'\\';
    }
    if !final_pipe {
        cells.push(text[beginning..].trim());
    }
    Ok((prefix, cells))
}
