use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

use dotfmt_core::edit::Edit;
use oxc_ast::ast::{ImportDeclaration, ImportDeclarationSpecifier, Statement};
use oxc_formatter::{GroupEntry, GroupName, ImportModifier, ImportSelector, SortImportsOptions};
use oxc_span::GetSpan;

use crate::config::SortOrder;

use super::context::Context;
use super::imports::ImportChange;

#[derive(Default)]
pub(crate) struct OrganizePlan {
    pub edits: Vec<Edit>,
    pub covered: BTreeSet<usize>,
}

struct Item {
    decl_start: usize,
    item_start: usize,
    item_end: usize,
    text: String,
    /// Comments that stay at this item's slot even when the item moves or is removed:
    /// the comments of a removed import, and a comment block separated from its import
    /// by a blank line.
    orphan: String,
    lead_blank: bool,
    trail_blank: bool,
    source: String,
    key: String,
    group: usize,
    side_effect: bool,
    type_import: bool,
    default_specifier: bool,
    namespace_specifier: bool,
    named_specifier: bool,
    fixed: bool,
    deleted: bool,
}

pub(crate) fn plan<'a, 'p>(context: &Context<'a, 'p>, changes: &[ImportChange]) -> OrganizePlan {
    let mut plan = OrganizePlan::default();
    let changes: HashMap<usize, &ImportChange> = changes
        .iter()
        .map(|change| (change.decl_start, change))
        .collect();
    let options = sort_options(context);
    let groups = Groups::new(&options);
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
        for (from, to) in partition(context, &options, run) {
            if to - from < 2 {
                continue;
            }
            if let Some((edit, covered)) =
                organize_run(context, &options, &groups, run, from, to, &changes)
            {
                plan.edits.push(edit);
                plan.covered.extend(covered);
            }
        }
    }
    plan
}

/// The ordering and partitioning options shared with `sort_imports`.
///
/// `Config::sort_imports_options` builds the same set for the formatter; it is private to
/// `config`, so the values are assembled here from the same fields and tsconfig aliases.
fn sort_options(context: &Context<'_, '_>) -> SortImportsOptions {
    let config = context.config;
    let mut options = SortImportsOptions {
        partition_by_newline: config.import_partition_by_newline,
        partition_by_comment: config.import_partition_by_comment,
        sort_side_effects: config.import_side_effects,
        order: match config.import_order {
            SortOrder::Asc => oxc_formatter::SortOrder::Asc,
            SortOrder::Desc => oxc_formatter::SortOrder::Desc,
        },
        newlines_between: config.import_newlines,
        internal_pattern: config.import_internal.clone(),
        ..SortImportsOptions::default()
    };
    if let Some(groups) = &config.import_groups {
        options.groups = groups.clone();
    }
    if let Some(patterns) = crate::tsconfig::import_patterns(context.path) {
        for pattern in patterns {
            if !options.internal_pattern.contains(&pattern) {
                options.internal_pattern.push(pattern);
            }
        }
    }
    options
}

/// Group the declarations of one import run into the chunks that are sorted separately.
///
/// Mirrors `sort_imports`: blank lines partition when `import_partition_by_newline` is set,
/// comment-only lines partition when `import_partition_by_comment` is set.
fn partition<'a>(
    context: &Context<'a, '_>,
    options: &SortImportsOptions,
    run: &[Statement<'a>],
) -> Vec<(usize, usize)> {
    let mut chunks = Vec::new();
    let mut start = 0;
    for index in 1..run.len() {
        let previous_end = run[index - 1].span().end as usize;
        let declaration_start = run[index].span().start as usize;
        let between = &context.text[previous_end..declaration_start];
        let boundary = (options.partition_by_newline && between.matches('\n').count() >= 2)
            || (options.partition_by_comment
                && has_own_line_comment(context, previous_end, declaration_start));
        if boundary {
            chunks.push((start, index));
            start = index;
        }
    }
    chunks.push((start, run.len()));
    chunks
}

