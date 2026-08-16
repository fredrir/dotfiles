use std::cmp::Ordering;

use dotfile_schema::{
    Domain, FormatContext, FormatEntryKind, FormatOrder, FormatPublishedKind, FormatSchema,
    attribute_order,
};
use dotfile_source::{ByteRange, RepoPath, SourceText};
use dotfile_syntax::{
    Atom, Block, Cst, Element, Entry, List, NodeId, NodeKind, Parse, StringExpr, StringSegment,
    Value,
};
use unicode_normalization::UnicodeNormalization;

use crate::comments::{Comment, attach, normalized_text, scan};

const WIDTH: usize = 100;
const INDENT: usize = 4;

pub(crate) fn format(
    source: &SourceText,
    parsed: &Parse,
    schema: Option<&FormatSchema>,
) -> Vec<u8> {
    let comments = scan(parsed.cst(), source.as_bytes());
    let renderer = Renderer {
        source: source.as_bytes(),
        cst: parsed.cst(),
        schema,
        comments,
    };
    let file = parsed.ast(source);
    let entries = file.entries();

    if entries.is_empty() && renderer.comments.is_empty() {
        return Vec::new();
    }

    let policy = renderer
        .schema
        .map_or(FormatOrder::Preserve, |schema| schema.order_for(&[]));
    let mut lines = renderer.render_entries(
        &entries,
        ByteRange::new(0, source.len(), source.len()).expect("whole source range"),
        0,
        &[],
        policy,
        false,
    );
    trim_blank_edges(&mut lines);
    collapse_blank_lines(&mut lines);

    let mut output = lines.join("\n").into_bytes();
    if !output.is_empty() {
        output.push(b'\n');
    }
    output
}

struct Renderer<'a> {
    source: &'a [u8],
    cst: &'a Cst,
    schema: Option<&'a FormatSchema>,
    comments: Vec<Comment>,
}

