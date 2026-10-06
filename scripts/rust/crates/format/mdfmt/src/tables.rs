use comrak::nodes::TableAlignment;
use unicode_width::UnicodeWidthStr;

use crate::config::{Config, TableStyle};
use crate::obsidian::Protected;

pub fn format(
    lines: &mut [String],
    alignments: &[TableAlignment],
    config: &Config,
    protected: Option<&Protected>,
) -> Result<(), String> {
    let rows: Vec<_> = lines
        .iter()
        .map(|line| split_row(line))
        .collect::<Result<_, _>>()?;
    if rows.len() < 2
        || rows
            .iter()
            .any(|(_, cells)| cells.len() != alignments.len())
    {
        return Err("could not safely lay out a Markdown table".into());
    }
    let mut widths: Vec<_> = alignments.iter().map(|a| separator(*a, 3).len()).collect();
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
                    let minimum = separator(alignments[column], 3).len();
                    let width = if align { widths[column] } else { minimum };
                    output.push_str(&separator(alignments[column], width));
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

fn split_row(line: &str) -> Result<(&str, Vec<&str>), String> {
    let start = line.find('|').ok_or("missing table row delimiter")?;
    let mut cells = Vec::new();
    let mut beginning = start + 1;
    let mut escaped = false;
    for (offset, byte) in line.bytes().enumerate().skip(beginning) {
        if byte == b'|' && !escaped {
            cells.push(line[beginning..offset].trim());
            beginning = offset + 1;
        }
        escaped = byte == b'\\' && !escaped;
    }
    if !line[beginning..].trim().is_empty() {
        return Err("missing closing table row delimiter".into());
    }
    Ok((&line[..start], cells))
}
