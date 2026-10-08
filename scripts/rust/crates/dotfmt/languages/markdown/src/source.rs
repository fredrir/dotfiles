use comrak::{
    Arena, Options,
    nodes::{AstNode, ListType, NodeValue, Sourcepos},
    parse_document,
};

use crate::{config::Config, obsidian::Protected, tables};

pub fn format(
    input: &str,
    config: &Config,
    options: &Options<'_>,
    protected: Option<&Protected>,
) -> Result<String, String> {
    let arena = Arena::new();
    let root = parse_document(&arena, input, options);
    let mut lines: Vec<String> = input.lines().map(str::to_owned).collect();
    normalize_inlines(root, &mut lines, options, input.ends_with('\n'));
    let mut removed = vec![false; lines.len()];
    let mut spacing = vec![None; lines.len()];
    let mut closing_fence = None;
    for node in root.descendants() {
        let data = node.data.borrow();
        let pos = data.sourcepos;
        if pos.start.line == 0 || pos.end.line == 0 || pos.end.line > lines.len() {
            continue;
        }
        match &data.value {
            NodeValue::CodeBlock(code)
                if code.fenced
                    && !code.closed
                    && code.literal.ends_with('\n')
                    && pos.end.line == lines.len() =>
            {
                // Closing an unfinished fence keeps code's terminal newlines
                // inside the block when final_newline=false trims the file end.
                let prefix = lines[pos.start.line - 1]
                    .get(..pos.start.column.saturating_sub(1))
                    .unwrap_or("")
                    .chars()
                    .map(|c| if c == '>' { '>' } else { ' ' })
                    .collect::<String>();
                closing_fence = Some(format!(
                    "{prefix}{}",
                    (code.fence_char as char)
                        .to_string()
                        .repeat(code.fence_length)
                ));
            }
            NodeValue::Table(table) => {
                let start = pos.start.line - 1;
                let end = pos.end.line;
                // Comrak reports the header's column for some continuation
                // rows even when their indentation differs. Read each actual
                // continuation prefix rather than slicing at that column.
                let mut columns: Vec<_> = lines[start..end]
                    .iter()
                    .map(|line| line.len() - line.trim_start_matches([' ', '\t', '>']).len())
                    .collect();
                columns[0] = pos.start.column.saturating_sub(1);
                tables::format(
                    &mut lines[start..end],
                    &columns,
                    &table.alignments,
                    config,
                    protected,
                )?;
            }
            NodeValue::List(list) => {
                // Different markers can deliberately separate adjacent lists.
                // Keep them instead of merging lists or injecting HTML comments.
                let adjacent_list = [node.previous_sibling(), node.next_sibling()]
                    .into_iter()
                    .flatten()
                    .any(|sibling| matches!(sibling.data.borrow().value, NodeValue::List(_)));
                if list.list_type == ListType::Bullet && !adjacent_list {
                    for item in node.children() {
                        let p = item.data.borrow().sourcepos.start;
                        let col = p.column.saturating_sub(1);
                        let line = &mut lines[p.line - 1];
                        if line.as_bytes().get(col) == Some(&list.bullet_char) {
                            line.replace_range(
                                col..col + 1,
                                &(config.list_marker as u8 as char).to_string(),
                            );
                        }
                    }
                }
                if simple_list(node) {
                    let items: Vec<_> = node.children().collect();
                    let prefix = blank_prefix(&lines[pos.start.line - 1], pos.start.column);
                    for pair in items.windows(2) {
                        let end = pair[0].data.borrow().sourcepos.end.line;
                        let next = pair[1].data.borrow().sourcepos.start.line - 1;
                        for index in end..next {
                            if blank(&lines[index], &prefix) {
                                removed[index] = true;
                            }
                        }
                    }
                }
            }
            NodeValue::Heading(_) => {
                let start = pos.start.line - 1;
                let end = pos.end.line - 1;
                let prefix = blank_prefix(&lines[start], pos.start.column);
                spacing[end] = Some(prefix.clone());
                // Collapse excess spacing before a heading, without touching
                // quoted or indented content from another container.
                let mut before = start;
                while before > 0 && blank(&lines[before - 1], &prefix) {
                    before -= 1;
                }
                for remove in &mut removed[before..start.saturating_sub(1).max(before)] {
                    *remove = true;
                }
            }
            _ => {}
        }
    }
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < lines.len() {
        if !removed[index] {
            output.push_str(&lines[index]);
            output.push('\n');
        }
        if let Some(prefix) = &spacing[index] {
            index += 1;
            let blank_start = index;
            while index < lines.len() && blank(&lines[index], prefix) {
                index += 1;
            }
            if index < lines.len() {
                for _ in 0..config.heading_blank_lines {
                    output.push_str(prefix);
                    output.push('\n');
                }
            } else if !config.trim_trailing_blank_lines {
                for line in &lines[blank_start..] {
                    output.push_str(line);
                    output.push('\n');
                }
            }
        } else {
            index += 1;
        }
    }
    if let Some(fence) = closing_fence {
        output.push_str(&fence);
        output.push('\n');
    }
    Ok(output)
}