fn has_own_line_comment(context: &Context<'_, '_>, start: usize, end: usize) -> bool {
    context.comments.iter().any(|comment| {
        let comment_start = comment.span.start as usize;
        let comment_end = comment.span.end as usize;
        comment_start >= start
            && comment_end <= end
            && context.text[context.line_start(comment_start)..comment_start]
                .trim()
                .is_empty()
            && context.text[comment_end..end].contains('\n')
    })
}

#[allow(clippy::too_many_arguments)]
fn organize_run<'a, 'p>(
    context: &Context<'a, 'p>,
    options: &SortImportsOptions,
    groups: &Groups,
    run: &[Statement<'a>],
    from: usize,
    to: usize,
    changes: &HashMap<usize, &ImportChange>,
) -> Option<(Edit, BTreeSet<usize>)> {
    let statements = &run[from..to];
    let mut items = Vec::with_capacity(statements.len());
    let mut previous_end = None;
    for (offset, statement) in statements.iter().enumerate() {
        let Statement::ImportDeclaration(declaration) = statement else {
            return None;
        };
        let decl_start = declaration.span.start as usize;
        let decl_end = declaration.span.end as usize;
        let position = from + offset;
        let item_start = previous_end.map_or(decl_start, |previous_end| {
            leading_comment_start(context, previous_end, decl_start)
        });
        let next_boundary = run
            .get(position + 1)
            .map_or(context.text.len(), |statement| {
                statement.span().start as usize
            });
        let item_end = trailing_comment_end(context, decl_end, next_boundary);
        let boundary_start = match position.checked_sub(1).and_then(|index| run.get(index)) {
            Some(statement) => statement.span().end as usize,
            None => item_start,
        };
        let lead_blank = context.text[boundary_start..item_start]
            .matches('\n')
            .count()
            >= 2;
        let change = changes.get(&decl_start);
        let deleted = change.is_some_and(|change| change.delete);
        let leading = &context.text[item_start..decl_start];
        let trailing = &context.text[decl_end..item_end];
        let rewritten = change.map_or_else(
            || context.text[decl_start..decl_end].to_owned(),
            |change| change.edit.new_text.clone(),
        );
        // A rewritten import that is no longer an `import` statement is a module marker
        // (`export {};`); it keeps its slot instead of being sorted as an import.
        let marker = change
            .is_some_and(|change| !change.delete && !rewritten.trim_start().starts_with("import"));
        let specifiers = declaration.specifiers.as_deref();
        let specifiers = specifiers.map_or(&[][..], |specifiers| specifiers.as_slice());
        let side_effect = change.map_or(specifiers.is_empty(), |_| {
            rewritten.trim_start().starts_with("import \"")
        });
        // A rewritten import that lost every specifier behaves like a side-effect import.
        let specifiers = if side_effect { &[][..] } else { specifiers };
        let type_import =
            declaration.import_kind.is_type() || rewritten.trim_start().starts_with("import type");
        let blank_after = has_blank_after(leading);
        // A comment block separated from its declaration by a blank line is a section
        // marker: it stays where it was written while the imports around it are sorted.
        let detached = !deleted && blank_after;
        let mut item = Item {
            decl_start,
            item_start,
            item_end,
            text: if deleted {
                String::new()
            } else if detached {
                format!("{rewritten}{trailing}")
            } else {
                format!("{leading}{rewritten}{trailing}")
            },
            orphan: if deleted {
                deleted_comments(leading, trailing)
            } else if detached {
                leading.trim().to_owned()
            } else {
                String::new()
            },
            lead_blank,
            trail_blank: blank_after,
            source: String::new(),
            key: String::new(),
            group: 0,
            side_effect,
            type_import,
            default_specifier: specifiers.iter().any(|specifier| {
                matches!(
                    specifier,
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(_)
                )
            }),
            namespace_specifier: specifiers.iter().any(|specifier| {
                matches!(
                    specifier,
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(_)
                )
            }),
            named_specifier: specifiers.iter().any(|specifier| {
                matches!(specifier, ImportDeclarationSpecifier::ImportSpecifier(_))
            }),
            fixed: marker
                || context.is_ignored(decl_start, decl_end)
                || (side_effect && !options.sort_side_effects && !regroups_side_effects(options)),
            deleted,
        };
        item.source = source_path(declaration);
        item.key = if options.ignore_case {
            item.source.to_lowercase()
        } else {
            item.source.clone()
        };
        item.group = group_of(&item, options, groups);
        items.push(item);
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
    let covered = changes
        .keys()
        .copied()
        .filter(|start| items.iter().any(|item| item.decl_start == *start))
        .collect();
    let start = items.first()?.item_start;
    let end = items.last()?.item_end;
    if slots.is_empty() {
        let output = residues(&items);
        if output.is_empty() {
            let (start, end) = context.statement_edit(start, end);
            return Some((Edit::delete(start, end), covered));
        }
        return Some((Edit::replace(start, end, output), covered));
    }

    let mut order = slots.clone();
    let mut position = 0;
    while position < slots.len() {
        if items[slots[position]].fixed {
            position += 1;
            continue;
        }
        let mut end = position;
        while end < slots.len() && !items[slots[end]].fixed {
            end += 1;
        }
        let mut segment = slots[position..end].to_vec();
        segment.sort_by(|left, right| {
            compare(&items[*left], &items[*right], options).then_with(|| left.cmp(right))
        });
        order[position..end].copy_from_slice(&segment);
        position = end;
    }

    let output = render(&items, &separators, &slots, &order, options);
    if output == context.text[start..end] {
        return None;
    }
    Some((Edit::replace(start, end, output), covered))
}

/// The comments of a deleted import, which stay in the file even though the statement goes away.
fn deleted_comments(leading: &str, trailing: &str) -> String {
    let leading = leading.trim();
    let trailing = trailing.trim();
    match (leading.is_empty(), trailing.is_empty()) {
        (true, _) => trailing.to_owned(),
        (_, true) => leading.to_owned(),
        _ => format!("{leading}\n{trailing}"),
    }
}

/// Whether a blank line separates the last leading comment from its declaration.
fn has_blank_after(leading: &str) -> bool {
    if leading.trim().is_empty() {
        return false;
    }
    let comments = leading.trim_end_matches(char::is_whitespace);
    leading[comments.len()..].matches('\n').count() >= 2
}

fn source_path(declaration: &ImportDeclaration<'_>) -> String {
    let source = declaration.source.value.as_str().unwrap_or_default();
    source.split('?').next().unwrap_or(source).to_owned()
}

fn compare(left: &Item, right: &Item, options: &SortImportsOptions) -> Ordering {
    left.group.cmp(&right.group).then_with(|| {
        if options.order.is_desc() {
            natural_cmp(&right.key, &left.key)
        } else {
            natural_cmp(&left.key, &right.key)
        }
    })
}

/// Natural ordering, matching the `natord::compare` oxc uses to sort import sources:
/// runs of digits compare by value, everything else by character.
fn natural_cmp(left: &str, right: &str) -> Ordering {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    let (mut index, mut other) = (0, 0);
    while index < left.len() && other < right.len() {
        if left[index].is_ascii_digit() && right[other].is_ascii_digit() {
            let left_start = index;
            let right_start = other;
            while index < left.len() && left[index].is_ascii_digit() {
                index += 1;
            }
            while other < right.len() && right[other].is_ascii_digit() {
                other += 1;
            }
            let run = &left[left_start..index];
            let other_run = &right[right_start..other];
            let ordering = if run[0] == b'0' || other_run[0] == b'0' {
                // Left-aligned matching, like `015` < `12`.
                run.cmp(other_run)
                    .then_with(|| run.len().cmp(&other_run.len()))
            } else {
                run.len()
                    .cmp(&other_run.len())
                    .then_with(|| run.cmp(other_run))
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
            continue;
        }
        if left[index] != right[other] {
            return left[index].cmp(&right[other]);
        }
        index += 1;
        other += 1;
    }
    (left.len() - index).cmp(&(right.len() - other))
}

fn render(
    items: &[Item],
    separators: &[String],
    slots: &[usize],
    order: &[usize],
    options: &SortImportsOptions,
) -> String {
    // Comments that stay at their slot: the import that occupied the slot may move or
    // disappear, the comment does not.
    let mut anchored: Vec<Vec<usize>> = vec![Vec::new(); order.len() + 1];
    for (index, item) in items.iter().enumerate() {
        if item.orphan.is_empty() {
            continue;
        }
        let anchor = slots.partition_point(|slot| *slot < index);
        anchored[anchor].push(index);
    }

    let mut output = String::new();
    let mut last: Option<Last> = None;
    for position in 0..=order.len() {
        for (offset, index) in anchored[position].iter().enumerate() {
            let item = &items[*index];
            if let Some(previous) = last {
                let separator = if offset > 0 {
                    if item.lead_blank { "\n\n" } else { "\n" }
                } else {
                    match previous {
                        Last::Orphan(previous) => {
                            if items[previous].trail_blank {
                                "\n\n"
                            } else {
                                "\n"
                            }
                        }
                        Last::Item { slot, .. } => {
                            separators.get(slot).map_or("\n", String::as_str)
                        }
                    }
                };
                output.push_str(separator);
            }
            output.push_str(&item.orphan);
            last = Some(Last::Orphan(*index));
        }
        if position == order.len() {
            break;
        }
        let item = &items[order[position]];
        if let Some(previous) = last {
            match previous {
                Last::Orphan(previous) => {
                    output.push_str(if items[previous].trail_blank {
                        "\n\n"
                    } else {
                        "\n"
                    });
                }
                Last::Item { slot, group } => {
                    if group != item.group && options.newlines_between {
                        output.push_str("\n\n");
                    } else {
                        output.push_str(separators.get(slot).map_or("\n", String::as_str));
                    }
                }
            }
        }
        output.push_str(&item.text);
        last = Some(Last::Item {
            slot: slots[position],
            group: item.group,
        });
    }
    output
}

/// The block written before the current one, used to choose the separator.
#[derive(Clone, Copy)]
enum Last {
    Item { slot: usize, group: usize },
    Orphan(usize),
}

/// Everything that survives when every import of a run is removed: only their comments.
fn residues(items: &[Item]) -> String {
    let mut output = String::new();
    let mut written = false;
    for item in items {
        if item.orphan.is_empty() {
            continue;
        }
        if written {
            output.push_str(if item.lead_blank { "\n\n" } else { "\n" });
        }
        output.push_str(&item.orphan);
        written = true;
    }
    output
}

struct Groups {
    predefined: Vec<(GroupName, usize)>,
    unknown: usize,
}

impl Groups {
    fn new(options: &SortImportsOptions) -> Self {
        let mut predefined = Vec::new();
        let mut unknown = None;
        for (index, group) in options.groups.iter().enumerate() {
            for entry in group {
                match entry {
                    GroupEntry::Predefined(name) => predefined.push((name.clone(), index)),
                    GroupEntry::Unknown => unknown = Some(index),
                    // `import_groups` cannot define custom groups, so `Custom` never resolves.
                    GroupEntry::Custom(_) => {}
                }
            }
        }
        predefined.sort_by(|left, right| left.0.cmp(&right.0));
        Self {
            predefined,
            unknown: unknown.unwrap_or(options.groups.len()),
        }
    }

    fn index(&self, selectors: &[ImportSelector], modifiers: &[ImportModifier]) -> usize {
        self.predefined
            .iter()
            .find(|(name, _)| name.is_a_possible_name_of(selectors, modifiers))
            .map_or(self.unknown, |(_, index)| *index)
    }
}

fn regroups_side_effects(options: &SortImportsOptions) -> bool {
    options.groups.iter().flatten().any(|entry| {
        matches!(entry, GroupEntry::Predefined(name) if name.is_plain_selector(ImportSelector::SideEffect))
    })
}

fn group_of(item: &Item, options: &SortImportsOptions, groups: &Groups) -> usize {
    let style = is_style(&item.source);
    let path = PathKind::of(&item.source, options);
    let mut selectors = Vec::new();
    if item.side_effect && style {
        selectors.push(ImportSelector::SideEffectStyle);
    }
    if item.side_effect {
        selectors.push(ImportSelector::SideEffect);
    }
    if style {
        selectors.push(ImportSelector::Style);
    }
    if item.type_import {
        selectors.push(ImportSelector::Type);
    }
    match path {
        PathKind::Index => selectors.push(ImportSelector::Index),
        PathKind::Sibling => selectors.push(ImportSelector::Sibling),
        PathKind::Parent => selectors.push(ImportSelector::Parent),
        _ => {}
    }
    if item.source.starts_with('#') {
        selectors.push(ImportSelector::Subpath);
    }
    match path {
        PathKind::Internal => selectors.push(ImportSelector::Internal),
        PathKind::Builtin => selectors.push(ImportSelector::Builtin),
        PathKind::External => selectors.push(ImportSelector::External),
        _ => {}
    }
    selectors.push(ImportSelector::Import);

    let mut modifiers = Vec::new();
    if item.side_effect {
        modifiers.push(ImportModifier::SideEffect);
    }
    if item.type_import {
        modifiers.push(ImportModifier::Type);
    } else {
        modifiers.push(ImportModifier::Value);
    }
    if item.default_specifier {
        modifiers.push(ImportModifier::Default);
    }
    if item.namespace_specifier {
        modifiers.push(ImportModifier::Wildcard);
    }
    if item.named_specifier {
        modifiers.push(ImportModifier::Named);
    }
    groups.index(&selectors, &modifiers)
}

#[derive(Clone, Copy)]
enum PathKind {
    Builtin,
    External,
    Internal,
    Parent,
    Sibling,
    Index,
}

impl PathKind {
    fn of(source: &str, options: &SortImportsOptions) -> Self {
        if is_builtin(source) {
            return Self::Builtin;
        }
        if source.starts_with('.') {
            if matches!(
                source,
                "." | "./"
                    | "./index"
                    | "./index.js"
                    | "./index.ts"
                    | "./index.d.ts"
                    | "./index.d.js"
            ) {
                return Self::Index;
            }
            if source.starts_with("..") {
                return Self::Parent;
            }
            return Self::Sibling;
        }
        if options
            .internal_pattern
            .iter()
            .any(|pattern| source.starts_with(pattern.as_str()))
        {
            return Self::Internal;
        }
        Self::External
    }
}

fn is_style(source: &str) -> bool {
    std::path::Path::new(source)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "css" | "scss" | "sass" | "less" | "styl" | "pcss" | "sss"
            )
        })
}

