use std::collections::{BTreeSet, HashMap};

use dotfmt_core::edit::Edit;
use oxc_ast::ast::{ImportDeclaration, Statement};
use oxc_span::GetSpan;

use crate::config::SortOrder;

use super::context::Context;
use super::imports::ImportChange;

#[derive(Default)]
pub(crate) struct OrganizePlan {
    pub edits: Vec<Edit>,
    pub covered: BTreeSet<usize>,
}

struct Item<'a> {
    declaration: &'a ImportDeclaration<'a>,
    item_start: usize,
    item_end: usize,
    text: String,
    side_effect: bool,
    ignored: bool,
    deleted: bool,
    key: String,
}

pub(crate) fn plan<'a, 'p>(context: &Context<'a, 'p>, changes: &[ImportChange]) -> OrganizePlan {
    let mut plan = OrganizePlan::default();
    let changes: HashMap<usize, &ImportChange> = changes
        .iter()
        .map(|change| (change.decl_start, change))
        .collect();
    let body = &context.program.body;
    let mut index = 0;
    while index < body.len() {
        if !matches!(body[index], Statement::ImportDeclaration(_)) {
            index += 1;
            continue;
        }
        let start = index;
        while index < body.len() && matches!(body[index], Statement::ImportDeclaration(_)) {
            index += 1;
        }
        let run = &body[start..index];
        if run.len() < 2 {
            continue;
        }
        if let Some((edit, covered)) = organize_run(context, run, &changes) {
            plan.edits.push(edit);
            plan.covered.extend(covered);
        }
    }
    plan
}

fn organize_run<'a, 'p>(
    context: &Context<'a, 'p>,
    run: &[Statement<'a>],
    changes: &HashMap<usize, &ImportChange>,
) -> Option<(Edit, BTreeSet<usize>)> {
    let mut items = Vec::with_capacity(run.len());
    let mut previous_end = None;
    for (index, statement) in run.iter().enumerate() {
        let Statement::ImportDeclaration(declaration) = statement else {
            return None;
        };
        let decl_start = declaration.span.start as usize;
        let decl_end = declaration.span.end as usize;
        let item_start = previous_end.map_or(decl_start, |previous_end| {
            leading_comment_start(context, previous_end, decl_start)
        });
        let next_boundary = run.get(index + 1).map_or(context.text.len(), |statement| {
            statement.span().start as usize
        });
        let item_end = trailing_comment_end(context, decl_end, next_boundary);
        let change = changes.get(&decl_start);
        let deleted = change.is_some_and(|change| change.delete);
        let text = if deleted {
            String::new()
        } else {
            let leading = &context.text[item_start..decl_start];
            let rewritten = change.map_or_else(
                || context.text[decl_start..decl_end].to_owned(),
                |change| change.edit.new_text.clone(),
            );
            let trailing = &context.text[decl_end..item_end];
            format!("{leading}{rewritten}{trailing}")
        };
        items.push(Item {
            declaration,
            item_start,
            item_end,
            text,
            side_effect: declaration
                .specifiers
                .as_ref()
                .is_none_or(|specifiers| specifiers.is_empty()),
            ignored: context.is_ignored(decl_start, decl_end),
            deleted,
            key: declaration
                .source
                .value
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        });
        previous_end = Some(decl_end);
    }

    let separators: Vec<String> = items
        .windows(2)
        .map(|pair| context.text[pair[0].item_end..pair[1].item_start].to_owned())
        .collect();

    let slots: Vec<usize> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (!item.deleted).then_some(index))
        .collect();
    if slots.is_empty() {
        let (start, end) =
            context.statement_edit(items[0].item_start, items[items.len() - 1].item_end);
        let edit = Edit::delete(start, end);
        let covered = changes
            .keys()
            .copied()
            .filter(|start| {
                items
                    .iter()
                    .any(|item| item.declaration.span.start as usize == *start)
            })
            .collect();
        return Some((edit, covered));
    }

    let mut fixed = BTreeSet::new();
    for slot in &slots {
        if items[*slot].side_effect || items[*slot].ignored {
            fixed.insert(*slot);
        }
    }

    // Sort only runs between side-effect/ignored imports. Moving a normal import
    // across a side-effect import would change module evaluation order.
    let mut order = slots.clone();
    let mut start = 0;
    while start < slots.len() {
        if fixed.contains(&slots[start]) {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < slots.len() && !fixed.contains(&slots[end]) {
            end += 1;
        }
        let mut segment = slots[start..end].to_vec();
        match context.config.import_order {
            SortOrder::Asc => segment.sort_by(|left, right| {
                items[*left]
                    .key
                    .cmp(&items[*right].key)
                    .then_with(|| left.cmp(right))
            }),
            SortOrder::Desc => segment.sort_by(|left, right| {
                items[*right]
                    .key
                    .cmp(&items[*left].key)
                    .then_with(|| left.cmp(right))
            }),
        }
        order[start..end].copy_from_slice(&segment);
        start = end;
    }

    let rewrites: BTreeSet<usize> = changes
        .keys()
        .copied()
        .filter(|start| {
            items
                .iter()
                .any(|item| item.declaration.span.start as usize == *start)
        })
        .collect();
    if order == slots && rewrites.is_empty() {
        return None;
    }

    let mut output = String::new();
    for (position, item_index) in order.iter().enumerate() {
        output.push_str(&items[*item_index].text);
        if let Some(slot) = slots.get(position)
            && position + 1 < order.len()
            && let Some(separator) = separators.get(*slot)
        {
            output.push_str(separator);
        }
    }

    let start = items.first()?.item_start;
    let end = items.last()?.item_end;
    let original = &context.text[start..end];
    if output == original {
        return None;
    }
    let covered = rewrites;
    Some((Edit::replace(start, end, output), covered))
}

fn leading_comment_start(
    context: &Context<'_, '_>,
    previous_end: usize,
    decl_start: usize,
) -> usize {
    let mut start = decl_start;
    for comment in context.comments {
        let comment_start = comment.span.start as usize;
        let comment_end = comment.span.end as usize;
        if comment_start >= previous_end
            && comment_end <= decl_start
            && context.text[previous_end..comment_start].contains('\n')
        {
            start = start.min(comment_start);
        }
    }
    start
}

fn trailing_comment_end(context: &Context<'_, '_>, decl_end: usize, limit: usize) -> usize {
    let mut end = decl_end;
    for comment in context.comments {
        let comment_start = comment.span.start as usize;
        let comment_end = comment.span.end as usize;
        if comment_start >= decl_end
            && comment_start < limit
            && !context.text[decl_end..comment_start].contains('\n')
        {
            end = end.max(comment_end);
        }
    }
    end
}
