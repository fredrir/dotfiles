#![forbid(unsafe_code)]

pub mod config;
pub mod dialect;
mod obsidian;
mod tables;

use comrak::{
    Arena, Options, format_commonmark,
    nodes::{NodeHtmlBlock, NodeValue},
    parse_document,
};

use config::Config;
use dialect::Dialect;

pub fn options(config: &Config) -> Options<'static> {
    let mut options = Options::default();
    if config.dialect != Dialect::Commonmark {
        options.extension.table = true;
        options.extension.strikethrough = true;
        options.extension.tasklist = true;
        options.extension.autolink = true;
        options.extension.footnotes = true;
        options.extension.front_matter_delimiter = Some("---".into());
        options.extension.alerts = config.dialect != Dialect::Obsidian;
    }
    if config.dialect == Dialect::Obsidian {
        options.extension.highlight = true;
        options.extension.inline_footnotes = true;
    }
    options.render.width = config.width;
    options.render.list_style = config.list_marker;
    options.render.prefer_fenced = true;
    options
}

pub fn format(input: &str, config: &Config) -> Result<String, String> {
    let protected = (config.dialect == Dialect::Obsidian)
        .then(|| obsidian::Protected::new(input, &options(config)))
        .transpose()?;
    let input = protected
        .as_ref()
        .map_or(input, |protected| protected.text.as_str());
    let mut options = options(config);
    if config.dialect != Dialect::Commonmark
        && (input.starts_with("+++\n") || input.starts_with("+++\r\n"))
    {
        options.extension.front_matter_delimiter = Some("+++".into());
    }
    let arena = Arena::new();
    let root = parse_document(&arena, input, &options);
    if config.heading_blank_lines > 0 {
        for heading in root
            .descendants()
            .filter(|node| matches!(node.data.borrow().value, NodeValue::Heading(_)))
        {
            for ancestor in heading.ancestors().skip(1) {
                if let NodeValue::List(list) = &mut ancestor.data.borrow_mut().value {
                    // Adding blank lines makes a containing list loose. Render
                    // that layout on the first pass as well as subsequent ones.
                    list.tight = false;
                }
            }
        }
    }
    // CommonMark's renderer also wraps table cells. Render those separately
    // without wrapping, then carry their Markdown as opaque blocks through
    // the prose renderer. Container prefixes are still supplied by Comrak.
    let tables: Vec<_> = root
        .descendants()
        .filter(|node| matches!(node.data.borrow().value, NodeValue::Table(_)))
        .collect();
    let mut table_options = options.clone();
    table_options.render.width = 0;
    for table in tables {
        let mut literal = String::new();
        format_commonmark(table, &table_options, &mut literal).map_err(|e| e.to_string())?;
        while let Some(child) = table.first_child() {
            child.detach();
        }
        table.data.borrow_mut().value = NodeValue::HtmlBlock(NodeHtmlBlock {
            block_type: 1,
            literal,
        });
    }
    let mut rendered = String::with_capacity(input.len());
    format_commonmark(root, &options, &mut rendered)
        .map_err(|error| format!("could not render Markdown: {error}"))?;

    // Source positions from the normalized document identify real tables and
    // headings, including those inside containers, without touching code/HTML.
    let normalized_arena = Arena::new();
    let normalized = parse_document(&normalized_arena, &rendered, &options);
    let mut lines: Vec<String> = rendered.lines().map(str::to_owned).collect();
    let mut spacing = vec![None; lines.len()];
    for node in normalized.descendants() {
        let data = node.data.borrow();
        match &data.value {
            NodeValue::Table(table) => {
                let start = data.sourcepos.start.line - 1;
                let end = data.sourcepos.end.line;
                tables::format(
                    &mut lines[start..end],
                    &table.alignments,
                    config,
                    protected.as_ref(),
                )?;
            }
            NodeValue::Heading(_) => {
                let end = data.sourcepos.end.line - 1;
                let prefix = lines[end].split_once('#').map_or("", |(prefix, _)| prefix);
                // A heading can be the first block of a list item. Blank lines
                // keep its indentation and quote markers, not its list marker.
                let prefix: String = prefix
                    .chars()
                    .map(|c| if c == '>' { c } else { ' ' })
                    .collect();
                spacing[end] = Some(prefix.trim_end().to_owned());
            }
            _ => {}
        }
    }
    let mut output = String::with_capacity(rendered.len());
    let mut index = 0;
    while index < lines.len() {
        output.push_str(&lines[index]);
        output.push('\n');
        if let Some(prefix) = &spacing[index] {
            index += 1;
            while index < lines.len() && lines[index].trim_end() == prefix {
                index += 1;
            }
            if index < lines.len() {
                for _ in 0..config.heading_blank_lines {
                    output.push_str(prefix);
                    output.push('\n');
                }
            }
        } else {
            index += 1;
        }
    }
    let mut output = match protected {
        Some(protected) => protected.restore(&output)?,
        None => output,
    };
    output.truncate(output.trim_end_matches(['\r', '\n']).len());
    if config.final_newline && !output.is_empty() {
        output.push('\n');
    }
    Ok(output)
}
