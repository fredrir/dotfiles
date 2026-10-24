use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, parse_document};

use crate::config::Config;
use crate::dialect::Dialect;

// Render the document as plain text: inline markers, list markers and link
// syntax are stripped to their content, block constructs lose their Markdown
// syntax, and code, HTML, math, frontmatter and Obsidian links stay literal.
pub(super) fn clean(input: &str, config: &Config) -> String {
    let normalized = input.replace("\r\n", "\n");
    let input = crate::without_leading_blank_lines(&normalized);
    let (frontmatter, body) = if config.dialect != Dialect::Commonmark {
        crate::split_frontmatter(input)
    } else {
        ("", input)
    };
    let options = crate::options(config);
    let arena = Arena::new();
    let root = parse_document(&arena, body, &options);
    let mut output = frontmatter.to_owned();
    let blocks = render_blocks(root, config);
    if !blocks.is_empty() {
        output.push_str(&blocks);
        output.push('\n');
    }
    crate::finish(output, config)
}

fn render_blocks<'a>(node: &'a AstNode<'a>, config: &Config) -> String {
    let separator = "\n".repeat(config.blank_lines + 1);
    node.children()
        .map(|child| render_block(child, config))
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join(&separator)
}

fn render_block<'a>(node: &'a AstNode<'a>, config: &Config) -> String {
    let value = node.data.borrow().value.clone();
    match value {
        NodeValue::Paragraph | NodeValue::Heading(_) => render_inlines(node),
        NodeValue::CodeBlock(code) => code.literal.trim_end_matches('\n').to_owned(),
        NodeValue::HtmlBlock(html) => html.literal.trim_end_matches('\n').to_owned(),
        NodeValue::BlockQuote => render_blocks(node, config),
        NodeValue::List(_) | NodeValue::Item(_) | NodeValue::TaskItem(_) => {
            render_blocks(node, config)
        }
        NodeValue::Table(_) => render_table(node, config),
        // Alerts keep their body blocks; the [!TYPE] marker is formatting.
        NodeValue::Alert(_) => render_blocks(node, config),
        NodeValue::FootnoteDefinition(_)
        | NodeValue::DescriptionList
        | NodeValue::DescriptionItem(_)
        | NodeValue::DescriptionTerm
        | NodeValue::DescriptionDetails
        | NodeValue::MultilineBlockQuote(_)
        | NodeValue::Subtext => render_blocks(node, config),
        NodeValue::ThematicBreak | NodeValue::FrontMatter(_) => String::new(),
        _ => String::new(),
    }
}

fn render_table<'a>(node: &'a AstNode<'a>, config: &Config) -> String {
    // Rows separated by blank lines parse back as independent paragraphs, so
    // cleaning settles after one pass instead of merging rows on the next.
    let separator = "\n".repeat(config.blank_lines + 1);
    node.children()
        .map(|row| {
            row.children()
                .map(|cell| render_inlines(cell))
                .collect::<Vec<_>>()
                .join("  ")
        })
        .collect::<Vec<_>>()
        .join(&separator)
}

fn render_inlines<'a>(node: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    for child in node.children() {
        let value = child.data.borrow().value.clone();
        match value {
            NodeValue::Text(literal) => text.push_str(&literal),
            NodeValue::Code(code) => text.push_str(&code.literal),
            NodeValue::Math(math) => text.push_str(&math.literal),
            NodeValue::HtmlInline(html) => text.push_str(&html),
            NodeValue::SoftBreak => text.push(' '),
            NodeValue::LineBreak => text.push('\n'),
            NodeValue::Emph
            | NodeValue::Strong
            | NodeValue::Strikethrough
            | NodeValue::Highlight
            | NodeValue::Superscript
            | NodeValue::Subscript
            | NodeValue::Underline
            | NodeValue::SpoileredText
            | NodeValue::Escaped
            | NodeValue::Link(_)
            | NodeValue::Image(_)
            | NodeValue::WikiLink(_) => text.push_str(&render_inlines(child)),
            NodeValue::FootnoteReference(_) => {}
            NodeValue::Raw(raw) => text.push_str(&raw),
            _ => {}
        }
    }
    text
}
