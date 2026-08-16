use std::collections::HashMap;

use dotfile_source::ByteRange;
use dotfile_syntax::{Cst, NodeId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Comment {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) text: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CommentBlock {
    pub(crate) comments: Vec<Comment>,
    pub(crate) blank_before: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ItemDecoration {
    pub(crate) leading: Vec<CommentBlock>,
    pub(crate) trailing: Vec<Comment>,
    pub(crate) blank_before: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Region {
    /// Original item ordinals belonging to this independently sortable
    /// region.  A blank-separated section comment is a stable boundary.
    pub(crate) items: Vec<usize>,
    pub(crate) header: Vec<CommentBlock>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Decorations {
    pub(crate) items: HashMap<NodeId, ItemDecoration>,
    pub(crate) regions: Vec<Region>,
    pub(crate) tail: Vec<CommentBlock>,
}

pub(crate) fn scan(cst: &Cst, source: &[u8]) -> Vec<Comment> {
    let mut comments = Vec::new();
    for gap in cst.gaps() {
        let bytes = &source[gap.range.start() as usize..gap.range.end() as usize];
        let Some(relative) = bytes.iter().position(|byte| *byte == b'#') else {
            continue;
        };
        let start = gap.range.start() + relative as u64;
        let raw = &source[start as usize..gap.range.end() as usize];
        // The lexer owns CRLF as one newline token, but the trivia gap before
        // it may end after the CR boundary.  A CR is line-ending syntax, not
        // comment payload, and canonical output is LF-only.
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        // Parse-valid input is strict UTF-8.  Keeping a lossy fallback makes
        // this helper total even when called during debugging.
        comments.push(Comment {
            start,
            end: gap.range.end(),
            text: String::from_utf8_lossy(raw).into_owned(),
        });
    }
    comments
}

/// Derives entry attachments for one file/block/list container.  Comments
/// inside a direct child belong to that child's nested syntax and are left
/// for the recursive renderer.
pub(crate) fn attach(
    source: &[u8],
    comments: &[Comment],
    container: ByteRange,
    items: &[(NodeId, ByteRange)],
) -> Decorations {
    let mut decorations = Decorations::default();
    for (node, _) in items {
        decorations.items.insert(*node, ItemDecoration::default());
    }

    let free: Vec<Comment> = comments
        .iter()
        .filter(|comment| {
            container.start() <= comment.start
                && comment.end <= container.end()
                && !items
                    .iter()
                    .any(|(_, range)| range.start() <= comment.start && comment.start < range.end())
        })
        .cloned()
        .collect();

    let mut standalone = Vec::new();
    for comment in free {
        if let Some((node, range)) = items
            .iter()
            .rev()
            .find(|(_, range)| range.end() <= comment.start)
            && !contains_newline(source, range.end(), comment.start)
        {
            decorations
                .items
                .get_mut(node)
                .expect("direct item decoration")
                .trailing
                .push(comment);
            continue;
        }
        standalone.push(comment);
    }

    let blocks = comment_blocks(source, standalone, items);
    let mut section_before: HashMap<usize, Vec<CommentBlock>> = HashMap::new();
    let mut tail = Vec::new();
    for mut block in blocks {
        let next = items
            .iter()
            .enumerate()
            .find(|(_, (_, range))| range.start() >= block.comments.last().unwrap().end);
        match next {
            Some((ordinal, (node, range))) => {
                let last_end = block.comments.last().unwrap().end;
                if has_blank_line(source, last_end, range.start()) {
                    section_before.entry(ordinal).or_default().push(block);
                } else {
                    let first_start = block.comments.first().unwrap().start;
                    let previous_end = items
                        .get(ordinal.wrapping_sub(1))
                        .map(|(_, previous)| previous.end())
                        .unwrap_or(container.start());
                    block.blank_before =
                        ordinal > 0 && has_blank_line(source, previous_end, first_start);
                    decorations
                        .items
                        .get_mut(node)
                        .expect("direct item decoration")
                        .leading
                        .push(block);
                }
            }
            None => {
                if let (Some((_, last)), Some(first)) = (items.last(), block.comments.first()) {
                    block.blank_before = has_blank_line(source, last.end(), first.start);
                }
                tail.push(block);
            }
        }
    }

    for (ordinal, (node, range)) in items.iter().enumerate() {
        let start = decorations
            .items
            .get(node)
            .and_then(|decoration| decoration.leading.first())
            .and_then(|block| block.comments.first())
            .map(|comment| comment.start)
            .unwrap_or(range.start());
        let previous_end = ordinal
            .checked_sub(1)
            .and_then(|previous| items.get(previous))
            .map(|(_, range)| range.end())
            .unwrap_or(container.start());
        decorations
            .items
            .get_mut(node)
            .expect("direct item decoration")
            .blank_before = ordinal > 0 && has_blank_line(source, previous_end, start);
    }

    let mut current = Region::default();
    for ordinal in 0..items.len() {
        if let Some(headers) = section_before.remove(&ordinal) {
            if !current.items.is_empty() || !current.header.is_empty() {
                decorations.regions.push(current);
                current = Region::default();
            }
            current.header = headers;
        }
        current.items.push(ordinal);
    }
    if !current.items.is_empty() || !current.header.is_empty() {
        decorations.regions.push(current);
    }
    if items.is_empty() && !tail.is_empty() {
        // A comment-only container has no entry to attach to.  Its comment
        // blocks stay at the container end in original order.
    }
    decorations.tail = tail;
    decorations
}

fn comment_blocks(
    source: &[u8],
    comments: Vec<Comment>,
    items: &[(NodeId, ByteRange)],
) -> Vec<CommentBlock> {
    let mut blocks: Vec<CommentBlock> = Vec::new();
    for comment in comments {
        let starts_new = blocks
            .last()
            .and_then(|block| block.comments.last())
            .is_some_and(|previous| {
                has_blank_line(source, previous.end, comment.start)
                    || items.iter().any(|(_, range)| {
                        previous.end <= range.start() && range.end() <= comment.start
                    })
            });
        if starts_new || blocks.is_empty() {
            blocks.push(CommentBlock {
                comments: vec![comment],
                blank_before: starts_new,
            });
        } else {
            blocks.last_mut().unwrap().comments.push(comment);
        }
    }
    blocks
}

fn contains_newline(source: &[u8], start: u64, end: u64) -> bool {
    source[start as usize..end as usize]
        .iter()
        .any(|byte| *byte == b'\n' || *byte == b'\r')
}

/// Whether the bytes between two syntax objects contain a complete blank
/// physical line.  Comment-only lines are not blank.
pub(crate) fn has_blank_line(source: &[u8], start: u64, end: u64) -> bool {
    if start >= end {
        return false;
    }
    let parts: Vec<&[u8]> = source[start as usize..end as usize]
        .split(|byte| *byte == b'\n')
        .collect();
    if parts.len() < 3 {
        return false;
    }
    parts[1..parts.len() - 1].iter().any(|line| {
        line.iter()
            .all(|byte| matches!(*byte, b' ' | b'\t' | b'\r'))
    })
}

pub(crate) fn normalized_text(comment: &Comment) -> &str {
    comment.text.trim_start_matches([' ', '\t'])
}