impl Renderer<'_> {
    fn render_entries(
        &self,
        entries: &[Entry<'_>],
        container: ByteRange,
        indent: usize,
        path: &[String],
        policy: FormatOrder,
        commas: bool,
    ) -> Vec<String> {
        let item_ranges: Vec<_> = entries
            .iter()
            .map(|entry| (entry.node_id(), entry.range()))
            .collect();
        let decorations = attach(self.source, &self.comments, container, &item_ranges);
        let prologue_len = entries
            .iter()
            .take_while(|entry| matches!(entry, Entry::Let(_)))
            .count();
        let resource_identity = valid_resource_identity(entries, path);
        let mut lines = Vec::new();

        for (region_index, region) in decorations.regions.iter().enumerate() {
            if !region.header.is_empty() {
                if region_index > 0 || !lines.is_empty() {
                    push_blank(&mut lines);
                }
                self.render_comment_blocks(&mut lines, &region.header, indent);
                push_blank(&mut lines);
            }

            let ordinals = self.sorted_region(
                entries,
                &region.items,
                path,
                policy,
                prologue_len,
                resource_identity,
            );
            for (position, ordinal) in ordinals.into_iter().enumerate() {
                let entry = &entries[ordinal];
                let decoration = decorations
                    .items
                    .get(&entry.node_id())
                    .expect("decoration for every direct entry");
                if decoration.blank_before && (!lines.is_empty() || position > 0) {
                    push_blank(&mut lines);
                }
                self.render_comment_blocks(&mut lines, &decoration.leading, indent);

                let mut entry_lines = self.render_entry(entry, indent, path, policy);
                let internal = self.internal_comments(entry);
                if !internal.is_empty() {
                    for comment in internal {
                        lines.push(format!("{}{}", spaces(indent), normalized_text(comment)));
                    }
                }
                if commas {
                    append_comma(&mut entry_lines);
                }
                append_trailing_comments(&mut entry_lines, &decoration.trailing);
                lines.extend(entry_lines);
            }
        }

        if !decorations.tail.is_empty() {
            self.render_comment_blocks(&mut lines, &decorations.tail, indent);
        }
        collapse_blank_lines(&mut lines);
        trim_blank_edges(&mut lines);
        lines
    }

    fn render_comment_blocks(
        &self,
        lines: &mut Vec<String>,
        blocks: &[crate::comments::CommentBlock],
        indent: usize,
    ) {
        for (index, block) in blocks.iter().enumerate() {
            if (block.blank_before || index > 0) && !lines.is_empty() {
                push_blank(lines);
            }
            for comment in &block.comments {
                lines.push(format!("{}{}", spaces(indent), normalized_text(comment)));
            }
        }
    }

    fn internal_comments<'a>(&'a self, entry: &Entry<'_>) -> Vec<&'a Comment> {
        let range = entry.range();
        let nested = self.descendant_containers(entry.node_id());
        self.comments
            .iter()
            .filter(|comment| {
                range.start() <= comment.start
                    && comment.end <= range.end()
                    && !nested.iter().any(|nested| {
                        nested.start() <= comment.start && comment.start < nested.end()
                    })
            })
            .collect()
    }

    fn descendant_containers(&self, node: NodeId) -> Vec<ByteRange> {
        let mut result = Vec::new();
        self.collect_descendant_containers(node, &mut result);
        result
    }

    fn collect_descendant_containers(&self, node: NodeId, result: &mut Vec<ByteRange>) {
        for child in self.cst.children(node) {
            let Element::Node(child) = *child else {
                continue;
            };
            if matches!(self.cst.node_kind(child), NodeKind::Block | NodeKind::List) {
                result.push(self.cst.node_range(child));
            } else {
                self.collect_descendant_containers(child, result);
            }
        }
    }

    fn sorted_region(
        &self,
        entries: &[Entry<'_>],
        ordinals: &[usize],
        path: &[String],
        policy: FormatOrder,
        prologue_len: usize,
        resource_identity: Option<NodeId>,
    ) -> Vec<usize> {
        if matches!(policy, FormatOrder::Preserve) {
            return ordinals.to_vec();
        }

        // An entry without a schema key is a stable barrier.  This keeps
        // unknown entries and misplaced bindings in place while still
        // sorting every well-understood run around them.
        let mut result = Vec::with_capacity(ordinals.len());
        let mut run: Vec<(usize, SortKey)> = Vec::new();
        let flush = |run: &mut Vec<(usize, SortKey)>, result: &mut Vec<usize>| {
            run.sort_by(|left, right| left.1.cmp(&right.1));
            result.extend(run.drain(..).map(|(ordinal, _)| ordinal));
        };

        for ordinal in ordinals.iter().copied() {
            match self.sort_key(
                &entries[ordinal],
                ordinal,
                path,
                policy,
                prologue_len,
                resource_identity,
            ) {
                Some(key) => run.push((ordinal, key)),
                None => {
                    flush(&mut run, &mut result);
                    result.push(ordinal);
                }
            }
        }
        flush(&mut run, &mut result);
        result
    }

    fn sort_key(
        &self,
        entry: &Entry<'_>,
        ordinal: usize,
        path: &[String],
        policy: FormatOrder,
        prologue_len: usize,
        resource_identity: Option<NodeId>,
    ) -> Option<SortKey> {
        if matches!(policy, FormatOrder::Preserve) {
            return None;
        }
        let schema = self.schema?;
        match policy {
            FormatOrder::Preserve => unreachable!("handled before schema lookup"),
            FormatOrder::NamesByBytes => {
                let (name, kind, optional) = published_entry(entry)?;
                let semantic_path = path_as_strs(path);
                if !schema.published_entry_allowed(&semantic_path, &name, kind, optional)
                    || !valid_byte_sort_identity(schema.domain, &semantic_path, &name)
                {
                    return None;
                }
                Some(SortKey {
                    category: 0,
                    order: 0,
                    text: name.into_bytes(),
                    optional: false,
                    ordinal,
                })
            }
            FormatOrder::Published(names) => {
                let (name, kind, optional) = published_entry(entry)?;
                let order = names
                    .iter()
                    .position(|candidate| *candidate == name.as_str())?
                    as u16;
                if !schema.published_entry_allowed(&path_as_strs(path), &name, kind, optional) {
                    return None;
                }
                Some(SortKey {
                    category: 0,
                    order,
                    text: Vec::new(),
                    optional: false,
                    ordinal,
                })
            }
            FormatOrder::OpenThenPublished(names) => match entry {
                Entry::Named(named)
                    if !named.optional()
                        && matches!(named.value(), Some(Value::Reference(_)))
                        && named.block().is_none()
                        && !names.iter().any(|name| Some(*name) == named.name()) =>
                {
                    Some(SortKey {
                        category: 0,
                        order: 0,
                        text: Vec::new(),
                        optional: false,
                        ordinal,
                    })
                }
                Entry::Named(named)
                    if named.block().is_some() && named.value().is_none() && !named.optional() =>
                {
                    let name = named.name()?;
                    let order = names.iter().position(|candidate| *candidate == name)? as u16;
                    if !schema.published_entry_allowed(
                        &path_as_strs(path),
                        name,
                        FormatPublishedKind::NamedBlock,
                        false,
                    ) {
                        return None;
                    }
                    Some(SortKey {
                        category: 1,
                        order,
                        text: Vec::new(),
                        optional: false,
                        ordinal,
                    })
                }
                Entry::Named(_) => None,
                Entry::Let(_)
                | Entry::Extend(_)
                | Entry::Attribute(_)
                | Entry::SigilBlock(_)
                | Entry::Path(_)
                | Entry::Error(_) => None,
            },
            FormatOrder::Requirement(context) => self.requirement_key(
                entry,
                ordinal,
                path,
                context,
                prologue_len,
                resource_identity,
            ),
        }
    }

    fn requirement_key(
        &self,
        entry: &Entry<'_>,
        ordinal: usize,
        path: &[String],
        context: FormatContext,
        prologue_len: usize,
        resource_identity: Option<NodeId>,
    ) -> Option<SortKey> {
        let schema = self.schema?;
        let semantic_path = path_as_strs(path);
        let key = match entry {
            Entry::Let(_) if ordinal < prologue_len => SortKey {
                category: 0,
                order: ordinal as u16,
                text: Vec::new(),
                optional: false,
                ordinal,
            },
            Entry::Let(_) => return None,
            Entry::Attribute(attribute)
                if attribute.name() == Some("key")
                    && resource_identity == Some(entry.node_id())
                    && schema.entry_allowed(&semantic_path, FormatEntryKind::ResourceIdentity) =>
            {
                SortKey {
                    category: 1,
                    order: 0,
                    text: Vec::new(),
                    optional: false,
                    ordinal,
                }
            }
            Entry::Attribute(attribute) => {
                let name = format!("@{}", attribute.name()?);
                let order = schema.attribute_order_for(&semantic_path, context, &name)?;
                SortKey {
                    category: 2,
                    order,
                    text: Vec::new(),
                    optional: false,
                    ordinal,
                }
            }
            Entry::Path(path) => {
                if !schema.entry_allowed(&semantic_path, FormatEntryKind::Path) {
                    return None;
                }
                let decoded = path.decoded_path()?;
                if !valid_source_path(&decoded) {
                    return None;
                }
                SortKey {
                    category: 3,
                    order: 0,
                    text: decoded.into_bytes(),
                    optional: false,
                    ordinal,
                }
            }
            Entry::SigilBlock(resource) => {
                let kind_order = schema.resource_kind_order(resource.name()?);
                if !schema.entry_allowed(&semantic_path, FormatEntryKind::Resource) {
                    return None;
                }
                SortKey {
                    category: 4,
                    order: kind_order?,
                    text: resource_sort_text(resource.name()?, resource.block()?)?,
                    optional: resource.optional(),
                    ordinal,
                }
            }
            Entry::Extend(extension) => {
                if !schema.entry_allowed(&semantic_path, FormatEntryKind::Extension) {
                    return None;
                }
                let target = extension.target()?;
                if !matches!(target.namespace(), Some("entity" | "font")) {
                    return None;
                }
                SortKey {
                    category: 5,
                    order: 0,
                    text: format!("{}/{}", target.namespace()?, target.name()?).into_bytes(),
                    optional: false,
                    ordinal,
                }
            }
            Entry::Named(named) => match context {
                FormatContext::Host
                    if !named.optional() && named.value().is_some() && named.block().is_none() =>
                {
                    let name = named.name()?;
                    let order = match attribute_order(context, name) {
                        Some(order) => order,
                        None if is_host_extension_name(name) => 1_000,
                        None => return None,
                    };
                    SortKey {
                        category: 2,
                        order,
                        text: Vec::new(),
                        optional: false,
                        ordinal,
                    }
                }
                FormatContext::Group
                    if !named.optional() && named.value().is_none() && named.block().is_some() =>
                {
                    SortKey {
                        category: 6,
                        order: 0,
                        text: Vec::new(),
                        optional: false,
                        ordinal,
                    }
                }
                FormatContext::Group => return None,
                FormatContext::Profile | FormatContext::Host => return None,
                FormatContext::Fact | FormatContext::Deployment | FormatContext::GroupRoot => {
                    if !matches!(
                        (named.value(), named.block()),
                        (None, None) | (Some(Value::String(_)), None) | (None, Some(_))
                    ) {
                        return None;
                    }
                    SortKey {
                        category: 6,
                        order: 0,
                        text: named.name()?.as_bytes().to_vec(),
                        optional: named.optional(),
                        ordinal,
                    }
                }
            },
            Entry::Error(_) => return None,
        };
        if matches!(entry, Entry::Named(_))
            && matches!(
                context,
                FormatContext::Fact | FormatContext::Deployment | FormatContext::GroupRoot
            )
            && !schema.entry_allowed(&semantic_path, FormatEntryKind::Demand)
        {
            return None;
        }
        Some(key)
    }

    fn render_entry(
        &self,
        entry: &Entry<'_>,
        indent: usize,
        path: &[String],
        parent_policy: FormatOrder,
    ) -> Vec<String> {
        let prefix = spaces(indent);
        match entry {
            Entry::Let(declaration) => {
                let line = format!("{prefix}@let {} = ", declaration.name().unwrap_or(""));
                match declaration.value() {
                    Some(value) => {
                        self.render_scalar_after_prefix(line, self.render_string(&value), indent)
                    }
                    None => vec![line],
                }
            }
            Entry::Extend(extension) => {
                let target = extension.target();
                let namespace = target.and_then(|target| target.namespace()).unwrap_or("");
                let name = target.and_then(|target| target.name()).unwrap_or("");
                let line = format!("{prefix}@extend {namespace}/{name} ");
                match extension.block() {
                    Some(block) => self.render_block(
                        line,
                        block,
                        indent,
                        child_path(
                            path,
                            match namespace {
                                "entity" => "entity_extension",
                                "font" => "resource_extension",
                                _ => "invalid_extension",
                            },
                        ),
                        Some(FormatContext::Fact),
                    ),
                    None => vec![line.trim_end().to_owned()],
                }
            }
            Entry::Attribute(attribute) => {
                let line = format!("{prefix}@{} = ", attribute.name().unwrap_or(""));
                self.render_optional_value(line, attribute.value(), indent)
            }
            Entry::SigilBlock(resource) => {
                let question = if resource.optional() { "?" } else { "" };
                let line = format!("{prefix}{question}@{} ", resource.name().unwrap_or(""));
                match resource.block() {
                    Some(block) => {
                        let requirement = matches!(parent_policy, FormatOrder::Requirement(_));
                        let registered_requirement = requirement && resource.name() == Some("font");
                        let child_name = if registered_requirement {
                            "resource_demand".to_owned()
                        } else {
                            format!("@{}", resource.name().unwrap_or(""))
                        };
                        self.render_block(
                            line,
                            block,
                            indent,
                            child_path(path, &child_name),
                            requirement.then_some(FormatContext::Fact),
                        )
                    }
                    None => vec![line.trim_end().to_owned()],
                }
            }
            Entry::Named(named) => {
                let question = if named.optional() { "?" } else { "" };
                let mut line = format!("{prefix}{question}{}", named.name().unwrap_or(""));
                if let Some(value) = named.value() {
                    line.push_str(" = ");
                    self.render_value_after_prefix(line, value, indent)
                } else if let Some(block) = named.block() {
                    line.push(' ');
                    let (child_name, override_context) = match parent_policy {
                        FormatOrder::Requirement(FormatContext::Group) => (
                            named.name().unwrap_or("*").to_owned(),
                            Some(FormatContext::Group),
                        ),
                        FormatOrder::Requirement(FormatContext::Fact)
                        | FormatOrder::Requirement(FormatContext::Deployment)
                        | FormatOrder::Requirement(FormatContext::GroupRoot) => {
                            ("entity_demand".to_owned(), Some(FormatContext::Fact))
                        }
                        FormatOrder::Requirement(FormatContext::Profile)
                        | FormatOrder::Requirement(FormatContext::Host)
                        | FormatOrder::Preserve
                        | FormatOrder::NamesByBytes
                        | FormatOrder::Published(_)
                        | FormatOrder::OpenThenPublished(_) => {
                            (named.name().unwrap_or("*").to_owned(), None)
                        }
                    };
                    self.render_block(
                        line,
                        block,
                        indent,
                        child_path(path, &child_name),
                        override_context,
                    )
                } else {
                    vec![line]
                }
            }
            Entry::Path(path_entry) => {
                let question = if path_entry.optional() { "?" } else { "" };
                let path_text = self.render_path(path_entry.node_id());
                let mut line = format!("{prefix}{question}{path_text}");
                if let Some(block) = path_entry.block() {
                    line.push(' ');
                    self.render_block(
                        line,
                        block,
                        indent,
                        child_path(path, "path"),
                        Some(FormatContext::Deployment),
                    )
                } else {
                    vec![line]
                }
            }
            Entry::Error(error) => vec![
                String::from_utf8_lossy(
                    &self.source[error.range().start() as usize..error.range().end() as usize],
                )
                .into_owned(),
            ],
        }
    }

    fn render_optional_value(
        &self,
        prefix: String,
        value: Option<Value<'_>>,
        indent: usize,
    ) -> Vec<String> {
        match value {
            Some(value) => self.render_value_after_prefix(prefix, value, indent),
            None => vec![prefix],
        }
    }

    fn render_value_after_prefix(
        &self,
        prefix: String,
        value: Value<'_>,
        indent: usize,
    ) -> Vec<String> {
        match value {
            Value::String(string) => {
                self.render_scalar_after_prefix(prefix, self.render_string(&string), indent)
            }
            Value::Reference(reference) => self.render_scalar_after_prefix(
                prefix,
                reference.name().unwrap_or("").to_owned(),
                indent,
            ),
            Value::List(list) => self.render_list(prefix, list, indent),
        }
    }

    fn render_scalar_after_prefix(
        &self,
        mut prefix: String,
        scalar: String,
        indent: usize,
    ) -> Vec<String> {
        if prefix.trim().is_empty() || scalar_len(&prefix) + scalar_len(&scalar) <= WIDTH {
            prefix.push_str(&scalar);
            return vec![prefix];
        }

        vec![
            prefix.trim_end().to_owned(),
            format!("{}{scalar}", spaces(indent + INDENT)),
        ]
    }

    fn render_block(
        &self,
        mut prefix: String,
        block: Block<'_>,
        indent: usize,
        path: Vec<String>,
        context_override: Option<FormatContext>,
    ) -> Vec<String> {
        let entries = block.entries();
        let policy = match (self.schema, context_override) {
            (Some(_), Some(context)) => FormatOrder::Requirement(context),
            (Some(schema), None) => schema.order_for(&path_as_strs(&path)),
            (None, _) => FormatOrder::Preserve,
        };
        let prologue_len = entries
            .iter()
            .take_while(|entry| matches!(entry, Entry::Let(_)))
            .count();
        let ordinals: Vec<_> = (0..entries.len()).collect();
        let inline_order = self.sorted_region(
            &entries,
            &ordinals,
            &path,
            policy,
            prologue_len,
            valid_resource_identity(&entries, &path),
        );
        let has_comments = self.comments.iter().any(|comment| {
            block.range().start() < comment.start && comment.end < block.range().end()
        });
        if entries.is_empty() && !has_comments {
            let prefix_width = scalar_len(&prefix);
            if prefix_width + 2 > WIDTH && prefix_width < WIDTH {
                prefix.push('{');
                return vec![prefix, format!("{}}}", spaces(indent))];
            }
            prefix.push_str("{}");
            return vec![prefix];
        }

        let nested_block = entries
            .iter()
            .any(|entry| self.entry_has_direct_block(entry.node_id()));
        if entries.len() <= 3 && !has_comments && !nested_block {
            let inline: Option<Vec<_>> = inline_order
                .iter()
                .map(|ordinal| self.inline_entry(&entries[*ordinal]))
                .collect();
            if let Some(inline) = inline {
                let candidate = format!("{{ {} }}", inline.join(", "));
                if scalar_len(&prefix) + scalar_len(&candidate) <= WIDTH {
                    prefix.push_str(&candidate);
                    return vec![prefix];
                }
            }
        }

        prefix.push('{');
        let mut lines = vec![prefix];
        lines.extend(self.render_entries(
            &entries,
            block.range(),
            indent + INDENT,
            &path,
            policy,
            true,
        ));
        lines.push(format!("{}}}", spaces(indent)));
        lines
    }

    fn render_list(&self, mut prefix: String, list: List<'_>, indent: usize) -> Vec<String> {
        let values = list.values();
        let has_comments = self.comments.iter().any(|comment| {
            list.range().start() < comment.start && comment.end < list.range().end()
        });
        if values.is_empty() && !has_comments {
            if !prefix.trim().is_empty() && scalar_len(&prefix) + 2 > WIDTH {
                return vec![
                    prefix.trim_end().to_owned(),
                    format!("{}[]", spaces(indent + INDENT)),
                ];
            }
            prefix.push_str("[]");
            return vec![prefix];
        }
        if !has_comments {
            let inline: Option<Vec<_>> = values
                .iter()
                .map(|value| self.inline_value(*value))
                .collect();
            if let Some(inline) = inline {
                let candidate = format!("[{}]", inline.join(", "));
                if scalar_len(&prefix) + scalar_len(&candidate) <= WIDTH {
                    prefix.push_str(&candidate);
                    return vec![prefix];
                }
            }
        }

        let (mut lines, list_indent) =
            if !prefix.trim().is_empty() && scalar_len(&prefix) + 1 > WIDTH {
                let assignment = prefix.trim_end().to_owned();
                prefix = format!("{}[", spaces(indent + INDENT));
                (vec![assignment, prefix], indent + INDENT)
            } else {
                prefix.push('[');
                (vec![prefix], indent)
            };
        let item_ranges: Vec<_> = values
            .iter()
            .map(|value| (value.node_id(), value.range()))
            .collect();
        let decorations = attach(self.source, &self.comments, list.range(), &item_ranges);
        for region in &decorations.regions {
            if !region.header.is_empty() {
                self.render_comment_blocks(&mut lines, &region.header, list_indent + INDENT);
            }
            for ordinal in &region.items {
                let value = values[*ordinal];
                let decoration = decorations
                    .items
                    .get(&value.node_id())
                    .expect("list value decoration");
                self.render_comment_blocks(&mut lines, &decoration.leading, list_indent + INDENT);
                let value_prefix = spaces(list_indent + INDENT);
                let mut value_lines =
                    self.render_value_after_prefix(value_prefix, value, list_indent + INDENT);
                append_comma(&mut value_lines);
                append_trailing_comments(&mut value_lines, &decoration.trailing);
                lines.extend(value_lines);
            }
        }
        self.render_comment_blocks(&mut lines, &decorations.tail, list_indent + INDENT);
        trim_blank_edges_after_first(&mut lines);
        lines.push(format!("{}]", spaces(list_indent)));
        lines
    }

    fn inline_entry(&self, entry: &Entry<'_>) -> Option<String> {
        match entry {
            Entry::Let(declaration) => Some(format!(
                "@let {} = {}",
                declaration.name()?,
                self.render_string(&declaration.value()?)
            )),
            Entry::Attribute(attribute) => Some(format!(
                "@{} = {}",
                attribute.name()?,
                self.inline_value(attribute.value()?)?
            )),
            Entry::Named(named) => {
                let question = if named.optional() { "?" } else { "" };
                match named.value() {
                    Some(value) => Some(format!(
                        "{question}{} = {}",
                        named.name()?,
                        self.inline_value(value)?
                    )),
                    None if named.block().is_none() => Some(format!("{question}{}", named.name()?)),
                    None => None,
                }
            }
            Entry::Path(path) if path.block().is_none() => Some(format!(
                "{}{}",
                if path.optional() { "?" } else { "" },
                self.render_path(path.node_id())
            )),
            Entry::Extend(_) | Entry::SigilBlock(_) | Entry::Path(_) | Entry::Error(_) => None,
        }
    }

    fn inline_value(&self, value: Value<'_>) -> Option<String> {
        match value {
            Value::String(string) => Some(self.render_string(&string)),
            Value::Reference(reference) => Some(reference.name()?.to_owned()),
            Value::List(list) => {
                if self.comments.iter().any(|comment| {
                    list.range().start() < comment.start && comment.end < list.range().end()
                }) {
                    return None;
                }
                let values: Option<Vec<_>> = list
                    .values()
                    .into_iter()
                    .map(|value| self.inline_value(value))
                    .collect();
                Some(format!("[{}]", values?.join(", ")))
            }
        }
    }

    fn render_string(&self, expression: &StringExpr<'_>) -> String {
        let mut pieces = Vec::new();
        for atom in expression.atoms() {
            match atom {
                Atom::String { data, text, .. } => match data {
                    Some(data) => {
                        for segment in &data.segments {
                            match segment {
                                StringSegment::Literal { text, .. } => {
                                    push_literal(&mut pieces, text)
                                }
                                StringSegment::Interpolation { name, .. } => {
                                    pieces.push(StringPiece::Interpolation(name.clone()));
                                }
                            }
                        }
                    }
                    None => push_literal(
                        &mut pieces,
                        text.strip_prefix('"')
                            .and_then(|text| text.strip_suffix('"'))
                            .unwrap_or(text),
                    ),
                },
                Atom::Var(variable) => pieces.push(StringPiece::Interpolation(
                    variable.name().unwrap_or("").to_owned(),
                )),
            }
        }
        encode_string(&pieces)
    }

    fn render_path(&self, node: NodeId) -> String {
        let Some((token, data)) = self.find_path_token(node) else {
            return "./\"\"".to_owned();
        };
        match data {
            None => {
                let raw = String::from_utf8_lossy(self.cst.token_text(self.source, token));
                let decoded = raw.strip_prefix("./").unwrap_or(&raw);
                if bare_path(decoded) {
                    format!("./{decoded}")
                } else {
                    format!(
                        "./{}",
                        encode_string(&[StringPiece::Literal(decoded.to_owned())])
                    )
                }
            }
            Some(data) => {
                let mut pieces = Vec::new();
                for segment in &data.segments {
                    match segment {
                        StringSegment::Literal { text, .. } => push_literal(&mut pieces, text),
                        StringSegment::Interpolation { name, .. } => {
                            pieces.push(StringPiece::Interpolation(name.clone()));
                        }
                    }
                }
                if pieces.len() == 1
                    && let StringPiece::Literal(path) = &pieces[0]
                    && bare_path(path)
                {
                    return format!("./{path}");
                }
                format!("./{}", encode_string(&pieces))
            }
        }
    }

    fn find_path_token(&self, node: NodeId) -> Option<(u32, Option<&dotfile_syntax::StringData>)> {
        self.cst
            .children(node)
            .iter()
            .find_map(|child| match *child {
                Element::Token(index)
                    if self.cst.token(index).kind == dotfile_syntax::TokenKind::PathRef =>
                {
                    Some((index, self.cst.string_data(index)))
                }
                _ => None,
            })
    }

    fn entry_has_direct_block(&self, node: NodeId) -> bool {
        self.cst.children(node).iter().any(|child| match *child {
            Element::Node(child) => self.cst.node_kind(child) == NodeKind::Block,
            Element::Token(_) | Element::Missing { .. } => false,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SortKey {
    category: u8,
    order: u16,
    text: Vec<u8>,
    optional: bool,
    ordinal: usize,
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.category
            .cmp(&other.category)
            .then_with(|| self.order.cmp(&other.order))
            .then_with(|| self.text.cmp(&other.text))
            // `false` (required) sorts before `true` (optional).
            .then_with(|| self.optional.cmp(&other.optional))
            .then_with(|| self.ordinal.cmp(&other.ordinal))
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StringPiece {
    Literal(String),
    Interpolation(String),
}

fn push_literal(pieces: &mut Vec<StringPiece>, text: &str) {
    match pieces.last_mut() {
        Some(StringPiece::Literal(existing)) => existing.push_str(text),
        Some(StringPiece::Interpolation(_)) | None => {
            pieces.push(StringPiece::Literal(text.to_owned()));
        }
    }
}

fn encode_string(pieces: &[StringPiece]) -> String {
    let mut output = String::from("\"");
    for piece in pieces {
        match piece {
            StringPiece::Interpolation(name) => {
                output.push_str("${");
                output.push_str(name);
                output.push('}');
            }
            StringPiece::Literal(text) => {
                let scalars: Vec<char> = text.chars().collect();
                for (index, scalar) in scalars.iter().copied().enumerate() {
                    match scalar {
                        '"' => output.push_str("\\\""),
                        '\\' => output.push_str("\\\\"),
                        '\n' => output.push_str("\\n"),
                        '\r' => output.push_str("\\r"),
                        '\t' => output.push_str("\\t"),
                        '\u{8}' => output.push_str("\\b"),
                        '\u{c}' => output.push_str("\\f"),
                        '$' if scalars.get(index + 1) == Some(&'{') => output.push_str("\\$"),
                        control if is_c0_or_c1(control) => {
                            output.push_str(&format!("\\u{{{:x}}}", control as u32));
                        }
                        scalar => output.push(scalar),
                    }
                }
            }
        }
    }
    output.push('"');
    output
}

fn is_c0_or_c1(scalar: char) -> bool {
    let value = scalar as u32;
    value <= 0x1f || (0x7f..=0x9f).contains(&value)
}

fn bare_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && component.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(byte, b'_' | b'.' | b'+' | b'-' | b'%' | b'@' | b'=')
                })
        })
}

fn valid_source_path(path: &str) -> bool {
    RepoPath::new(path).is_ok() && path.nfc().eq(path.chars())
}

fn resource_sort_text(kind: &str, block: Block<'_>) -> Option<Vec<u8>> {
    let mut keys = block.entries().into_iter().filter_map(|entry| {
        let Entry::Attribute(attribute) = entry else {
            return None;
        };
        if attribute.name() != Some("key") {
            return None;
        }
        Some(attribute.value())
    });
    let Value::Reference(reference) = keys.next()?? else {
        return None;
    };
    if keys.next().is_some() {
        return None;
    }
    let key = reference.name()?;
    Some(format!("{kind}\0{key}").into_bytes())
}

fn valid_resource_identity(entries: &[Entry<'_>], path: &[String]) -> Option<NodeId> {
    if path.last().map(String::as_str) != Some("resource_demand") {
        return None;
    }
    let mut identities = entries.iter().filter_map(|entry| {
        let Entry::Attribute(attribute) = entry else {
            return None;
        };
        (attribute.name() == Some("key")).then_some((entry.node_id(), attribute.value()))
    });
    let (node, value) = identities.next()?;
    if identities.next().is_some() || !matches!(value, Some(Value::Reference(_))) {
        return None;
    }
    Some(node)
}

fn is_host_extension_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z'))
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_byte_sort_identity(domain: Domain, path: &[&str], name: &str) -> bool {
    match domain {
        Domain::RecipientKeys if path == ["recipients"] => {
            let mut bytes = name.bytes();
            bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
                && bytes
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        }
        Domain::BenchmarkBaselines if path.len() == 1 => {
            name.len() == 8
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        }
        _ => true,
    }
}

fn published_entry(entry: &Entry<'_>) -> Option<(String, FormatPublishedKind, bool)> {
    match entry {
        Entry::Attribute(attribute) if attribute.value().is_some() => Some((
            format!("@{}", attribute.name()?),
            FormatPublishedKind::Attribute,
            false,
        )),
        Entry::SigilBlock(block) if block.block().is_some() => Some((
            format!("@{}", block.name()?),
            FormatPublishedKind::SigilBlock,
            block.optional(),
        )),
        Entry::Named(named) if named.value().is_some() && named.block().is_none() => Some((
            named.name()?.to_owned(),
            FormatPublishedKind::NamedValue,
            named.optional(),
        )),
        Entry::Named(named) if named.block().is_some() && named.value().is_none() => Some((
            named.name()?.to_owned(),
            FormatPublishedKind::NamedBlock,
            named.optional(),
        )),
        Entry::Let(_)
        | Entry::Extend(_)
        | Entry::Attribute(_)
        | Entry::SigilBlock(_)
        | Entry::Named(_)
        | Entry::Path(_)
        | Entry::Error(_) => None,
    }
}

fn child_path(path: &[String], name: &str) -> Vec<String> {
    let mut child = path.to_vec();
    child.push(name.to_owned());
    child
}

fn path_as_strs(path: &[String]) -> Vec<&str> {
    path.iter().map(String::as_str).collect()
}

fn append_comma(lines: &mut [String]) {
    if let Some(last) = lines.last_mut() {
        last.push(',');
    }
}

fn append_trailing_comments(lines: &mut Vec<String>, comments: &[Comment]) {
    for (index, comment) in comments.iter().enumerate() {
        if index == 0 {
            if let Some(last) = lines.last_mut() {
                last.push_str("  ");
                last.push_str(normalized_text(comment));
            }
        } else {
            let indent = lines
                .last()
                .map(|line| line.chars().take_while(|scalar| *scalar == ' ').count())
                .unwrap_or(0);
            lines.push(format!("{}{}", spaces(indent), normalized_text(comment)));
        }
    }
}

fn scalar_len(text: &str) -> usize {
    text.chars().count()
}

fn spaces(count: usize) -> String {
    " ".repeat(count)
}

fn push_blank(lines: &mut Vec<String>) {
    if !lines.is_empty() && lines.last().is_some_and(|line| !line.is_empty()) {
        lines.push(String::new());
    }
}

fn collapse_blank_lines(lines: &mut Vec<String>) {
    let mut previous_blank = false;
    lines.retain(|line| {
        let blank = line.is_empty();
        let keep = !blank || !previous_blank;
        previous_blank = blank;
        keep
    });
}

fn trim_blank_edges(lines: &mut Vec<String>) {
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
}

fn trim_blank_edges_after_first(lines: &mut Vec<String>) {
    while lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
}