fn blank(line: &str, prefix: &str) -> bool {
    line.trim().is_empty() || line.trim_end() == prefix
}

fn blank_prefix(line: &str, column: usize) -> String {
    line.get(..column.saturating_sub(1))
        .unwrap_or("")
        .chars()
        .map(|c| if c == '>' { '>' } else { ' ' })
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn simple_list<'a>(node: &'a AstNode<'a>) -> bool {
    node.children().all(|item| {
        let mut paragraphs = 0;
        item.children()
            .all(|block| match block.data.borrow().value {
                NodeValue::Paragraph => {
                    paragraphs += 1;
                    paragraphs <= 1
                }
                NodeValue::List(_) => true,
                _ => false,
            })
    })
}

// All replacements retain their byte length, so the original AST positions
// remain valid for subsequent table and block edits. Reparse only when inline
// edits were proposed; ambiguous delimiter combinations retain their source.
fn normalize_inlines<'a>(
    root: &'a AstNode<'a>,
    lines: &mut Vec<String>,
    options: &Options<'_>,
    final_newline: bool,
) {
    let mut candidate = lines.clone();
    let mut changed = false;
    for node in root.descendants() {
        let data = node.data.borrow();
        let pos = data.sourcepos;
        let count = match data.value {
            NodeValue::Emph => 1,
            NodeValue::Strong => 2,
            NodeValue::Link(_) | NodeValue::Image(_) => {
                if pos.start.line == pos.end.line
                    && let Some((line, start, end)) = span(&mut candidate, pos)
                {
                    let text = &line[start..end];
                    // Only normalize explicitly quoted inline titles. References
                    // and their definitions stay exactly where the author put them.
                    if text.ends_with("')")
                        && let Some(open) = text.rfind(" '")
                        && !text[open + 2..text.len() - 2].contains(['"', '\\'])
                    {
                        let open = start + open + 1;
                        line.replace_range(open..open + 1, "\"");
                        line.replace_range(end - 2..end - 1, "\"");
                        changed = true;
                    }
                }
                continue;
            }
            _ => continue,
        };
        let start_line = pos.start.line.saturating_sub(1);
        let end_line = pos.end.line.saturating_sub(1);
        let start = pos.start.column.saturating_sub(1);
        let end = pos.end.column;
        let delimiter = "_".repeat(count);
        if candidate
            .get(start_line)
            .and_then(|l| l.get(start..start + count))
            == Some(delimiter.as_str())
            && end >= count
            && candidate
                .get(end_line)
                .and_then(|l| l.get(end - count..end))
                == Some(delimiter.as_str())
        {
            let replacement = "*".repeat(count);
            candidate[start_line].replace_range(start..start + count, &replacement);
            candidate[end_line].replace_range(end - count..end, &replacement);
            changed = true;
        }
    }
    if changed {
        let arena = Arena::new();
        let mut text = candidate.join("\n");
        if final_newline {
            text.push('\n');
        }
        let parsed = parse_document(&arena, &text, options);
        if same_tree(root, parsed) {
            *lines = candidate;
        }
    }
}

fn span(lines: &mut [String], pos: Sourcepos) -> Option<(&mut String, usize, usize)> {
    let line = lines.get_mut(pos.start.line.checked_sub(1)?)?;
    let start = pos.start.column.checked_sub(1)?;
    let end = pos.end.column;
    line.get(start..end)?;
    Some((line, start, end))
}

fn same_tree<'a, 'b>(left: &'a AstNode<'a>, right: &'b AstNode<'b>) -> bool {
    let mut left = left.descendants();
    let mut right = right.descendants();
    loop {
        match (left.next(), right.next()) {
            (Some(a), Some(b))
                if a.data.borrow().value == b.data.borrow().value
                    && a.children().count() == b.children().count() => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}