fn is_builtin(source: &str) -> bool {
    if source.starts_with("node:") || source.starts_with("bun:") {
        return true;
    }
    let name = source.split('/').next().unwrap_or(source);
    NODE_BUILTINS.binary_search(&name).is_ok()
}

/// Node's core modules that resolve without the `node:` prefix.
const NODE_BUILTINS: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

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

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::{NODE_BUILTINS, natural_cmp};

    #[test]
    fn natural_cmp_matches_the_order_oxc_sorts_import_sources_with() {
        let ordering = |left: &str, right: &str| natural_cmp(left, right);
        assert_eq!(ordering("a", "a0"), Ordering::Less);
        assert_eq!(ordering("a2", "a10"), Ordering::Less);
        assert_eq!(ordering("a10", "a2"), Ordering::Greater);
        assert_eq!(ordering("015", "12"), Ordering::Less);
        assert_eq!(ordering("15", "123"), Ordering::Less);
        assert_eq!(ordering("x2-y7", "x2-y8"), Ordering::Less);
        assert_eq!(ordering("./a", "./a"), Ordering::Equal);
        assert_eq!(ordering("./a", "./b"), Ordering::Less);
        assert_eq!(ordering("./z", "./ab"), Ordering::Greater);
    }

    #[test]
    fn node_builtins_are_sorted_for_binary_search() {
        assert!(
            NODE_BUILTINS.windows(2).all(|pair| pair[0] < pair[1]),
            "{NODE_BUILTINS:?}"
        );
    }
}
