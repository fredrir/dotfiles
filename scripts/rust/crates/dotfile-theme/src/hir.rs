use std::collections::HashSet;

use dotfile_schema::{HirId, LoweredFile, Poison, PoisonKind};
use dotfile_source::{ByteRange, Diagnostic, Severity};
use dotfile_syntax::{Atom, Block, Cst, Element, Entry, File, NodeId, TokenKind, Value};
use serde_json::{Value as JsonValue, json};

use crate::ThemeFileKind;

/// A tolerant, source-ordered semantic arena for one registered theme file.
///
/// Present nodes reuse the authoritative IDs allocated by `dotfile-schema`
/// for their CST nodes. Required-but-absent semantic nodes receive synthetic
/// IDs in that same source map at zero-width insertion anchors.
#[derive(Clone, Debug)]
pub struct ThemeHir {
    root: HirId,
    nodes: Vec<ThemeHirNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeHirNode {
    pub id: HirId,
    pub parent: Option<HirId>,
    pub kind: ThemeHirNodeKind,
    pub range: ByteRange,
    pub children: Vec<HirId>,
    pub poison: Vec<Poison>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThemeHirNodeKind {
    Document(ThemeFileKind),
    Entry {
        name: Option<String>,
        authored: AuthoredShape,
        expected: Option<ThemeExpectation>,
    },
    Container(ThemeContainerKind),
    Value {
        authored: AuthoredValue,
        expected: Option<ThemeValueType>,
        decoded: Option<ThemeScalar>,
    },
    MissingField {
        name: String,
        expected: ThemeExpectation,
    },
    MissingChoice {
        choice: ThemeChoice,
    },
    MissingValue {
        expected: Option<ThemeExpectation>,
    },
    SyntaxMissing {
        expected: String,
    },
    SyntaxError,
    InvalidEntry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthoredShape {
    Assignment,
    Block,
    OptionalAssignment,
    OptionalBlock,
    Incomplete,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthoredValue {
    String,
    Reference,
    List,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThemeScalar {
    String(String),
    Reference(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeExpectation {
    Block(ThemeContainerKind),
    Value(ThemeValueType),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeChoice {
    ObsidianValueShape,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeValueType {
    LiteralString,
    NonemptyString,
    FontFamily,
    Appearance,
    PaletteReference,
    RoleReference,
    IdentifierReference,
    PositiveDecimal,
    UnitDecimal,
    ApplicationState,
    PaletteColor,
    PlainHexKey,
    ExternalKey,
    CssCustomProperty,
    ExtensionList,
    EzaExtension,
    ReferencePair,
    ObsidianDerived,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeContainerKind {
    RolesRoot,
    FontsRoot,
    ProfileRoot,
    CatppuccinRoot,
    EzaMapRoot,
    GtkRoot,
    KdeRoot,
    ObsidianRoot,
    OpenRoleMap,
    TerminalRoles,
    EzaRoles,
    EzaPattern,
    FontMapRequired,
    FontMapOverride,
    SizesRequired,
    SizesOverride,
    Applications,
    Nvim,
    Palette,
    CatppuccinEntries,
    CatppuccinEntry,
    EzaCategories,
    EzaCategory,
    GtkEntries,
    GtkEntry,
    KdeGroups,
    KdeGroupEntry,
    KdeRoleEntries,
    KdeRoleEntry,
    ObsidianDerived,
    ObsidianVariables,
    ObsidianVariable,
    Unknown,
}

impl ThemeHir {
    pub fn root(&self) -> HirId {
        self.root
    }

    pub fn nodes(&self) -> &[ThemeHirNode] {
        &self.nodes
    }

    pub fn node(&self, id: HirId) -> Option<&ThemeHirNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn has_poison(&self) -> bool {
        self.nodes.iter().any(|node| !node.poison.is_empty())
    }

    pub(crate) fn is_authoritative_for(&self, kind: ThemeFileKind, schema: &LoweredFile) -> bool {
        let Some(root) = self.node(self.root) else {
            return false;
        };
        if root.parent.is_some()
            || root.kind != ThemeHirNodeKind::Document(kind)
            || schema.hir().root.hir_id() != self.root
            || !schema.deferred_snapshot_is_authoritative()
        {
            return false;
        }

        let ids = self
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<HashSet<_>>();
        if ids.len() != self.nodes.len() {
            return false;
        }
        self.nodes.iter().all(|node| {
            let Some(origin) = schema.source_map().source_for_hir(node.id) else {
                return false;
            };
            if origin.range != node.range
                || node.range.end() > schema.source_len()
                || !schema
                    .source_map()
                    .hir_for_range(node.range)
                    .contains(&node.id)
                || origin.syntax.is_some_and(|syntax| {
                    !schema
                        .source_map()
                        .hir_for_syntax(syntax)
                        .contains(&node.id)
                })
                || node
                    .poison
                    .iter()
                    .any(|poison| poison.range.end() > schema.source_len())
            {
                return false;
            }
            let parent_is_consistent = node.parent.is_none_or(|parent| {
                self.node(parent)
                    .is_some_and(|parent| parent.children.contains(&node.id))
            });
            let children_are_consistent = node.children.iter().all(|child| {
                ids.contains(child)
                    && self
                        .node(*child)
                        .is_some_and(|child| child.parent == Some(node.id))
            });
            parent_is_consistent && children_are_consistent
        })
    }

    pub(crate) fn dump_json(&self, schema: &LoweredFile) -> JsonValue {
        let theme_ids = self
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<HashSet<_>>();
        let nodes = self
            .nodes
            .iter()
            .map(|node| {
                let origin = schema
                    .source_map()
                    .source_for_hir(node.id)
                    .expect("every theme HIR node is source-mapped");
                json!({
                    "id": node.id.get(),
                    "parent": node.parent.map(HirId::get),
                    "kind": kind_json(&node.kind),
                    "range": range_json(node.range),
                    "children": node.children.iter().map(|id| id.get()).collect::<Vec<_>>(),
                    "poison": node.poison.iter().map(poison_json).collect::<Vec<_>>(),
                    "origin": {
                        "authored": origin.syntax.is_some(),
                        "range": range_json(origin.range),
                    },
                })
            })
            .collect::<Vec<_>>();
        let source_map = self
            .nodes
            .iter()
            .map(|node| {
                let origin = schema
                    .source_map()
                    .source_for_hir(node.id)
                    .expect("every theme HIR node is source-mapped");
                let range_reverse = schema
                    .source_map()
                    .hir_for_range(origin.range)
                    .iter()
                    .filter(|id| theme_ids.contains(id))
                    .map(|id| id.get())
                    .collect::<Vec<_>>();
                let syntax_reverse = origin
                    .syntax
                    .map(|syntax| {
                        schema
                            .source_map()
                            .hir_for_syntax(syntax)
                            .iter()
                            .filter(|id| theme_ids.contains(id))
                            .map(|id| id.get())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                json!({
                    "hir_id": node.id.get(),
                    "range": range_json(origin.range),
                    "authored": origin.syntax.is_some(),
                    "theme_ids_for_range": range_reverse,
                    "theme_ids_for_syntax": syntax_reverse,
                })
            })
            .collect::<Vec<_>>();
        json!({ "root": self.root.get(), "nodes": nodes, "source_map": source_map })
    }
}

pub(crate) fn lower_theme_hir(
    kind: ThemeFileKind,
    file: File<'_>,
    cst: &Cst,
    schema: &mut LoweredFile,
    diagnostics: &[Diagnostic],
) -> ThemeHir {
    let root_owner = schema.hir().root.hir_id();
    let mut builder = Builder {
        cst,
        schema,
        nodes: Vec::new(),
    };
    let root_shape = root_container(kind);
    let root = builder.alloc_present(
        file.node_id(),
        file.range(),
        None,
        ThemeHirNodeKind::Document(kind),
        root_owner,
        Vec::new(),
    );
    let mut children = builder.lower_entries(file.entries(), root_shape, root, root);
    children.extend(builder.lower_direct_missing(file.node_id(), root, root));
    children.extend(builder.lower_root_unexpected(file.node_id(), root, root));
    builder.set_children(root, children);
    builder.attach_diagnostics(diagnostics);
    ThemeHir {
        root,
        nodes: builder.nodes,
    }
}

struct Builder<'a> {
    cst: &'a Cst,
    schema: &'a mut LoweredFile,
    nodes: Vec<ThemeHirNode>,
}

impl Builder<'_> {
    fn lower_entries(
        &mut self,
        entries: Vec<Entry<'_>>,
        container: ThemeContainerKind,
        parent: HirId,
        owner: HirId,
    ) -> Vec<HirId> {
        let mut children = Vec::new();
        let mut seen = HashSet::new();
        for entry in entries {
            match entry {
                Entry::Named(named) => {
                    if let Some(name) = named.name() {
                        seen.insert(name.to_owned());
                    }
                    children.push(self.lower_named(named, container, parent, owner));
                }
                other => {
                    let poison_kind = if matches!(other, Entry::Error(_)) {
                        PoisonKind::Syntax
                    } else {
                        PoisonKind::Context
                    };
                    let range = other.range();
                    let id = self.alloc_present(
                        other.node_id(),
                        range,
                        Some(parent),
                        ThemeHirNodeKind::InvalidEntry,
                        owner,
                        vec![Poison {
                            kind: poison_kind,
                            range,
                        }],
                    );
                    children.push(id);
                }
            }
        }

        let anchor = self.insertion_anchor_for_owner(owner);
        for (name, expected) in required_fields(container, &seen) {
            let range = ByteRange::at(anchor, self.cst.source_len())
                .expect("container insertion anchor is source-bounded");
            let id = self
                .schema
                .allocate_synthetic_hir(range)
                .expect("theme missing-field anchor is source-bounded");
            self.nodes.push(ThemeHirNode {
                id,
                parent: Some(parent),
                kind: ThemeHirNodeKind::MissingField {
                    name: name.to_owned(),
                    expected,
                },
                range,
                children: Vec::new(),
                poison: vec![Poison {
                    kind: PoisonKind::Missing,
                    range,
                }],
            });
            children.push(id);
        }
        for choice in required_choices(container, &seen) {
            let range = ByteRange::at(anchor, self.cst.source_len())
                .expect("container insertion anchor is source-bounded");
            let id = self
                .schema
                .allocate_synthetic_hir(range)
                .expect("theme missing-choice anchor is source-bounded");
            self.nodes.push(ThemeHirNode {
                id,
                parent: Some(parent),
                kind: ThemeHirNodeKind::MissingChoice { choice },
                range,
                children: Vec::new(),
                poison: vec![Poison {
                    kind: PoisonKind::Missing,
                    range,
                }],
            });
            children.push(id);
        }
        children
    }

    fn lower_named(
        &mut self,
        named: dotfile_syntax::NamedEntry<'_>,
        container: ThemeContainerKind,
        parent: HirId,
        owner: HirId,
    ) -> HirId {
        let name = named.name().map(str::to_owned);
        let expected = name
            .as_deref()
            .and_then(|name| expectation(container, name));
        let authored = match (named.optional(), named.value(), named.block()) {
            (false, Some(_), None) => AuthoredShape::Assignment,
            (false, None, Some(_)) => AuthoredShape::Block,
            (true, Some(_), None) => AuthoredShape::OptionalAssignment,
            (true, None, Some(_)) => AuthoredShape::OptionalBlock,
            (_, None, None) => AuthoredShape::Incomplete,
            _ => AuthoredShape::Other,
        };
        let range = named.range();
        let mut poison = Vec::new();
        if named.optional() || expected.is_none() {
            poison.push(Poison {
                kind: PoisonKind::Context,
                range,
            });
        }
        if !shape_matches(authored, expected) {
            poison.push(Poison {
                kind: PoisonKind::Context,
                range,
            });
        }
        dedup_poison(&mut poison);
        let id = self.alloc_present(
            named.node_id(),
            range,
            Some(parent),
            ThemeHirNodeKind::Entry {
                name,
                authored,
                expected,
            },
            owner,
            poison,
        );
        let mut children = Vec::new();
        if let Some(block) = named.block() {
            let block_kind = match expected {
                Some(ThemeExpectation::Block(kind)) => kind,
                _ => ThemeContainerKind::Unknown,
            };
            children.push(self.lower_block(block, block_kind, id, id));
        }
        if let Some(value) = named.value() {
            let value_type = match expected {
                Some(ThemeExpectation::Value(value_type)) => Some(value_type),
                _ => None,
            };
            children.push(self.lower_value(value, value_type, id, id));
        }
        if named.value().is_none() && named.block().is_none() {
            children.push(self.lower_missing_value(named.node_id(), expected, id, id));
        } else {
            children.extend(self.lower_direct_missing(named.node_id(), id, id));
        }
        children.extend(self.lower_direct_errors(named.node_id(), id, id));
        self.set_children(id, children);
        id
    }

    fn lower_block(
        &mut self,
        block: Block<'_>,
        kind: ThemeContainerKind,
        parent: HirId,
        owner: HirId,
    ) -> HirId {
        let id = self.alloc_present(
            block.node_id(),
            block.range(),
            Some(parent),
            ThemeHirNodeKind::Container(kind),
            owner,
            Vec::new(),
        );
        let mut children = self.lower_entries(block.entries(), kind, id, id);
        children.extend(self.lower_direct_missing(block.node_id(), id, id));
        children.extend(self.lower_direct_errors(block.node_id(), id, id));
        self.set_children(id, children);
        id
    }

    fn lower_value(
        &mut self,
        value: Value<'_>,
        expected: Option<ThemeValueType>,
        parent: HirId,
        owner: HirId,
    ) -> HirId {
        let (authored, decoded) = match value {
            Value::String(expression) => {
                let decoded = match expression.atoms().as_slice() {
                    [
                        Atom::String {
                            data: Some(data), ..
                        },
                    ] if !data.has_interpolation() => Some(ThemeScalar::String(data.decoded())),
                    _ => None,
                };
                (AuthoredValue::String, decoded)
            }
            Value::Reference(reference) => (
                AuthoredValue::Reference,
                reference
                    .name()
                    .map(|name| ThemeScalar::Reference(name.to_owned())),
            ),
            Value::List(_) => (AuthoredValue::List, None),
        };
        let range = value.range();
        let id = self.alloc_present(
            value.node_id(),
            range,
            Some(parent),
            ThemeHirNodeKind::Value {
                authored,
                expected,
                decoded,
            },
            owner,
            value_form_matches(authored, expected)
                .then(Vec::new)
                .unwrap_or_else(|| {
                    vec![Poison {
                        kind: PoisonKind::Value,
                        range,
                    }]
                }),
        );
        let mut children = Vec::new();
        if let Value::List(list) = value {
            let item_expected = match expected {
                Some(ThemeValueType::ExtensionList) => Some(ThemeValueType::EzaExtension),
                Some(ThemeValueType::ReferencePair) => Some(ThemeValueType::RoleReference),
                _ => None,
            };
            for item in list.values() {
                children.push(self.lower_value(item, item_expected, id, id));
            }
        }
        children.extend(self.lower_direct_missing(value.node_id(), id, id));
        children.extend(self.lower_direct_errors(value.node_id(), id, id));
        self.set_children(id, children);
        id
    }

    fn lower_missing_value(
        &mut self,
        syntax: NodeId,
        expected: Option<ThemeExpectation>,
        parent: HirId,
        owner: HirId,
    ) -> HirId {
        let missing = self
            .cst
            .children(syntax)
            .iter()
            .find_map(|element| match element {
                Element::Missing { at, .. } => ByteRange::at(*at, self.cst.source_len()),
                _ => None,
            });
        let range = missing.unwrap_or_else(|| {
            ByteRange::at(self.cst.node_range(syntax).end(), self.cst.source_len())
                .expect("entry end is source-bounded")
        });
        let id = self.synthetic_or_missing_id(range, owner);
        let poison = vec![
            Poison {
                kind: PoisonKind::Missing,
                range,
            },
            Poison {
                kind: PoisonKind::Syntax,
                range,
            },
        ];
        self.nodes.push(ThemeHirNode {
            id,
            parent: Some(parent),
            kind: ThemeHirNodeKind::MissingValue { expected },
            range,
            children: Vec::new(),
            poison,
        });
        id
    }

    fn lower_direct_missing(&mut self, syntax: NodeId, parent: HirId, owner: HirId) -> Vec<HirId> {
        let missing = self
            .cst
            .children(syntax)
            .iter()
            .filter_map(|element| match element {
                Element::Missing { kind, at } => Some((*kind, *at)),
                _ => None,
            })
            .collect::<Vec<_>>();
        missing
            .into_iter()
            .map(|(expected, at)| {
                let range = ByteRange::at(at, self.cst.source_len())
                    .expect("missing terminal is source-bounded");
                let id = self.synthetic_or_missing_id(range, owner);
                self.nodes.push(ThemeHirNode {
                    id,
                    parent: Some(parent),
                    kind: ThemeHirNodeKind::SyntaxMissing {
                        expected: expected.name().to_owned(),
                    },
                    range,
                    children: Vec::new(),
                    poison: vec![Poison {
                        kind: PoisonKind::Syntax,
                        range,
                    }],
                });
                id
            })
            .collect()
    }

    fn lower_direct_errors(&mut self, syntax: NodeId, parent: HirId, owner: HirId) -> Vec<HirId> {
        let ranges = self
            .cst
            .children(syntax)
            .iter()
            .filter_map(|element| match element {
                Element::Token(index) if self.cst.token(*index).kind == TokenKind::Error => {
                    Some(self.cst.token(*index).range)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        ranges
            .into_iter()
            .map(|range| {
                let id = self.synthetic_or_missing_id(range, owner);
                self.nodes.push(ThemeHirNode {
                    id,
                    parent: Some(parent),
                    kind: ThemeHirNodeKind::SyntaxError,
                    range,
                    children: Vec::new(),
                    poison: vec![Poison {
                        kind: PoisonKind::Syntax,
                        range,
                    }],
                });
                id
            })
            .collect()
    }

    fn lower_root_unexpected(&mut self, syntax: NodeId, parent: HirId, owner: HirId) -> Vec<HirId> {
        let ranges = self
            .cst
            .children(syntax)
            .iter()
            .filter_map(|element| match element {
                Element::Token(index) if self.cst.token(*index).kind != TokenKind::Newline => {
                    Some(self.cst.token(*index).range)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        ranges
            .into_iter()
            .map(|range| {
                let id = self.synthetic_or_missing_id(range, owner);
                self.nodes.push(ThemeHirNode {
                    id,
                    parent: Some(parent),
                    kind: ThemeHirNodeKind::SyntaxError,
                    range,
                    children: Vec::new(),
                    poison: vec![Poison {
                        kind: PoisonKind::Syntax,
                        range,
                    }],
                });
                id
            })
            .collect()
    }

    fn alloc_present(
        &mut self,
        syntax: NodeId,
        range: ByteRange,
        parent: Option<HirId>,
        kind: ThemeHirNodeKind,
        fallback_owner: HirId,
        poison: Vec<Poison>,
    ) -> HirId {
        let id = if parent.is_none() {
            fallback_owner
        } else {
            self.schema
                .source_map()
                .hir_for_syntax(syntax)
                .first()
                .copied()
                .expect("validated deferred source map covers every present theme node")
        };
        assert_eq!(
            self.schema
                .source_map()
                .source_for_hir(id)
                .map(|item| item.range),
            Some(range),
            "deferred source map must cover every present theme node"
        );
        assert!(
            self.nodes.iter().all(|node| node.id != id),
            "one schema HIR identity cannot represent two theme nodes"
        );
        self.nodes.push(ThemeHirNode {
            id,
            parent,
            kind,
            range,
            children: Vec::new(),
            poison,
        });
        id
    }

    fn synthetic_or_missing_id(&mut self, range: ByteRange, fallback: HirId) -> HirId {
        self.schema
            .source_map()
            .hir_for_range(range)
            .iter()
            .copied()
            .find(|id| {
                self.schema
                    .source_map()
                    .source_for_hir(*id)
                    .is_some_and(|origin| origin.syntax.is_none())
                    && self.nodes.iter().all(|node| node.id != *id)
            })
            .unwrap_or_else(|| {
                let id = self
                    .schema
                    .allocate_synthetic_hir(range)
                    .expect("theme missing-value anchor is source-bounded");
                debug_assert_ne!(id, fallback);
                id
            })
    }

    fn insertion_anchor_for_owner(&self, owner: HirId) -> u64 {
        let Some(origin) = self.schema.source_map().source_for_hir(owner) else {
            return self.cst.source_len();
        };
        let Some(syntax) = origin.syntax else {
            return origin.range.end();
        };
        self.cst
            .children(syntax)
            .iter()
            .filter_map(|element| match element {
                Element::Token(index) if self.cst.token(*index).kind == TokenKind::RightBrace => {
                    Some(self.cst.token(*index).range.start())
                }
                _ => None,
            })
            .next_back()
            .unwrap_or(origin.range.end())
    }

    fn set_children(&mut self, id: HirId, children: Vec<HirId>) {
        self.nodes
            .iter_mut()
            .find(|node| node.id == id)
            .expect("allocated theme node")
            .children = children;
    }

    fn attach_diagnostics(&mut self, diagnostics: &[Diagnostic]) {
        for diagnostic in diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
        {
            let Some(range) = ByteRange::new(
                diagnostic.primary_span.start_byte,
                diagnostic.primary_span.end_byte,
                self.cst.source_len(),
            ) else {
                continue;
            };
            let missing_name = missing_field_name(&diagnostic.summary);
            let missing_candidate = missing_name.and_then(|name| {
                self.nodes
                    .iter()
                    .filter(|node| {
                        matches!(
                            &node.kind,
                            ThemeHirNodeKind::MissingField { name: node_name, .. }
                                if name.ends_with(node_name)
                        )
                    })
                    .min_by_key(|node| {
                        (
                            node.range != range,
                            node.range.start().abs_diff(range.start()),
                            node.range.end().abs_diff(range.end()),
                            node.range.len(),
                        )
                    })
                    .map(|node| node.id)
            });
            let candidate = missing_candidate.or_else(|| {
                self.nodes
                    .iter()
                    .filter(|node| contains_range(node.range, range))
                    .min_by_key(|node| (node.range.len(), node_rank(&node.kind)))
                    .map(|node| node.id)
            });
            let Some(candidate) = candidate else {
                continue;
            };
            let kind = diagnostic_poison_kind(diagnostic, self.node(candidate));
            let node = self.node_mut(candidate);
            node.poison.push(Poison { kind, range });
            dedup_poison(&mut node.poison);
        }
    }

    fn node(&self, id: HirId) -> &ThemeHirNode {
        self.nodes
            .iter()
            .find(|node| node.id == id)
            .expect("allocated theme node")
    }

    fn node_mut(&mut self, id: HirId) -> &mut ThemeHirNode {
        self.nodes
            .iter_mut()
            .find(|node| node.id == id)
            .expect("allocated theme node")
    }
}

fn root_container(kind: ThemeFileKind) -> ThemeContainerKind {
    match kind {
        ThemeFileKind::Roles => ThemeContainerKind::RolesRoot,
        ThemeFileKind::Fonts => ThemeContainerKind::FontsRoot,
        ThemeFileKind::Profile => ThemeContainerKind::ProfileRoot,
        ThemeFileKind::CatppuccinMap => ThemeContainerKind::CatppuccinRoot,
        ThemeFileKind::EzaMap => ThemeContainerKind::EzaMapRoot,
        ThemeFileKind::GtkMap => ThemeContainerKind::GtkRoot,
        ThemeFileKind::KdeMap => ThemeContainerKind::KdeRoot,
        ThemeFileKind::ObsidianMap => ThemeContainerKind::ObsidianRoot,
    }
}

fn expectation(container: ThemeContainerKind, name: &str) -> Option<ThemeExpectation> {
    use ThemeContainerKind as C;
    use ThemeExpectation::{Block as B, Value as V};
    use ThemeValueType as T;

    match container {
        C::RolesRoot => match name {
            "roles" | "kde" | "konsole" => Some(B(C::OpenRoleMap)),
            "terminal" => Some(B(C::TerminalRoles)),
            "eza" => Some(B(C::EzaRoles)),
            _ => None,
        },
        C::FontsRoot => match name {
            "fonts" => Some(B(C::FontMapRequired)),
            "sizes" => Some(B(C::SizesRequired)),
            "applications" => Some(B(C::Applications)),
            _ => None,
        },
        C::ProfileRoot => match name {
            "display-name" | "icons" => Some(V(T::NonemptyString)),
            "appearance" => Some(V(T::Appearance)),
            "nvim" => Some(B(C::Nvim)),
            "palette" => Some(B(C::Palette)),
            "roles" | "kde" | "konsole" => Some(B(C::OpenRoleMap)),
            "terminal" => Some(B(C::TerminalRoles)),
            "eza" => Some(B(C::EzaRoles)),
            "fonts" => Some(B(C::FontMapOverride)),
            "sizes" => Some(B(C::SizesOverride)),
            "applications" => Some(B(C::Applications)),
            _ => None,
        },
        C::CatppuccinRoot => (name == "colors").then_some(B(C::CatppuccinEntries)),
        C::EzaMapRoot => (name == "categories").then_some(B(C::EzaCategories)),
        C::GtkRoot => (name == "colors").then_some(B(C::GtkEntries)),
        C::KdeRoot => match name {
            "groups" => Some(B(C::KdeGroups)),
            "foregrounds" | "selection-foregrounds" => Some(B(C::KdeRoleEntries)),
            _ => None,
        },
        C::ObsidianRoot => match name {
            "derived" => Some(B(C::ObsidianDerived)),
            "variables" => Some(B(C::ObsidianVariables)),
            _ => None,
        },
        C::OpenRoleMap => Some(V(T::PaletteReference)),
        C::TerminalRoles => match name {
            "ansi" | "tabs" => Some(B(C::OpenRoleMap)),
            _ => Some(V(T::PaletteReference)),
        },
        C::EzaRoles => match name {
            "categories" => Some(B(C::OpenRoleMap)),
            "pattern" => Some(B(C::EzaPattern)),
            _ => Some(V(T::PaletteReference)),
        },
        C::EzaPattern => match name {
            "key" => Some(V(T::LiteralString)),
            "role" => Some(V(T::RoleReference)),
            _ => None,
        },
        C::FontMapRequired | C::FontMapOverride => Some(V(T::FontFamily)),
        C::SizesRequired | C::SizesOverride => match name {
            "terminal" | "terminal_mac" | "interface" => Some(V(T::PositiveDecimal)),
            _ => None,
        },
        C::Applications => Some(V(T::ApplicationState)),
        C::Nvim => (name == "flavour").then_some(V(T::NonemptyString)),
        C::Palette => Some(V(T::PaletteColor)),
        C::CatppuccinEntries => (name == "entry").then_some(B(C::CatppuccinEntry)),
        C::CatppuccinEntry => match name {
            "key" => Some(V(T::PlainHexKey)),
            "palette" => Some(V(T::PaletteReference)),
            _ => None,
        },
        C::EzaCategories => (name == "category").then_some(B(C::EzaCategory)),
        C::EzaCategory => match name {
            "name" => Some(V(T::IdentifierReference)),
            "extensions" => Some(V(T::ExtensionList)),
            _ => None,
        },
        C::GtkEntries => (name == "entry").then_some(B(C::GtkEntry)),
        C::GtkEntry => match name {
            "key" => Some(V(T::ExternalKey)),
            "role" => Some(V(T::RoleReference)),
            _ => None,
        },
        C::KdeGroups => (name == "entry").then_some(B(C::KdeGroupEntry)),
        C::KdeGroupEntry => match name {
            "key" => Some(V(T::ExternalKey)),
            "roles" => Some(V(T::ReferencePair)),
            _ => None,
        },
        C::KdeRoleEntries => (name == "entry").then_some(B(C::KdeRoleEntry)),
        C::KdeRoleEntry => match name {
            "key" => Some(V(T::ExternalKey)),
            "role" => Some(V(T::RoleReference)),
            _ => None,
        },
        C::ObsidianDerived => (name == "source").then_some(V(T::PaletteReference)),
        C::ObsidianVariables => (name == "variable").then_some(B(C::ObsidianVariable)),
        C::ObsidianVariable => match name {
            "key" => Some(V(T::CssCustomProperty)),
            "palette" | "rgb" | "color" => Some(V(T::PaletteReference)),
            "alpha" => Some(V(T::UnitDecimal)),
            "derived" => Some(V(T::ObsidianDerived)),
            "literal" => Some(V(T::LiteralString)),
            _ => None,
        },
        C::Unknown => None,
    }
}

fn required_fields(
    container: ThemeContainerKind,
    seen: &HashSet<String>,
) -> Vec<(&'static str, ThemeExpectation)> {
    use ThemeContainerKind as C;
    use ThemeExpectation::{Block as B, Value as V};
    use ThemeValueType as T;

    let required: Vec<(&'static str, ThemeExpectation)> = match container {
        C::FontsRoot => vec![
            ("fonts", B(C::FontMapRequired)),
            ("sizes", B(C::SizesRequired)),
            ("applications", B(C::Applications)),
        ],
        C::ProfileRoot => vec![
            ("display-name", V(T::NonemptyString)),
            ("appearance", V(T::Appearance)),
            ("icons", V(T::NonemptyString)),
            ("nvim", B(C::Nvim)),
            ("palette", B(C::Palette)),
        ],
        C::CatppuccinRoot => vec![("colors", B(C::CatppuccinEntries))],
        C::EzaMapRoot => vec![("categories", B(C::EzaCategories))],
        C::GtkRoot => vec![("colors", B(C::GtkEntries))],
        C::KdeRoot => vec![
            ("groups", B(C::KdeGroups)),
            ("foregrounds", B(C::KdeRoleEntries)),
            ("selection-foregrounds", B(C::KdeRoleEntries)),
        ],
        C::ObsidianRoot => vec![
            ("derived", B(C::ObsidianDerived)),
            ("variables", B(C::ObsidianVariables)),
        ],
        C::EzaPattern => vec![("key", V(T::LiteralString)), ("role", V(T::RoleReference))],
        C::FontMapRequired => vec![("general", V(T::FontFamily)), ("nerd", V(T::FontFamily))],
        C::SizesRequired => vec![
            ("terminal", V(T::PositiveDecimal)),
            ("terminal_mac", V(T::PositiveDecimal)),
            ("interface", V(T::PositiveDecimal)),
        ],
        C::Nvim => vec![("flavour", V(T::NonemptyString))],
        C::CatppuccinEntry => vec![
            ("key", V(T::PlainHexKey)),
            ("palette", V(T::PaletteReference)),
        ],
        C::EzaCategory => vec![
            ("name", V(T::IdentifierReference)),
            ("extensions", V(T::ExtensionList)),
        ],
        C::GtkEntry | C::KdeRoleEntry => {
            vec![("key", V(T::ExternalKey)), ("role", V(T::RoleReference))]
        }
        C::KdeGroupEntry => vec![("key", V(T::ExternalKey)), ("roles", V(T::ReferencePair))],
        C::ObsidianDerived => vec![("source", V(T::PaletteReference))],
        C::ObsidianVariable => {
            let mut fields = vec![("key", V(T::CssCustomProperty))];
            if seen.contains("color") && !seen.contains("alpha") {
                fields.push(("alpha", V(T::UnitDecimal)));
            }
            fields
        }
        _ => Vec::new(),
    };
    required
        .into_iter()
        .filter(|(name, _)| !seen.contains(*name))
        .collect()
}

fn required_choices(container: ThemeContainerKind, seen: &HashSet<String>) -> Vec<ThemeChoice> {
    if container == ThemeContainerKind::ObsidianVariable
        && !["palette", "rgb", "color", "derived", "literal"]
            .iter()
            .any(|name| seen.contains(*name))
    {
        vec![ThemeChoice::ObsidianValueShape]
    } else {
        Vec::new()
    }
}

fn shape_matches(authored: AuthoredShape, expected: Option<ThemeExpectation>) -> bool {
    matches!(
        (authored, expected),
        (AuthoredShape::Assignment, Some(ThemeExpectation::Value(_)))
            | (AuthoredShape::Block, Some(ThemeExpectation::Block(_)))
    )
}

fn value_form_matches(authored: AuthoredValue, expected: Option<ThemeValueType>) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    match expected {
        ThemeValueType::PaletteReference
        | ThemeValueType::RoleReference
        | ThemeValueType::IdentifierReference
        | ThemeValueType::ObsidianDerived => authored == AuthoredValue::Reference,
        ThemeValueType::ExtensionList | ThemeValueType::ReferencePair => {
            authored == AuthoredValue::List
        }
        _ => authored == AuthoredValue::String,
    }
}

fn contains_range(outer: ByteRange, inner: ByteRange) -> bool {
    outer.start() <= inner.start() && outer.end() >= inner.end()
}

fn missing_field_name(summary: &str) -> Option<&str> {
    let prefix = "missing required theme field `";
    summary
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix('`'))
}

fn node_rank(kind: &ThemeHirNodeKind) -> u8 {
    match kind {
        ThemeHirNodeKind::MissingField { .. }
        | ThemeHirNodeKind::MissingChoice { .. }
        | ThemeHirNodeKind::MissingValue { .. }
        | ThemeHirNodeKind::SyntaxMissing { .. }
        | ThemeHirNodeKind::SyntaxError => 0,
        ThemeHirNodeKind::Value { .. } => 1,
        ThemeHirNodeKind::Entry { .. } => 2,
        ThemeHirNodeKind::InvalidEntry => 3,
        ThemeHirNodeKind::Container(_) => 4,
        ThemeHirNodeKind::Document(_) => 5,
    }
}

fn diagnostic_poison_kind(diagnostic: &Diagnostic, node: &ThemeHirNode) -> PoisonKind {
    if matches!(
        diagnostic.stage,
        dotfile_source::Stage::Lex | dotfile_source::Stage::Parse
    ) {
        PoisonKind::Syntax
    } else if diagnostic.code == "schema/duplicate" || diagnostic.summary.contains("duplicate") {
        PoisonKind::Duplicate
    } else if diagnostic.summary.starts_with("missing required") {
        PoisonKind::Missing
    } else if matches!(node.kind, ThemeHirNodeKind::Value { .. }) {
        PoisonKind::Value
    } else {
        PoisonKind::Context
    }
}

fn dedup_poison(poison: &mut Vec<Poison>) {
    poison.sort_by_key(|item| (item.range, item.kind.as_str()));
    poison.dedup();
}

fn kind_json(kind: &ThemeHirNodeKind) -> JsonValue {
    match kind {
        ThemeHirNodeKind::Document(kind) => json!({ "document": format!("{kind:?}") }),
        ThemeHirNodeKind::Entry {
            name,
            authored,
            expected,
        } => json!({
            "entry": name,
            "authored": format!("{authored:?}"),
            "expected": expected.map(|value| format!("{value:?}")),
        }),
        ThemeHirNodeKind::Container(kind) => json!({ "container": format!("{kind:?}") }),
        ThemeHirNodeKind::Value {
            authored,
            expected,
            decoded,
        } => json!({
            "value": format!("{authored:?}"),
            "expected": expected.map(|value| format!("{value:?}")),
            "decoded": decoded.as_ref().map(|value| match value {
                ThemeScalar::String(value) | ThemeScalar::Reference(value) => value,
            }),
        }),
        ThemeHirNodeKind::MissingField { name, expected } => {
            json!({ "missing_field": name, "expected": format!("{expected:?}") })
        }
        ThemeHirNodeKind::MissingChoice { choice } => {
            json!({ "missing_choice": format!("{choice:?}") })
        }
        ThemeHirNodeKind::MissingValue { expected } => {
            json!({ "missing_value": expected.map(|value| format!("{value:?}")) })
        }
        ThemeHirNodeKind::SyntaxMissing { expected } => {
            json!({ "syntax_missing": expected })
        }
        ThemeHirNodeKind::SyntaxError => json!("syntax_error"),
        ThemeHirNodeKind::InvalidEntry => json!("invalid_entry"),
    }
}

fn range_json(range: ByteRange) -> JsonValue {
    json!([range.start(), range.end()])
}

fn poison_json(poison: &Poison) -> JsonValue {
    json!({ "kind": poison.kind.as_str(), "range": range_json(poison.range) })
}
