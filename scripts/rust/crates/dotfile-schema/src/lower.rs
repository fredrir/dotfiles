use std::collections::{BTreeMap, HashMap};

use dotfile_source::{
    ByteRange, Diagnostic, RepoPath, Severity, SourceText, Span, Stage, sort_diagnostics,
};
use dotfile_syntax::{
    Atom, Attribute as AstAttribute, Block, Element, Entry, File as AstFile, NodeId, NodeKind,
    Parse, StringExpr, StringSegment as SyntaxStringSegment, Value as AstValue,
};

use crate::classifier::{
    ClassifiedPath, Domain, DomainClassifier, DomainLocation, PathClassification,
};
use crate::hir::*;
use crate::validate;

/// Failure to establish the preconditions for schema lowering.
///
/// In particular, a lossless syntax tree and its diagnostics are only
/// meaningful with the exact repository path and source bytes from which
/// they were parsed. This error keeps that invariant at the public
/// syntax-to-HIR boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoweringError {
    MismatchedParse,
}

impl std::fmt::Display for LoweringError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MismatchedParse => {
                formatter.write_str("parse was built from a different path or source bytes")
            }
        }
    }
}

impl std::error::Error for LoweringError {}

/// Lowers one already-classified file.  The returned HIR owns every string
/// and collection; it never retains an AST/CST reference.
#[cfg(test)]
pub(crate) fn lower(
    path: &RepoPath,
    source: &SourceText,
    parse: &Parse,
    classification: ClassifiedPath,
) -> Result<LoweredFile, LoweringError> {
    check_parse_identity(path, source, parse)?;
    Ok(Lowerer::new(path, source, parse).lower_known(classification))
}

/// Lowers a complete classifier result.  Unknown repository-owned
/// `.dotfile` paths receive the frozen `schema/context` rejection.
pub fn lower_path(
    path: &RepoPath,
    source: &SourceText,
    parse: &Parse,
    classifier: &DomainClassifier,
) -> Result<LoweredFile, LoweringError> {
    check_parse_identity(path, source, parse)?;
    Ok(match classifier.classify(path) {
        PathClassification::Known(classified) => {
            Lowerer::new(path, source, parse).lower_known(classified)
        }
        PathClassification::UnknownDotfile => Lowerer::new(path, source, parse).lower_unknown(),
        PathClassification::NotDotfile => Lowerer::new(path, source, parse).lower_unowned(),
    })
}

fn check_parse_identity(
    path: &RepoPath,
    source: &SourceText,
    parse: &Parse,
) -> Result<(), LoweringError> {
    if parse.was_parsed_from(path, source) {
        Ok(())
    } else {
        Err(LoweringError::MismatchedParse)
    }
}

struct Lowerer<'a> {
    path: &'a RepoPath,
    source: &'a SourceText,
    parse: &'a Parse,
    source_map: SourceMap,
    diagnostics: Vec<Diagnostic>,
    scopes: Vec<BindingScope>,
    bindings: Vec<BindingDeclaration>,
    binding_entries: HashMap<NodeId, BindingId>,
    recovery: Vec<RecoveryNode>,
}

impl<'a> Lowerer<'a> {
    fn new(path: &'a RepoPath, source: &'a SourceText, parse: &'a Parse) -> Self {
        Self {
            path,
            source,
            parse,
            source_map: SourceMap::default(),
            diagnostics: Vec::new(),
            scopes: Vec::new(),
            bindings: Vec::new(),
            binding_entries: HashMap::new(),
            recovery: Vec::new(),
        }
    }

    fn lower_known(mut self, classification: ClassifiedPath) -> LoweredFile {
        let ast = self.parse.ast(self.source);
        let root = match classification.domain {
            Domain::Profiles => HirRoot::Profiles(Box::new(self.lower_profiles(ast))),
            Domain::Hosts => HirRoot::Hosts(Box::new(self.lower_hosts(ast))),
            Domain::GroupRootRequirements | Domain::FacetRequirements | Domain::OverrideVariant => {
                HirRoot::Requirements(Box::new(
                    self.lower_requirements(ast, classification.domain),
                ))
            }
            Domain::RecipientKeys => HirRoot::RecipientKeys(Box::new(self.lower_recipients(ast))),
            Domain::SecretScanRules => {
                HirRoot::SecretScanRules(Box::new(self.lower_scan_rules(ast)))
            }
            Domain::BenchmarkBaselines => {
                HirRoot::BenchmarkBaselines(Box::new(self.lower_benchmarks(ast)))
            }
            domain => HirRoot::Deferred(self.lower_deferred(ast, domain, &classification.location)),
        };
        self.finish_unused_warnings();
        sort_diagnostics(&mut self.diagnostics);
        let mut poison = Vec::new();
        if self.parse.has_errors() {
            poison.push(Poison {
                kind: PoisonKind::Syntax,
                range: ast.range(),
            });
        }
        if self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
        {
            poison.push(Poison {
                kind: PoisonKind::Context,
                range: ast.range(),
            });
        }
        LoweredFile {
            path: self.path.clone(),
            parsed_source: self.source.clone(),
            classification: Some(classification),
            hir: HirFile {
                root,
                scopes: self.scopes,
                bindings: self.bindings,
                recovery: self.recovery,
                poison,
            },
            source_map: self.source_map,
            diagnostics: self.diagnostics,
        }
    }

    fn lower_unknown(mut self) -> LoweredFile {
        let ast = self.parse.ast(self.source);
        let id = self.alloc(Some(ast.node_id()), ast.range());
        self.context(
            ast.range(),
            "repository-owned source has no registered schema",
            "move the file to a registered domain path or rename it",
        );
        let context_poison = Poison {
            kind: PoisonKind::Context,
            range: ast.range(),
        };
        let mut file_poison = Vec::new();
        if self.parse.has_errors() {
            file_poison.push(Poison {
                kind: PoisonKind::Syntax,
                range: ast.range(),
            });
        }
        file_poison.push(context_poison.clone());
        LoweredFile {
            path: self.path.clone(),
            parsed_source: self.source.clone(),
            classification: None,
            hir: HirFile {
                root: HirRoot::Unknown(PoisonNode {
                    id,
                    range: ast.range(),
                    poison: file_poison.clone(),
                }),
                scopes: Vec::new(),
                bindings: Vec::new(),
                recovery: self.recovery,
                poison: file_poison,
            },
            source_map: self.source_map,
            diagnostics: self.diagnostics,
        }
    }

    fn lower_unowned(mut self) -> LoweredFile {
        let ast = self.parse.ast(self.source);
        let id = self.alloc(Some(ast.node_id()), ast.range());
        LoweredFile {
            path: self.path.clone(),
            parsed_source: self.source.clone(),
            classification: None,
            hir: HirFile {
                root: HirRoot::Unknown(PoisonNode {
                    id,
                    range: ast.range(),
                    poison: Vec::new(),
                }),
                scopes: Vec::new(),
                bindings: Vec::new(),
                recovery: self.recovery,
                poison: if self.parse.has_errors() {
                    vec![Poison {
                        kind: PoisonKind::Syntax,
                        range: ast.range(),
                    }]
                } else {
                    Vec::new()
                },
            },
            source_map: self.source_map,
            diagnostics: Vec::new(),
        }
    }

    fn alloc(&mut self, syntax: Option<NodeId>, range: ByteRange) -> HirId {
        self.source_map.allocate(syntax, range)
    }

    fn retain_poison(&mut self, syntax: NodeId, range: ByteRange, kind: PoisonKind) -> Poison {
        let poison = Poison { kind, range };
        let id = self.alloc(Some(syntax), range);
        self.recovery.push(RecoveryNode::Poison(PoisonNode {
            id,
            range,
            poison: vec![poison.clone()],
        }));
        poison
    }

    fn retain_attribute(&mut self, attribute: Attribute) {
        self.recovery.push(RecoveryNode::Attribute(attribute));
    }

    fn retain_named_field(&mut self, field: NamedField) {
        self.recovery.push(RecoveryNode::NamedField(field));
    }

    fn span(&self, range: ByteRange) -> Span {
        Span::new(self.path, range, self.source, self.parse.line_index())
    }

    fn emit(
        &mut self,
        code: &'static str,
        severity: Severity,
        range: ByteRange,
        summary: impl Into<String>,
        remedy: impl Into<String>,
    ) {
        self.diagnostics.push(Diagnostic::new(
            code,
            Stage::Schema,
            severity,
            summary,
            remedy,
            self.span(range),
        ));
    }

    fn context(&mut self, range: ByteRange, summary: impl Into<String>, remedy: impl Into<String>) {
        self.emit("schema/context", Severity::Error, range, summary, remedy);
    }

    fn duplicate(&mut self, range: ByteRange, previous: &[ByteRange], summary: impl Into<String>) {
        let mut diagnostic = Diagnostic::new(
            "schema/duplicate",
            Stage::Schema,
            Severity::Error,
            summary,
            "remove the duplicate entry",
            self.span(range),
        );
        let mut previous = previous.to_vec();
        previous.sort_unstable();
        diagnostic.related_spans = previous.into_iter().map(|range| self.span(range)).collect();
        self.diagnostics.push(diagnostic);
    }

    fn record_origin<K: Ord>(
        origins: &mut BTreeMap<K, Vec<ByteRange>>,
        key: K,
        range: ByteRange,
    ) -> Vec<ByteRange> {
        let ranges = origins.entry(key).or_default();
        let previous = ranges.clone();
        ranges.push(range);
        previous
    }

    fn record_singleton(origins: &mut Vec<ByteRange>, range: ByteRange) -> Vec<ByteRange> {
        let previous = origins.clone();
        origins.push(range);
        previous
    }

    fn binding_error(
        &mut self,
        range: ByteRange,
        summary: impl Into<String>,
        remedy: impl Into<String>,
    ) {
        self.emit("schema/binding", Severity::Error, range, summary, remedy);
    }

    // -- lexical scopes -------------------------------------------------

    fn prepare_scope(
        &mut self,
        syntax: NodeId,
        range: ByteRange,
        entries: &[Entry<'a>],
        parent: Option<ScopeId>,
        bindings_allowed: bool,
    ) -> ScopeId {
        let hir_id = self.alloc(Some(syntax), range);
        let id = ScopeId(self.scopes.len() as u32);
        let prologue_end = entries
            .iter()
            .find(|entry| !matches!(entry, Entry::Let(_)))
            .map_or(range.end(), |entry| entry.range().start());
        self.scopes.push(BindingScope {
            id,
            hir_id,
            parent,
            range,
            prologue_end,
            bindings: Vec::new(),
            poison: Vec::new(),
        });

        let mut in_prologue = true;
        let mut first_names: BTreeMap<String, Vec<ByteRange>> = BTreeMap::new();
        let mut declarations = Vec::new();
        for entry in entries {
            let Entry::Let(declaration) = entry else {
                in_prologue = false;
                continue;
            };
            let binding_id = BindingId(self.bindings.len() as u32);
            let binding_hir = self.alloc(Some(declaration.node_id()), declaration.range());
            let name = declaration.name().unwrap_or("").to_owned();
            let name_range = declaration.name_range().unwrap_or(declaration.range());
            let value_range = declaration.value_range().unwrap_or(declaration.range());
            let mut poison = Vec::new();
            if !bindings_allowed {
                self.context(
                    declaration.range(),
                    "binding declaration is not legal in this schema context",
                    "remove the binding declaration",
                );
                poison.push(Poison {
                    kind: PoisonKind::Context,
                    range: declaration.range(),
                });
            }
            if !in_prologue {
                self.binding_error(
                    declaration.range(),
                    "binding declaration appears after the block prologue",
                    "move the binding before the first non-binding entry",
                );
                poison.push(Poison {
                    kind: PoisonKind::Binding,
                    range: declaration.range(),
                });
            }
            let mut previous = Self::record_origin(&mut first_names, name.clone(), name_range);
            if !previous.is_empty() {
                previous.sort_unstable();
                let mut diagnostic = Diagnostic::new(
                    "schema/binding",
                    Stage::Schema,
                    Severity::Error,
                    "binding is redeclared in the same scope",
                    "rename or remove the later declaration",
                    self.span(name_range),
                );
                diagnostic.related_spans =
                    previous.into_iter().map(|range| self.span(range)).collect();
                self.diagnostics.push(diagnostic);
                poison.push(Poison {
                    kind: PoisonKind::Binding,
                    range: name_range,
                });
            }
            let empty = EvaluatedString {
                value: String::new(),
                segments: Vec::new(),
                poisoned: true,
            };
            self.bindings.push(BindingDeclaration {
                id: binding_id,
                hir_id: binding_hir,
                scope: id,
                name,
                name_range,
                range: declaration.range(),
                initializer: StringExpression {
                    range: value_range,
                    segments: Vec::new(),
                    evaluated: empty.clone(),
                },
                evaluated: empty,
                used: false,
                poison,
            });
            self.binding_entries
                .insert(declaration.node_id(), binding_id);
            self.scopes[id.0 as usize].bindings.push(binding_id);
            declarations.push((*declaration, binding_id));
        }

        for (declaration, binding_id) in declarations {
            let expression = match declaration.value() {
                Some(value) => self.lower_expression(
                    value,
                    id,
                    Some(binding_id),
                    binding_hir_for(&self.bindings, binding_id),
                ),
                None => {
                    self.binding_error(
                        declaration.range(),
                        "binding initializer is missing",
                        "assign a string expression",
                    );
                    let range = declaration.value_range().unwrap_or(declaration.range());
                    StringExpression {
                        range,
                        segments: vec![ExpressionSegment::Poison { range }],
                        evaluated: EvaluatedString {
                            value: String::new(),
                            segments: Vec::new(),
                            poisoned: true,
                        },
                    }
                }
            };
            let evaluated = expression.evaluated.clone();
            let binding = &mut self.bindings[binding_id.0 as usize];
            binding.initializer = expression;
            binding.evaluated = evaluated;
        }
        id
    }

    fn lower_expression(
        &mut self,
        expression: StringExpr<'a>,
        scope: ScopeId,
        current: Option<BindingId>,
        owner: HirId,
    ) -> StringExpression {
        let mut segments = Vec::new();
        for atom in expression.atoms() {
            match atom {
                Atom::String { data, range, .. } => match data {
                    Some(data) => {
                        for segment in &data.segments {
                            match segment {
                                SyntaxStringSegment::Literal { text, range } => {
                                    self.source_map.insert_range(owner, *range);
                                    segments.push(ExpressionSegment::Literal {
                                        text: text.clone(),
                                        range: *range,
                                    });
                                }
                                SyntaxStringSegment::Interpolation {
                                    name,
                                    range,
                                    name_range,
                                } => {
                                    self.source_map.insert_range(owner, *range);
                                    segments.push(ExpressionSegment::Binding {
                                        name: name.clone(),
                                        range: *name_range,
                                        resolution: BindingResolution::Poison,
                                    });
                                }
                            }
                        }
                    }
                    None => segments.push(ExpressionSegment::Poison { range }),
                },
                Atom::Var(variable) => {
                    let range = variable.name_range().unwrap_or(variable.range());
                    self.source_map.insert_range(owner, variable.range());
                    segments.push(ExpressionSegment::Binding {
                        name: variable.name().unwrap_or("").to_owned(),
                        range,
                        resolution: BindingResolution::Poison,
                    });
                }
            }
        }
        if segments.is_empty() {
            segments.push(ExpressionSegment::Poison {
                range: expression.range(),
            });
        }
        let evaluated = self.resolve_segments(&mut segments, scope, current);
        StringExpression {
            range: expression.range(),
            segments,
            evaluated,
        }
    }

    fn resolve_segments(
        &mut self,
        segments: &mut [ExpressionSegment],
        scope: ScopeId,
        current: Option<BindingId>,
    ) -> EvaluatedString {
        let mut output = EvaluatedString {
            value: String::new(),
            segments: Vec::new(),
            poisoned: false,
        };
        for segment in segments {
            match segment {
                ExpressionSegment::Literal { text, range } => {
                    output.value.push_str(text);
                    output.segments.push(EvaluatedSegment {
                        text: text.clone(),
                        source_range: *range,
                        binding_edges: Vec::new(),
                    });
                }
                ExpressionSegment::Poison { .. } => output.poisoned = true,
                ExpressionSegment::Binding {
                    name,
                    range,
                    resolution,
                } => {
                    let resolved = self.lookup_binding(scope, name, current, range.start());
                    *resolution = resolved.clone();
                    match resolved {
                        BindingResolution::Resolved(binding_id) => {
                            let binding = &mut self.bindings[binding_id.0 as usize];
                            binding.used = true;
                            let declaration_range = binding.name_range;
                            let evaluated = binding.evaluated.clone();
                            let edge = BindingEdge {
                                binding: binding_id,
                                declaration_range,
                                reference_range: *range,
                            };
                            output.value.push_str(&evaluated.value);
                            output.poisoned |= evaluated.poisoned;
                            for mut expanded in evaluated.segments {
                                expanded.binding_edges.insert(0, edge.clone());
                                output.segments.push(expanded);
                            }
                        }
                        BindingResolution::SelfReference => {
                            self.binding_error(
                                *range,
                                "binding initializer refers to itself",
                                "refer only to outer or earlier bindings",
                            );
                            output.poisoned = true;
                        }
                        BindingResolution::UseBeforeDeclaration(_) => {
                            self.binding_error(
                                *range,
                                "binding is used before its declaration",
                                "move the declaration earlier in the prologue",
                            );
                            output.poisoned = true;
                        }
                        BindingResolution::Unbound => {
                            self.binding_error(
                                *range,
                                format!("binding `{name}` is not declared in this lexical scope"),
                                "declare it in this block prologue or an enclosing prologue",
                            );
                            output.poisoned = true;
                        }
                        BindingResolution::Poison => output.poisoned = true,
                    }
                }
            }
        }
        output
    }

    fn lookup_binding(
        &self,
        scope: ScopeId,
        name: &str,
        current: Option<BindingId>,
        reference_start: u64,
    ) -> BindingResolution {
        let scope_data = &self.scopes[scope.0 as usize];
        if let Some(current_id) = current {
            let current_binding = &self.bindings[current_id.0 as usize];
            if current_binding.name == name {
                return match scope_data
                    .parent
                    .map(|parent| self.lookup_binding(parent, name, None, reference_start))
                {
                    Some(BindingResolution::Resolved(binding)) => {
                        BindingResolution::Resolved(binding)
                    }
                    Some(BindingResolution::UseBeforeDeclaration(binding)) => {
                        BindingResolution::UseBeforeDeclaration(binding)
                    }
                    _ => BindingResolution::SelfReference,
                };
            }
            let current_index = scope_data
                .bindings
                .iter()
                .position(|id| *id == current_id)
                .unwrap_or(scope_data.bindings.len());
            if let Some(found) = scope_data.bindings[..current_index]
                .iter()
                .rev()
                .find(|id| self.bindings[id.0 as usize].name == name)
            {
                return BindingResolution::Resolved(*found);
            }
            let outer = scope_data
                .parent
                .map(|parent| self.lookup_binding(parent, name, None, reference_start));
            if let Some(BindingResolution::Resolved(binding)) = outer {
                return BindingResolution::Resolved(binding);
            }
            if let Some(found) = scope_data.bindings[current_index.saturating_add(1)..]
                .iter()
                .find(|id| self.bindings[id.0 as usize].name == name)
            {
                return BindingResolution::UseBeforeDeclaration(*found);
            }
            if let Some(resolution) = outer {
                return resolution;
            }
        } else {
            if let Some(found) = scope_data.bindings.iter().rev().find(|id| {
                let binding = &self.bindings[id.0 as usize];
                binding.name == name && binding.range.end() <= reference_start
            }) {
                return BindingResolution::Resolved(*found);
            }
            let outer = scope_data
                .parent
                .map(|parent| self.lookup_binding(parent, name, None, reference_start));
            if let Some(BindingResolution::Resolved(binding)) = outer {
                return BindingResolution::Resolved(binding);
            }
            if let Some(found) = scope_data.bindings.iter().find(|id| {
                let binding = &self.bindings[id.0 as usize];
                binding.name == name && binding.range.end() > reference_start
            }) {
                return BindingResolution::UseBeforeDeclaration(*found);
            }
            if let Some(resolution) = outer {
                return resolution;
            }
        }
        match scope_data.parent {
            Some(parent) => self.lookup_binding(parent, name, None, reference_start),
            None => BindingResolution::Unbound,
        }
    }

    fn finish_unused_warnings(&mut self) {
        let unused: Vec<_> = self
            .bindings
            .iter()
            .filter(|binding| !binding.used && binding.poison.is_empty())
            .map(|binding| (binding.name.clone(), binding.name_range))
            .collect();
        for (name, range) in unused {
            let diagnostic = Diagnostic::new(
                "schema/binding",
                Stage::Schema,
                Severity::Warning,
                format!("unused binding `{name}`"),
                "remove the binding or reference it from a string expression",
                self.span(range),
            )
            .with_detail("unused_binding");
            self.diagnostics.push(diagnostic);
        }
    }

    // -- generic value validation --------------------------------------

    fn lower_value(
        &mut self,
        value: Option<AstValue<'a>>,
        expected: ValueType,
        scope: ScopeId,
        fallback_syntax: NodeId,
        fallback_range: ByteRange,
    ) -> HirValue {
        let Some(value) = value else {
            self.context(
                fallback_range,
                format!("missing value of type `{}`", expected.as_str()),
                "provide the required value",
            );
            let id = self.alloc(Some(fallback_syntax), fallback_range);
            return HirValue {
                id,
                value_type: expected,
                kind: HirValueKind::Missing,
                range: fallback_range,
                poison: vec![Poison {
                    kind: PoisonKind::Missing,
                    range: fallback_range,
                }],
            };
        };
        let id = self.alloc(Some(value.node_id()), value.range());
        let mut lowered = match expected {
            ValueType::ThemeReference => {
                self.lower_reference_value(value, expected, ReferenceNamespace::Theme, id)
            }
            ValueType::ProfileReference => {
                self.lower_reference_value(value, expected, ReferenceNamespace::Profile, id)
            }
            ValueType::ResourceKeyReference => {
                self.lower_reference_value(value, expected, ReferenceNamespace::ResourceKey, id)
            }
            ValueType::GroupReferenceList => {
                self.lower_reference_list(value, expected, ReferenceNamespace::Group, id)
            }
            ValueType::ArchitectureList => self.lower_string_list(value, expected, scope, id, true),
            ValueType::Hostnames => self.lower_string_list(value, expected, scope, id, true),
            ValueType::StringOrStringList | ValueType::HostExtension => {
                self.lower_normalized_string_list(value, expected, scope, id)
            }
            ValueType::StringExpression | ValueType::MachinePath | ValueType::DestinationPath => {
                self.lower_string_value(value, expected, scope, id, false)
            }
            _ => self.lower_string_value(value, expected, scope, id, true),
        };
        self.validate_value(&mut lowered);
        lowered
    }

    fn lower_string_value(
        &mut self,
        value: AstValue<'a>,
        expected: ValueType,
        scope: ScopeId,
        id: HirId,
        single_token: bool,
    ) -> HirValue {
        let range = value.range();
        let mut poison = Vec::new();
        let kind = if let AstValue::String(expression) = value {
            if single_token && !is_plain_single_string(expression) {
                self.context(
                    expression.range(),
                    format!(
                        "`{}` requires one uninterpolated quoted string",
                        expected.as_str()
                    ),
                    "quote one literal string value",
                );
                poison.push(Poison {
                    kind: PoisonKind::Value,
                    range: expression.range(),
                });
            }
            HirValueKind::String(self.lower_expression(expression, scope, None, id))
        } else {
            self.context(
                range,
                format!(
                    "expected `{}`, found a bare reference or list",
                    expected.as_str()
                ),
                "use the value shape required by this field",
            );
            poison.push(Poison {
                kind: PoisonKind::Value,
                range,
            });
            HirValueKind::Missing
        };
        HirValue {
            id,
            value_type: expected,
            kind,
            range,
            poison,
        }
    }

    fn lower_reference_value(
        &mut self,
        value: AstValue<'a>,
        expected: ValueType,
        namespace: ReferenceNamespace,
        id: HirId,
    ) -> HirValue {
        let range = value.range();
        let mut poison = Vec::new();
        let kind = if let AstValue::Reference(reference) = value {
            HirValueKind::Reference(ReferenceValue {
                name: reference.name().unwrap_or("").to_owned(),
                namespace,
                range: reference.name_range().unwrap_or(range),
            })
        } else {
            self.context(
                range,
                format!("`{}` requires a bare typed reference", expected.as_str()),
                "remove quotes and provide one reference name",
            );
            poison.push(Poison {
                kind: PoisonKind::Value,
                range,
            });
            HirValueKind::Missing
        };
        HirValue {
            id,
            value_type: expected,
            kind,
            range,
            poison,
        }
    }

    fn lower_reference_list(
        &mut self,
        value: AstValue<'a>,
        expected: ValueType,
        namespace: ReferenceNamespace,
        id: HirId,
    ) -> HirValue {
        let range = value.range();
        let mut poison = Vec::new();
        let kind = if let AstValue::List(list) = value {
            let mut values = Vec::new();
            let mut seen = BTreeMap::<String, Vec<ByteRange>>::new();
            for item in list.values() {
                let item_id = self.alloc(Some(item.node_id()), item.range());
                let mut lowered =
                    self.lower_reference_value(item, ValueType::GroupReference, namespace, item_id);
                if let HirValueKind::Reference(reference) = &lowered.kind {
                    let previous =
                        Self::record_origin(&mut seen, reference.name.clone(), reference.range);
                    if !previous.is_empty() {
                        self.duplicate(
                            reference.range,
                            &previous,
                            format!("duplicate `{}` reference", reference.name),
                        );
                        lowered.poison.push(Poison {
                            kind: PoisonKind::Duplicate,
                            range: reference.range,
                        });
                    }
                }
                values.push(lowered);
            }
            HirValueKind::List(values)
        } else {
            self.context(
                range,
                format!("`{}` requires a list of bare references", expected.as_str()),
                "use a bracketed comma-separated reference list",
            );
            poison.push(Poison {
                kind: PoisonKind::Value,
                range,
            });
            HirValueKind::Missing
        };
        HirValue {
            id,
            value_type: expected,
            kind,
            range,
            poison,
        }
    }

    fn lower_normalized_string_list(
        &mut self,
        value: AstValue<'a>,
        expected: ValueType,
        scope: ScopeId,
        id: HirId,
    ) -> HirValue {
        if matches!(value, AstValue::List(_)) {
            return self.lower_string_list(value, expected, scope, id, false);
        }

        let range = value.range();
        let item_id = self.alloc(Some(value.node_id()), range);
        let item = self.lower_string_value(value, ValueType::String, scope, item_id, true);
        HirValue {
            id,
            value_type: expected,
            kind: HirValueKind::List(vec![item]),
            range,
            poison: Vec::new(),
        }
    }

    fn lower_string_list(
        &mut self,
        value: AstValue<'a>,
        expected: ValueType,
        scope: ScopeId,
        id: HirId,
        nonempty: bool,
    ) -> HirValue {
        let range = value.range();
        let mut poison = Vec::new();
        let kind = if let AstValue::List(list) = value {
            let items = list.values();
            if nonempty && items.is_empty() {
                self.context(
                    range,
                    format!("`{}` requires a non-empty list", expected.as_str()),
                    "add at least one quoted string",
                );
                poison.push(Poison {
                    kind: PoisonKind::Value,
                    range,
                });
            }
            let mut values = Vec::new();
            let mut seen = BTreeMap::<String, Vec<ByteRange>>::new();
            for item in items {
                let item_id = self.alloc(Some(item.node_id()), item.range());
                let mut lowered =
                    self.lower_string_value(item, ValueType::String, scope, item_id, true);
                if let Some(text) = value_text(&lowered).map(str::to_owned) {
                    if matches!(expected, ValueType::ArchitectureList)
                        && !matches!(text.as_str(), "x86_64" | "aarch64")
                    {
                        self.context(
                            lowered.range,
                            "architecture is not registered in source version 1",
                            "use `x86_64` or `aarch64`",
                        );
                        lowered.poison.push(Poison {
                            kind: PoisonKind::Value,
                            range: lowered.range,
                        });
                    }
                    if expected == ValueType::Hostnames && !validate::is_one_line(&text) {
                        self.context(
                            lowered.range,
                            "hostname alias must decode to one line",
                            "remove the line break from the quoted alias",
                        );
                        lowered.poison.push(Poison {
                            kind: PoisonKind::Value,
                            range: lowered.range,
                        });
                    }
                    if expected == ValueType::ArchitectureList {
                        let previous = Self::record_origin(&mut seen, text.clone(), lowered.range);
                        if !previous.is_empty() {
                            self.duplicate(
                                lowered.range,
                                &previous,
                                format!("duplicate list value `{text}`"),
                            );
                            lowered.poison.push(Poison {
                                kind: PoisonKind::Duplicate,
                                range: lowered.range,
                            });
                        }
                    }
                }
                values.push(lowered);
            }
            HirValueKind::List(values)
        } else {
            self.context(
                range,
                format!("`{}` requires a list of quoted strings", expected.as_str()),
                "use a bracketed comma-separated string list",
            );
            poison.push(Poison {
                kind: PoisonKind::Value,
                range,
            });
            HirValueKind::Missing
        };
        HirValue {
            id,
            value_type: expected,
            kind,
            range,
            poison,
        }
    }

    fn validate_value(&mut self, value: &mut HirValue) {
        let Some(text) = value_text(value).map(str::to_owned) else {
            return;
        };
        if value.value_type == ValueType::DotfileVersion && text != "1" {
            self.diagnostics.push(
                Diagnostic::new(
                    "schema/context",
                    Stage::Schema,
                    Severity::Error,
                    "the authored source version is unsupported",
                    "declare @dotfile-version = \"1\"",
                    self.span(value.range),
                )
                .with_detail("unsupported_dotfile_version")
                .with_expected("version", "1")
                .with_actual("version", text),
            );
            value.poison.push(Poison {
                kind: PoisonKind::Value,
                range: value.range,
            });
            return;
        }
        let valid = match value.value_type {
            ValueType::DotfileVersion => true,
            ValueType::OneLineString => validate::is_one_line(&text),
            ValueType::CommandName => validate::is_command_name(&text),
            ValueType::Check => validate::is_enum_value(
                &text,
                &["command", "package", "font", "service", "path", "none"],
            ),
            ValueType::MachinePath => validate::is_machine_path(&text),
            ValueType::DestinationPath => validate::is_destination_path(&text),
            ValueType::RepositoryDirectory => validate::is_repository_directory(&text),
            ValueType::Deploy => validate::is_enum_value(&text, &["link", "copy", "none"]),
            ValueType::Privilege => validate::is_enum_value(&text, &["user", "system"]),
            ValueType::Sensitivity => validate::is_enum_value(&text, &["public", "private"]),
            ValueType::Mode => validate::is_mode(&text),
            ValueType::Expect => {
                validate::is_enum_value(&text, &["any", "file", "directory", "symlink"])
            }
            ValueType::ServiceScope => validate::is_enum_value(&text, &["user", "system"]),
            ValueType::Os => validate::is_enum_value(&text, &["darwin", "linux"]),
            ValueType::Manager => validate::is_manager(&text),
            ValueType::Installer => validate::is_installer(&text),
            ValueType::HostRole => validate::is_enum_value(&text, &["desktop", "laptop", "server"]),
            ValueType::Recipient => validate::is_age_public_recipient(&text),
            ValueType::ScanPattern => validate::is_scan_glob(&text),
            ValueType::Inspect => validate::is_enum_value(&text, &["path", "value"]),
            ValueType::String
            | ValueType::StringExpression
            | ValueType::StringOrStringList
            | ValueType::ResourceKeyReference
            | ValueType::ArchitectureList
            | ValueType::GroupReferenceList
            | ValueType::GroupReference
            | ValueType::ThemeReference
            | ValueType::ProfileReference
            | ValueType::Hostnames
            | ValueType::HostExtension
            | ValueType::BenchmarkRunId => true,
        };
        if !valid {
            self.context(
                value.range,
                format!("value is invalid for `{}`", value.value_type.as_str()),
                "use a value accepted by this field's source-version-1 schema",
            );
            value.poison.push(Poison {
                kind: PoisonKind::Value,
                range: value.range,
            });
        }
    }

    fn lower_attribute(
        &mut self,
        attribute: AstAttribute<'a>,
        expected: ValueType,
        scope: ScopeId,
    ) -> Attribute {
        let id = self.alloc(Some(attribute.node_id()), attribute.range());
        let name = attribute.name().unwrap_or("");
        let kind =
            AttributeKind::from_name(name).expect("known attribute passed to lower_attribute");
        let value = self.lower_value(
            attribute.value(),
            expected,
            scope,
            attribute.node_id(),
            attribute.value_range().unwrap_or(attribute.range()),
        );
        let poison = value.poison.clone();
        Attribute {
            id,
            kind,
            value,
            range: attribute.range(),
            poison,
        }
    }

    fn duplicate_attributes(&mut self, attributes: &mut [Attribute]) {
        let mut seen = BTreeMap::<AttributeKind, Vec<ByteRange>>::new();
        for attribute in attributes {
            let previous = Self::record_origin(&mut seen, attribute.kind, attribute.range);
            if !previous.is_empty() {
                self.duplicate(
                    attribute.range,
                    &previous,
                    format!("duplicate attribute `{}`", attribute.kind.as_str()),
                );
                attribute.poison.push(Poison {
                    kind: PoisonKind::Duplicate,
                    range: attribute.range,
                });
            }
        }
    }

    // Domain lowerers are below.

    fn lower_profiles(&mut self, ast: AstFile<'a>) -> Profiles {
        let entries = ast.entries();
        let root_scope = self.prepare_scope(ast.node_id(), ast.range(), &entries, None, false);
        let id = self.alloc(Some(ast.node_id()), ast.range());
        let mut version = None;
        let mut theme = None;
        let mut version_origins = Vec::new();
        let mut theme_origins = Vec::new();
        let mut groups = Vec::new();
        let mut profiles = Vec::new();
        let mut groups_shapes = Vec::new();
        let mut profiles_shapes = Vec::new();
        let mut group_names = BTreeMap::new();
        let mut profile_names = BTreeMap::new();
        let mut poison = Vec::new();

        if !matches!(entries.first(), Some(Entry::Attribute(attribute)) if attribute.name() == Some("dotfile-version"))
        {
            self.context(
                entries.first().map_or(ast.range(), Entry::range),
                "`@dotfile-version` must be the first non-comment profiles entry",
                "place the exact `@dotfile-version = \"1\"` declaration first",
            );
            poison.push(Poison {
                kind: PoisonKind::Context,
                range: ast.range(),
            });
        }

        for entry in entries {
            match entry {
                Entry::Attribute(attribute) if attribute.name() == Some("dotfile-version") => {
                    let mut lowered =
                        self.lower_attribute(attribute, ValueType::DotfileVersion, root_scope);
                    if value_text(&lowered.value) == Some("1")
                        && !self.has_exact_source_version_syntax(attribute)
                    {
                        self.context(
                            attribute.range(),
                            "`@dotfile-version` must use the exact source-version preamble shape",
                            "write `@dotfile-version = \"1\"` on one physical line without escapes or interpolation",
                        );
                        lowered.poison.push(Poison {
                            kind: PoisonKind::Value,
                            range: attribute.range(),
                        });
                    }
                    let previous = Self::record_singleton(&mut version_origins, lowered.range);
                    if !previous.is_empty() {
                        self.duplicate(
                            lowered.range,
                            &previous,
                            "duplicate `@dotfile-version` declaration",
                        );
                        lowered.poison.push(Poison {
                            kind: PoisonKind::Duplicate,
                            range: lowered.range,
                        });
                        self.retain_attribute(lowered);
                    } else {
                        version = Some(lowered);
                    }
                }
                Entry::Attribute(attribute) if attribute.name() == Some("theme") => {
                    let mut lowered =
                        self.lower_attribute(attribute, ValueType::ThemeReference, root_scope);
                    let previous = Self::record_singleton(&mut theme_origins, lowered.range);
                    if !previous.is_empty() {
                        self.duplicate(lowered.range, &previous, "duplicate repository `@theme`");
                        lowered.poison.push(Poison {
                            kind: PoisonKind::Duplicate,
                            range: lowered.range,
                        });
                        self.retain_attribute(lowered);
                    } else {
                        theme = Some(lowered);
                    }
                }
                Entry::SigilBlock(block) if block.name() == Some("groups") && !block.optional() => {
                    let previous = Self::record_singleton(&mut groups_shapes, block.range());
                    if !previous.is_empty() {
                        self.duplicate(block.range(), &previous, "duplicate `@groups` block");
                        poison.push(self.retain_poison(
                            block.node_id(),
                            block.range(),
                            PoisonKind::Duplicate,
                        ));
                    } else {
                        if let Some(body) = block.block() {
                            let body_entries = body.entries();
                            let body_scope = self.prepare_scope(
                                body.node_id(),
                                body.range(),
                                &body_entries,
                                Some(root_scope),
                                false,
                            );
                            groups =
                                self.lower_groups(body, body_scope, None, &mut group_names, false);
                        }
                    }
                }
                Entry::SigilBlock(block)
                    if block.name() == Some("profiles") && !block.optional() =>
                {
                    let previous = Self::record_singleton(&mut profiles_shapes, block.range());
                    if !previous.is_empty() {
                        self.duplicate(block.range(), &previous, "duplicate `@profiles` block");
                        poison.push(self.retain_poison(
                            block.node_id(),
                            block.range(),
                            PoisonKind::Duplicate,
                        ));
                    } else {
                        if let Some(body) = block.block() {
                            let body_entries = body.entries();
                            let body_scope = self.prepare_scope(
                                body.node_id(),
                                body.range(),
                                &body_entries,
                                Some(root_scope),
                                false,
                            );
                            profiles = self.lower_profile_declarations(
                                body,
                                body_scope,
                                &mut profile_names,
                            );
                        }
                    }
                }
                Entry::Let(_) => {}
                Entry::Error(error) => {
                    poison.push(self.retain_poison(
                        error.node_id(),
                        error.range(),
                        PoisonKind::Syntax,
                    ));
                }
                other => {
                    self.context(
                        other.range(),
                        "entry is not legal at the profiles file root",
                        "use only the registered profiles root shapes",
                    );
                    poison.push(self.retain_poison(
                        other.node_id(),
                        other.range(),
                        PoisonKind::Context,
                    ));
                }
            }
        }

        if version.is_none() {
            self.context(
                ast.range(),
                "profiles file is missing `@dotfile-version`",
                "add the exact source-version declaration first",
            );
            poison.push(Poison {
                kind: PoisonKind::Missing,
                range: ast.range(),
            });
        }
        if groups_shapes.is_empty() {
            self.context(
                ast.range(),
                "profiles file is missing its `@groups` block",
                "add exactly one `@groups` block",
            );
            poison.push(Poison {
                kind: PoisonKind::Missing,
                range: ast.range(),
            });
        }
        if profiles_shapes.is_empty() {
            self.context(
                ast.range(),
                "profiles file is missing its `@profiles` block",
                "add exactly one `@profiles` block",
            );
            poison.push(Poison {
                kind: PoisonKind::Missing,
                range: ast.range(),
            });
        }

        match find_group(&groups, "shared") {
            None => {
                let range = groups_shapes.first().copied().unwrap_or(ast.range());
                self.context(
                    range,
                    "the required root group `shared` is not declared",
                    "declare `shared` directly inside `@groups` with `@directory`",
                );
                poison.push(Poison {
                    kind: PoisonKind::Missing,
                    range,
                });
            }
            Some(shared) => {
                if shared.parent.is_some() {
                    self.context(
                        shared.name_range,
                        "the `shared` group must be a declared root group",
                        "move `shared` directly inside `@groups`",
                    );
                    poison.push(Poison {
                        kind: PoisonKind::Context,
                        range: shared.name_range,
                    });
                }
                if shared.attribute(AttributeKind::Directory).is_none() {
                    self.context(
                        shared.name_range,
                        "the `shared` group requires `@directory`",
                        "add a normalized repository-relative directory",
                    );
                    poison.push(Poison {
                        kind: PoisonKind::Missing,
                        range: shared.name_range,
                    });
                }
            }
        }

        if version
            .as_ref()
            .is_some_and(|attribute| !attribute.poison.is_empty())
            || theme
                .as_ref()
                .is_some_and(|attribute| !attribute.poison.is_empty())
        {
            poison.push(Poison {
                kind: PoisonKind::Value,
                range: ast.range(),
            });
        }

        Profiles {
            id,
            version,
            groups,
            profiles,
            theme,
            poison,
        }
    }

    fn has_exact_source_version_syntax(&self, attribute: AstAttribute<'a>) -> bool {
        let Some(value_range) = attribute.value_range() else {
            return false;
        };
        if self.source.slice(value_range) != b"\"1\""
            || self
                .source
                .slice(attribute.range())
                .iter()
                .any(|byte| matches!(byte, b'\n' | b'\r'))
        {
            return false;
        }

        let tail = &self.source.as_bytes()[attribute.range().end() as usize..];
        let physical_line_end = tail
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(tail.len());
        let mut suffix = &tail[..physical_line_end];
        if suffix.last() == Some(&b'\r') {
            suffix = &suffix[..suffix.len() - 1];
        }
        let first_non_space = suffix
            .iter()
            .position(|byte| !matches!(byte, b' ' | b'\t'))
            .unwrap_or(suffix.len());
        first_non_space == suffix.len() || suffix[first_non_space] == b'#'
    }

    fn lower_groups(
        &mut self,
        block: Block<'a>,
        parent_scope: ScopeId,
        parent_group: Option<HirId>,
        names: &mut BTreeMap<String, Vec<ByteRange>>,
        nested_body: bool,
    ) -> Vec<GroupDeclaration> {
        let mut groups = Vec::new();
        for entry in block.entries() {
            match entry {
                Entry::Named(named)
                    if !named.optional() && named.value().is_none() && named.block().is_some() =>
                {
                    let name = named.name().unwrap_or("").to_owned();
                    let name_range = named.name_range().unwrap_or(named.range());
                    let id = self.alloc(Some(named.node_id()), named.range());
                    let mut poison = Vec::new();
                    let previous = Self::record_origin(names, name.clone(), name_range);
                    if !previous.is_empty() {
                        self.duplicate(name_range, &previous, format!("duplicate group `{name}`"));
                        poison.push(Poison {
                            kind: PoisonKind::Duplicate,
                            range: name_range,
                        });
                    }
                    let body = named.block().expect("guarded above");
                    let body_entries = body.entries();
                    let scope = self.prepare_scope(
                        body.node_id(),
                        body.range(),
                        &body_entries,
                        Some(parent_scope),
                        false,
                    );
                    let mut attributes = Vec::new();
                    for child in &body_entries {
                        if let Entry::Attribute(attribute) = child {
                            let expected = group_attribute_type(attribute.name().unwrap_or(""));
                            if let Some(expected) = expected {
                                attributes.push(self.lower_attribute(*attribute, expected, scope));
                            } else {
                                self.context(
                                    attribute.range(),
                                    "attribute is not legal in a group declaration",
                                    "use only `@directory`, `@os`, `@arch`, or `@description`",
                                );
                                poison.push(Poison {
                                    kind: PoisonKind::Context,
                                    range: attribute.range(),
                                });
                                self.retain_poison(
                                    attribute.node_id(),
                                    attribute.range(),
                                    PoisonKind::Context,
                                );
                            }
                        }
                    }
                    self.duplicate_attributes(&mut attributes);
                    let children = self.lower_groups(body, scope, Some(id), names, true);
                    for child in body_entries {
                        if !matches!(
                            child,
                            Entry::Attribute(_) | Entry::Named(_) | Entry::Let(_) | Entry::Error(_)
                        ) {
                            self.context(
                                child.range(),
                                "entry is not legal in a group declaration",
                                "use attributes or nested named group blocks",
                            );
                            poison.push(Poison {
                                kind: PoisonKind::Context,
                                range: child.range(),
                            });
                        }
                    }
                    groups.push(GroupDeclaration {
                        id,
                        name,
                        name_range,
                        parent: parent_group,
                        attributes,
                        children,
                        poison,
                    });
                }
                Entry::Attribute(_) if nested_body => {}
                Entry::Let(_) => {}
                Entry::Error(error) => {
                    self.retain_poison(error.node_id(), error.range(), PoisonKind::Syntax);
                }
                Entry::Named(named) => {
                    self.context(
                        named.range(),
                        "group declaration must be a non-optional named block",
                        "use `name { ... }`",
                    );
                    self.retain_poison(named.node_id(), named.range(), PoisonKind::Context);
                }
                other => {
                    self.context(
                        other.range(),
                        "only named group blocks are legal in `@groups`",
                        "replace the entry with a named group block",
                    );
                    self.retain_poison(other.node_id(), other.range(), PoisonKind::Context);
                }
            }
        }
        groups
    }

    fn lower_profile_declarations(
        &mut self,
        block: Block<'a>,
        parent_scope: ScopeId,
        names: &mut BTreeMap<String, Vec<ByteRange>>,
    ) -> Vec<ProfileDeclaration> {
        let mut profiles = Vec::new();
        for entry in block.entries() {
            match entry {
                Entry::Named(named)
                    if !named.optional() && named.value().is_none() && named.block().is_some() =>
                {
                    let name = named.name().unwrap_or("").to_owned();
                    let name_range = named.name_range().unwrap_or(named.range());
                    let id = self.alloc(Some(named.node_id()), named.range());
                    let mut poison = Vec::new();
                    let previous = Self::record_origin(names, name.clone(), name_range);
                    if !previous.is_empty() {
                        self.duplicate(
                            name_range,
                            &previous,
                            format!("duplicate profile `{name}`"),
                        );
                        poison.push(Poison {
                            kind: PoisonKind::Duplicate,
                            range: name_range,
                        });
                    }
                    let body = named.block().expect("guarded above");
                    let body_entries = body.entries();
                    let scope = self.prepare_scope(
                        body.node_id(),
                        body.range(),
                        &body_entries,
                        Some(parent_scope),
                        false,
                    );
                    let mut attributes = Vec::new();
                    for child in body_entries {
                        match child {
                            Entry::Attribute(attribute) => {
                                let expected =
                                    profile_attribute_type(attribute.name().unwrap_or(""));
                                if let Some(expected) = expected {
                                    attributes
                                        .push(self.lower_attribute(attribute, expected, scope));
                                } else {
                                    self.context(
                                        attribute.range(),
                                        "attribute is not legal in a profile declaration",
                                        "use only registered profile attributes",
                                    );
                                    poison.push(Poison {
                                        kind: PoisonKind::Context,
                                        range: attribute.range(),
                                    });
                                    self.retain_poison(
                                        attribute.node_id(),
                                        attribute.range(),
                                        PoisonKind::Context,
                                    );
                                }
                            }
                            Entry::Let(_) => {}
                            Entry::Error(error) => {
                                self.retain_poison(
                                    error.node_id(),
                                    error.range(),
                                    PoisonKind::Syntax,
                                );
                            }
                            other => {
                                self.context(
                                    other.range(),
                                    "only profile attributes are legal in a profile declaration",
                                    "remove the nested entry",
                                );
                                poison.push(Poison {
                                    kind: PoisonKind::Context,
                                    range: other.range(),
                                });
                                self.retain_poison(
                                    other.node_id(),
                                    other.range(),
                                    PoisonKind::Context,
                                );
                            }
                        }
                    }
                    self.duplicate_attributes(&mut attributes);
                    for required in [
                        AttributeKind::Groups,
                        AttributeKind::Manager,
                        AttributeKind::Os,
                    ] {
                        if !attributes
                            .iter()
                            .any(|attribute| attribute.kind == required)
                        {
                            self.context(
                                name_range,
                                format!("profile `{name}` is missing `{}`", required.as_str()),
                                "add the required profile attribute",
                            );
                            poison.push(Poison {
                                kind: PoisonKind::Missing,
                                range: name_range,
                            });
                        }
                    }
                    profiles.push(ProfileDeclaration {
                        id,
                        name,
                        name_range,
                        attributes,
                        poison,
                    });
                }
                Entry::Let(_) => {}
                Entry::Error(error) => {
                    self.retain_poison(error.node_id(), error.range(), PoisonKind::Syntax);
                }
                other => {
                    self.context(
                        other.range(),
                        "only named profile blocks are legal in `@profiles`",
                        "replace the entry with a named profile block",
                    );
                    self.retain_poison(other.node_id(), other.range(), PoisonKind::Context);
                }
            }
        }
        profiles
    }

    fn lower_hosts(&mut self, ast: AstFile<'a>) -> Hosts {
        let entries = ast.entries();
        let root_scope = self.prepare_scope(ast.node_id(), ast.range(), &entries, None, false);
        let id = self.alloc(Some(ast.node_id()), ast.range());
        let mut hosts = Vec::new();
        let mut names = BTreeMap::new();
        let mut poison = Vec::new();
        for entry in entries {
            let Entry::Named(named) = entry else {
                match entry {
                    Entry::Let(_) => {}
                    Entry::Error(error) => poison.push(self.retain_poison(
                        error.node_id(),
                        error.range(),
                        PoisonKind::Syntax,
                    )),
                    other => {
                        self.context(
                            other.range(),
                            "hosts file root accepts only named host blocks",
                            "use `host-name { ... }`",
                        );
                        poison.push(self.retain_poison(
                            other.node_id(),
                            other.range(),
                            PoisonKind::Context,
                        ));
                    }
                }
                continue;
            };
            let name = named.name().unwrap_or("").to_owned();
            let name_range = named.name_range().unwrap_or(named.range());
            let host_id = self.alloc(Some(named.node_id()), named.range());
            let mut host_poison = Vec::new();
            if named.optional() || named.value().is_some() || named.block().is_none() {
                self.context(
                    named.range(),
                    "host declaration must be a non-optional named block",
                    "use `host-name { ... }`",
                );
                host_poison.push(Poison {
                    kind: PoisonKind::Context,
                    range: named.range(),
                });
            }
            let previous = Self::record_origin(&mut names, name.clone(), name_range);
            if !previous.is_empty() {
                self.duplicate(name_range, &previous, format!("duplicate host `{name}`"));
                host_poison.push(Poison {
                    kind: PoisonKind::Duplicate,
                    range: name_range,
                });
            }
            let mut host = HostDeclaration {
                id: host_id,
                name,
                name_range,
                hostnames: None,
                role: None,
                profile: None,
                theme: None,
                extensions: Vec::new(),
                poison: host_poison,
            };
            if let Some(body) = named.block() {
                let body_entries = body.entries();
                let scope = self.prepare_scope(
                    body.node_id(),
                    body.range(),
                    &body_entries,
                    Some(root_scope),
                    false,
                );
                let mut fields = BTreeMap::<String, Vec<ByteRange>>::new();
                let mut attribute_origins = BTreeMap::<AttributeKind, Vec<ByteRange>>::new();
                for child in body_entries {
                    match child {
                        Entry::Named(field)
                            if !field.optional()
                                && field.value().is_some()
                                && field.block().is_none() =>
                        {
                            let field_name = field.name().unwrap_or("").to_owned();
                            let field_range = field.name_range().unwrap_or(field.range());
                            let expected = match field_name.as_str() {
                                "hostnames" => Some(ValueType::Hostnames),
                                "role" => Some(ValueType::HostRole),
                                name if validate::is_extension_key(name) => {
                                    Some(ValueType::HostExtension)
                                }
                                _ => None,
                            };
                            if let Some(expected) = expected {
                                let field_id = self.alloc(Some(field.node_id()), field.range());
                                let value = self.lower_value(
                                    field.value(),
                                    expected,
                                    scope,
                                    field.node_id(),
                                    field.value_range().unwrap_or(field.range()),
                                );
                                let mut lowered = NamedField {
                                    id: field_id,
                                    name: field_name.clone(),
                                    name_range: field_range,
                                    range: field.range(),
                                    poison: value.poison.clone(),
                                    value,
                                };
                                let previous = Self::record_origin(
                                    &mut fields,
                                    field_name.clone(),
                                    field_range,
                                );
                                if !previous.is_empty() {
                                    self.duplicate(
                                        field_range,
                                        &previous,
                                        format!("duplicate host field `{field_name}`"),
                                    );
                                    lowered.poison.push(Poison {
                                        kind: PoisonKind::Duplicate,
                                        range: field_range,
                                    });
                                }
                                match field_name.as_str() {
                                    "hostnames" if host.hostnames.is_none() => {
                                        host.hostnames = Some(lowered)
                                    }
                                    "role" if host.role.is_none() => host.role = Some(lowered),
                                    "hostnames" | "role" => self.retain_named_field(lowered),
                                    _ => host.extensions.push(lowered),
                                }
                            } else {
                                self.context(
                                    field.range(),
                                    "unknown standard host field or invalid extension fact key",
                                    "use `hostnames`, `role`, or an uppercase extension key",
                                );
                                host.poison.push(Poison {
                                    kind: PoisonKind::Context,
                                    range: field.range(),
                                });
                                self.retain_poison(
                                    field.node_id(),
                                    field.range(),
                                    PoisonKind::Context,
                                );
                            }
                        }
                        Entry::Attribute(attribute)
                            if matches!(attribute.name(), Some("profile" | "theme")) =>
                        {
                            let expected = if attribute.name() == Some("profile") {
                                ValueType::ProfileReference
                            } else {
                                ValueType::ThemeReference
                            };
                            let mut lowered = self.lower_attribute(attribute, expected, scope);
                            let slot = if expected == ValueType::ProfileReference {
                                &mut host.profile
                            } else {
                                &mut host.theme
                            };
                            let previous = Self::record_origin(
                                &mut attribute_origins,
                                lowered.kind,
                                lowered.range,
                            );
                            if !previous.is_empty() {
                                self.duplicate(
                                    lowered.range,
                                    &previous,
                                    format!("duplicate host `{}`", lowered.kind.as_str()),
                                );
                                lowered.poison.push(Poison {
                                    kind: PoisonKind::Duplicate,
                                    range: lowered.range,
                                });
                                self.retain_attribute(lowered);
                            } else {
                                *slot = Some(lowered);
                            }
                        }
                        Entry::Let(_) => {}
                        Entry::Error(error) => {
                            self.retain_poison(error.node_id(), error.range(), PoisonKind::Syntax);
                        }
                        other => {
                            self.context(
                                other.range(),
                                "entry is not legal in a host block",
                                "use registered host fields or uppercase extension facts",
                            );
                            host.poison.push(Poison {
                                kind: PoisonKind::Context,
                                range: other.range(),
                            });
                            self.retain_poison(other.node_id(), other.range(), PoisonKind::Context);
                        }
                    }
                }
            }
            for (present, field) in [
                (host.hostnames.is_some(), "hostnames"),
                (host.role.is_some(), "role"),
                (host.profile.is_some(), "@profile"),
            ] {
                if !present {
                    self.context(
                        host.name_range,
                        format!("host `{}` is missing required field `{field}`", host.name),
                        "add the required host field",
                    );
                    host.poison.push(Poison {
                        kind: PoisonKind::Missing,
                        range: host.name_range,
                    });
                }
            }
            hosts.push(host);
        }

        let mut aliases = BTreeMap::<String, Vec<(usize, ByteRange)>>::new();
        for (host_index, host) in hosts.iter_mut().enumerate() {
            let implicit = normalized_hostname(&host.name);
            let implicit_origins = aliases.entry(implicit.clone()).or_default();
            let previous = implicit_origins
                .iter()
                .filter_map(|(owner, range)| (*owner != host_index).then_some(*range))
                .collect::<Vec<_>>();
            implicit_origins.push((host_index, host.name_range));
            if !previous.is_empty() {
                self.duplicate(
                    host.name_range,
                    &previous,
                    format!("duplicate hostname alias `{implicit}`"),
                );
                host.poison.push(Poison {
                    kind: PoisonKind::Duplicate,
                    range: host.name_range,
                });
            }
            if let Some(field) = &mut host.hostnames
                && let HirValueKind::List(values) = &mut field.value.kind
            {
                let mut local = BTreeMap::<String, Vec<ByteRange>>::new();
                for value in values {
                    if let Some(alias) = value_text(value).map(str::to_owned) {
                        let normalized = normalized_hostname(&alias);
                        let previous =
                            Self::record_origin(&mut local, normalized.clone(), value.range);
                        if !previous.is_empty() {
                            self.duplicate(
                                value.range,
                                &previous,
                                format!("duplicate hostname alias `{normalized}`"),
                            );
                            value.poison.push(Poison {
                                kind: PoisonKind::Duplicate,
                                range: value.range,
                            });
                            continue;
                        }
                        if normalized == implicit {
                            continue;
                        }
                        let origins = aliases.entry(normalized.clone()).or_default();
                        let previous = origins
                            .iter()
                            .filter_map(|(owner, range)| (*owner != host_index).then_some(*range))
                            .collect::<Vec<_>>();
                        origins.push((host_index, value.range));
                        if !previous.is_empty() {
                            self.duplicate(
                                value.range,
                                &previous,
                                format!("duplicate hostname alias `{normalized}`"),
                            );
                            value.poison.push(Poison {
                                kind: PoisonKind::Duplicate,
                                range: value.range,
                            });
                        }
                    }
                }
            }
        }
        Hosts { id, hosts, poison }
    }

    fn lower_requirements(&mut self, ast: AstFile<'a>, domain: Domain) -> Requirements {
        let entries = ast.entries();
        let scope = self.prepare_scope(ast.node_id(), ast.range(), &entries, None, true);
        let id = self.alloc(Some(ast.node_id()), ast.range());
        let context = match domain {
            Domain::GroupRootRequirements => RequirementContext::GroupRoot,
            Domain::FacetRequirements => RequirementContext::Facet,
            Domain::OverrideVariant => RequirementContext::Variant,
            _ => unreachable!("requirement lowerer called for another domain"),
        };
        let mut path_names = BTreeMap::new();
        let lowered = self.lower_requirement_entries(&entries, scope, context, &mut path_names);
        let poison = lowered
            .iter()
            .filter_map(|entry| match entry {
                RequirementEntry::Poison(node) => Some(Poison {
                    kind: PoisonKind::Context,
                    range: node.range,
                }),
                _ => None,
            })
            .collect();
        Requirements {
            id,
            context,
            scope,
            entries: lowered,
            poison,
        }
    }

    fn lower_requirement_entries(
        &mut self,
        entries: &[Entry<'a>],
        scope: ScopeId,
        context: RequirementContext,
        path_names: &mut BTreeMap<String, Vec<ByteRange>>,
    ) -> Vec<RequirementEntry> {
        let mut lowered = Vec::new();
        let mut attributes = Vec::<Attribute>::new();
        for entry in entries {
            match *entry {
                Entry::Let(declaration) => {
                    if let Some(binding) = self.binding_entries.get(&declaration.node_id()) {
                        lowered.push(RequirementEntry::Binding(*binding));
                    }
                }
                Entry::Attribute(attribute) => {
                    if let Some(expected) =
                        requirement_attribute_type(context, attribute.name().unwrap_or(""))
                    {
                        let value = self.lower_attribute(attribute, expected, scope);
                        attributes.push(value.clone());
                        lowered.push(RequirementEntry::Attribute(value));
                    } else {
                        lowered.push(self.invalid_requirement_entry(
                            entry,
                            "attribute is not legal in this requirement context",
                        ));
                    }
                }
                Entry::Named(named) => {
                    if matches!(
                        context,
                        RequirementContext::GroupRoot
                            | RequirementContext::Facet
                            | RequirementContext::EntityFact
                    ) {
                        lowered.push(RequirementEntry::Entity(
                            self.lower_entity(named, scope, path_names),
                        ));
                    } else {
                        lowered.push(self.invalid_requirement_entry(
                            entry,
                            "entity demands are not legal in this requirement context",
                        ));
                    }
                }
                Entry::SigilBlock(block) if block.name() == Some("font") => {
                    if matches!(
                        context,
                        RequirementContext::GroupRoot
                            | RequirementContext::Facet
                            | RequirementContext::EntityFact
                    ) {
                        lowered.push(RequirementEntry::Resource(
                            self.lower_resource(block, scope, path_names),
                        ));
                    } else {
                        lowered.push(self.invalid_requirement_entry(
                            entry,
                            "resource demands are not legal in this requirement context",
                        ));
                    }
                }
                Entry::SigilBlock(_) => lowered.push(self.invalid_requirement_entry(
                    entry,
                    "unknown or structurally misplaced sigil block",
                )),
                Entry::Extend(extension) => {
                    if matches!(
                        context,
                        RequirementContext::GroupRoot | RequirementContext::Facet
                    ) {
                        lowered.push(RequirementEntry::Extension(
                            self.lower_extension(extension, scope, path_names),
                        ));
                    } else {
                        lowered.push(self.invalid_requirement_entry(
                            entry,
                            "fact extensions are not legal in this requirement context",
                        ));
                    }
                }
                Entry::Path(path) => {
                    if matches!(
                        context,
                        RequirementContext::Facet | RequirementContext::Variant
                    ) {
                        let node = self.lower_path_node(path, scope, path_names);
                        lowered.push(RequirementEntry::Path(node));
                    } else {
                        lowered.push(self.invalid_requirement_entry(
                            entry,
                            "path nodes are legal only at a facet or variant root",
                        ));
                    }
                }
                Entry::Error(error) => {
                    let id = self.alloc(Some(error.node_id()), error.range());
                    lowered.push(RequirementEntry::Poison(PoisonNode {
                        id,
                        range: error.range(),
                        poison: vec![Poison {
                            kind: PoisonKind::Syntax,
                            range: error.range(),
                        }],
                    }));
                }
            }
        }
        let mut seen = BTreeMap::<AttributeKind, Vec<ByteRange>>::new();
        for entry in &mut lowered {
            if let RequirementEntry::Attribute(attribute) = entry {
                let previous = Self::record_origin(&mut seen, attribute.kind, attribute.range);
                if !previous.is_empty() {
                    self.duplicate(
                        attribute.range,
                        &previous,
                        format!("duplicate attribute `{}`", attribute.kind.as_str()),
                    );
                    attribute.poison.push(Poison {
                        kind: PoisonKind::Duplicate,
                        range: attribute.range,
                    });
                }
            }
        }
        lowered
    }

    fn invalid_requirement_entry(
        &mut self,
        entry: &Entry<'a>,
        summary: &'static str,
    ) -> RequirementEntry {
        self.context(
            entry.range(),
            summary,
            "move or remove the entry so it matches the containing schema",
        );
        let id = self.alloc(Some(entry.node_id()), entry.range());
        RequirementEntry::Poison(PoisonNode {
            id,
            range: entry.range(),
            poison: vec![Poison {
                kind: PoisonKind::Context,
                range: entry.range(),
            }],
        })
    }

    fn lower_entity(
        &mut self,
        named: dotfile_syntax::NamedEntry<'a>,
        parent_scope: ScopeId,
        path_names: &mut BTreeMap<String, Vec<ByteRange>>,
    ) -> EntityDemand {
        let id = self.alloc(Some(named.node_id()), named.range());
        let name = named.name().unwrap_or("").to_owned();
        let name_range = named.name_range().unwrap_or(named.range());
        let mut poison = Vec::new();
        let mut entries = Vec::new();
        let mut scope = None;
        let assignment_sugar = named.value().is_some();
        if assignment_sugar && named.block().is_some() {
            self.context(
                named.range(),
                "entity demand cannot have both an assignment and a block",
                "choose assignment sugar or a fact block",
            );
            poison.push(Poison {
                kind: PoisonKind::Context,
                range: named.range(),
            });
        }
        if let Some(value) = named.value() {
            let value_hir = self.lower_value(
                Some(value),
                ValueType::StringExpression,
                parent_scope,
                named.node_id(),
                named.value_range().unwrap_or(named.range()),
            );
            entries.push(RequirementEntry::Attribute(Attribute {
                id: self.alloc(Some(named.node_id()), named.range()),
                kind: AttributeKind::Pkg,
                poison: value_hir.poison.clone(),
                value: value_hir,
                range: named.range(),
            }));
        }
        if let Some(body) = named.block() {
            let body_entries = body.entries();
            let child_scope = self.prepare_scope(
                body.node_id(),
                body.range(),
                &body_entries,
                Some(parent_scope),
                true,
            );
            scope = Some(child_scope);
            entries.extend(self.lower_requirement_entries(
                &body_entries,
                child_scope,
                RequirementContext::EntityFact,
                path_names,
            ));
        }
        EntityDemand {
            id,
            name,
            name_range,
            optional: named.optional(),
            assignment_sugar,
            scope,
            entries,
            range: named.range(),
            poison,
        }
    }

    fn lower_resource(
        &mut self,
        block: dotfile_syntax::SigilBlock<'a>,
        parent_scope: ScopeId,
        path_names: &mut BTreeMap<String, Vec<ByteRange>>,
    ) -> ResourceDemand {
        let id = self.alloc(Some(block.node_id()), block.range());
        let mut poison = Vec::new();
        let Some(body) = block.block() else {
            self.context(
                block.range(),
                "resource demand is missing its block",
                "add a block containing exactly one direct bare `@key`",
            );
            let scope = parent_scope;
            return ResourceDemand {
                id,
                kind: block.name().unwrap_or("").to_owned(),
                optional: block.optional(),
                key: None,
                scope,
                entries: Vec::new(),
                range: block.range(),
                poison: vec![Poison {
                    kind: PoisonKind::Missing,
                    range: block.range(),
                }],
            };
        };
        let body_entries = body.entries();
        let scope = self.prepare_scope(
            body.node_id(),
            body.range(),
            &body_entries,
            Some(parent_scope),
            true,
        );
        let entries = self.lower_requirement_entries(
            &body_entries,
            scope,
            RequirementContext::ResourceFact,
            path_names,
        );
        let keys: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry {
                RequirementEntry::Attribute(attribute) if attribute.kind == AttributeKind::Key => {
                    Some(attribute)
                }
                _ => None,
            })
            .collect();
        if keys.len() != 1 {
            self.context(
                block.range(),
                "resource demand requires exactly one direct bare `@key`",
                "add one `@key = resource_name` and remove duplicates",
            );
            poison.push(Poison {
                kind: if keys.is_empty() {
                    PoisonKind::Missing
                } else {
                    PoisonKind::Duplicate
                },
                range: block.range(),
            });
        }
        let key = keys
            .first()
            .and_then(|attribute| match &attribute.value.kind {
                HirValueKind::Reference(reference) => Some(reference.clone()),
                _ => None,
            });
        ResourceDemand {
            id,
            kind: "font".to_owned(),
            optional: block.optional(),
            key,
            scope,
            entries,
            range: block.range(),
            poison,
        }
    }

    fn lower_extension(
        &mut self,
        extension: dotfile_syntax::ExtendEntry<'a>,
        parent_scope: ScopeId,
        path_names: &mut BTreeMap<String, Vec<ByteRange>>,
    ) -> Extension {
        let id = self.alloc(Some(extension.node_id()), extension.range());
        let namespace = extension
            .target()
            .and_then(|target| target.namespace())
            .unwrap_or("")
            .to_owned();
        let name = extension
            .target()
            .and_then(|target| target.name())
            .unwrap_or("")
            .to_owned();
        let mut poison = Vec::new();
        if !matches!(namespace.as_str(), "entity" | "font") {
            self.context(
                extension.target_range().unwrap_or(extension.range()),
                "extension target namespace is not registered in source version 1",
                "use `entity/<name>` or `font/<key>`",
            );
            poison.push(Poison {
                kind: PoisonKind::Context,
                range: extension.target_range().unwrap_or(extension.range()),
            });
        }
        let (scope, mut entries) = if let Some(body) = extension.block() {
            let body_entries = body.entries();
            let scope = self.prepare_scope(
                body.node_id(),
                body.range(),
                &body_entries,
                Some(parent_scope),
                true,
            );
            let entries = self.lower_requirement_entries(
                &body_entries,
                scope,
                RequirementContext::Extension,
                path_names,
            );
            (scope, entries)
        } else {
            self.context(
                extension.range(),
                "extension is missing its fact block",
                "add a block containing only bindings and fact attributes",
            );
            (parent_scope, Vec::new())
        };
        if namespace == "font" {
            for entry in &mut entries {
                if let RequirementEntry::Attribute(attribute) = entry
                    && matches!(
                        attribute.kind,
                        AttributeKind::Bin
                            | AttributeKind::Service
                            | AttributeKind::Scope
                            | AttributeKind::Path
                    )
                {
                    self.context(
                        attribute.range,
                        "entity-only fact is not legal on a font extension",
                        "remove the entity-only attribute",
                    );
                    attribute.poison.push(Poison {
                        kind: PoisonKind::Context,
                        range: attribute.range,
                    });
                }
            }
        }
        Extension {
            id,
            namespace,
            name,
            scope,
            entries,
            range: extension.range(),
            poison,
        }
    }

    fn lower_path_node(
        &mut self,
        path: dotfile_syntax::PathEntry<'a>,
        parent_scope: ScopeId,
        path_names: &mut BTreeMap<String, Vec<ByteRange>>,
    ) -> PathNode {
        let id = self.alloc(Some(path.node_id()), path.range());
        let decoded = path.decoded_path().unwrap_or_default();
        let path_range = path.path_range().unwrap_or(path.range());
        let mut poison = Vec::new();
        if !validate::is_nfc(&decoded) {
            self.context(
                path_range,
                "decoded source path is not Unicode NFC",
                "rewrite the path using its NFC spelling",
            );
            poison.push(Poison {
                kind: PoisonKind::Value,
                range: path_range,
            });
        }
        let previous = Self::record_origin(path_names, decoded.clone(), path_range);
        if !previous.is_empty() {
            self.duplicate(
                path_range,
                &previous,
                format!("duplicate decoded facet path `{decoded}`"),
            );
            poison.push(Poison {
                kind: PoisonKind::Duplicate,
                range: path_range,
            });
        }
        let (scope, entries) = if let Some(body) = path.block() {
            let body_entries = body.entries();
            let scope = self.prepare_scope(
                body.node_id(),
                body.range(),
                &body_entries,
                Some(parent_scope),
                true,
            );
            let entries = self.lower_requirement_entries(
                &body_entries,
                scope,
                RequirementContext::Path,
                path_names,
            );
            (scope, entries)
        } else {
            (parent_scope, Vec::new())
        };
        PathNode {
            id,
            path: decoded,
            optional: path.optional(),
            scope,
            entries,
            range: path.range(),
            poison,
        }
    }

    fn lower_recipients(&mut self, ast: AstFile<'a>) -> RecipientKeys {
        let entries = ast.entries();
        let root_scope = self.prepare_scope(ast.node_id(), ast.range(), &entries, None, false);
        let id = self.alloc(Some(ast.node_id()), ast.range());
        let mut result = RecipientKeys {
            id,
            entries: Vec::new(),
            poison: Vec::new(),
        };
        let mut root_blocks = Vec::new();
        for entry in entries {
            let Entry::Named(named) = entry else {
                match entry {
                    Entry::Let(_) => {}
                    Entry::Error(error) => result.poison.push(self.retain_poison(
                        error.node_id(),
                        error.range(),
                        PoisonKind::Syntax,
                    )),
                    other => result.poison.push(self.context_poison(
                        &other,
                        "recipient-key file accepts only one `recipients` block",
                    )),
                }
                continue;
            };
            if named.name() != Some("recipients")
                || named.optional()
                || named.value().is_some()
                || named.block().is_none()
            {
                result.poison.push(self.context_poison(
                    &Entry::Named(named),
                    "recipient-key root shape must be `recipients { ... }`",
                ));
                continue;
            }
            let previous = Self::record_singleton(&mut root_blocks, named.range());
            if !previous.is_empty() {
                self.duplicate(named.range(), &previous, "duplicate `recipients` block");
                result.poison.push(self.retain_poison(
                    named.node_id(),
                    named.range(),
                    PoisonKind::Duplicate,
                ));
                continue;
            }
            let body = named.block().expect("guarded above");
            let body_entries = body.entries();
            let scope = self.prepare_scope(
                body.node_id(),
                body.range(),
                &body_entries,
                Some(root_scope),
                false,
            );
            let mut labels = BTreeMap::new();
            for child in body_entries {
                let Entry::Named(field) = child else {
                    match child {
                        Entry::Let(_) => {}
                        Entry::Error(error) => result.poison.push(self.retain_poison(
                            error.node_id(),
                            error.range(),
                            PoisonKind::Syntax,
                        )),
                        other => result.poison.push(self.context_poison(
                            &other,
                            "recipient block accepts only label assignments",
                        )),
                    }
                    continue;
                };
                let label = field.name().unwrap_or("").to_owned();
                let label_range = field.name_range().unwrap_or(field.range());
                let field_id = self.alloc(Some(field.node_id()), field.range());
                let mut field_poison = Vec::new();
                if field.optional() || field.block().is_some() || field.value().is_none() {
                    self.context(
                        field.range(),
                        "recipient record must be a non-optional assignment",
                        "use `label = \"age1...\"`",
                    );
                    field_poison.push(Poison {
                        kind: PoisonKind::Context,
                        range: field.range(),
                    });
                }
                if !validate::is_label(&label) {
                    self.context(
                        label_range,
                        "recipient label does not match the registered ASCII pattern",
                        "use an alphanumeric first character followed by alphanumeric, dot, underscore, or hyphen",
                    );
                    field_poison.push(Poison {
                        kind: PoisonKind::Value,
                        range: label_range,
                    });
                }
                let previous = Self::record_origin(&mut labels, label.clone(), label_range);
                if !previous.is_empty() {
                    self.duplicate(
                        label_range,
                        &previous,
                        format!("duplicate recipient label `{label}`"),
                    );
                    field_poison.push(Poison {
                        kind: PoisonKind::Duplicate,
                        range: label_range,
                    });
                }
                let value = self.lower_value(
                    field.value(),
                    ValueType::Recipient,
                    scope,
                    field.node_id(),
                    field.value_range().unwrap_or(field.range()),
                );
                field_poison.extend(value.poison.clone());
                result.entries.push(NamedField {
                    id: field_id,
                    name: label,
                    name_range: label_range,
                    value,
                    range: field.range(),
                    poison: field_poison,
                });
            }
        }
        if root_blocks.is_empty() {
            self.context(
                ast.range(),
                "recipient-key file is missing its `recipients` block",
                "add exactly one `recipients` block",
            );
            result.poison.push(Poison {
                kind: PoisonKind::Missing,
                range: ast.range(),
            });
        }
        result
    }

    fn lower_scan_rules(&mut self, ast: AstFile<'a>) -> SecretScanRules {
        let entries = ast.entries();
        let root_scope = self.prepare_scope(ast.node_id(), ast.range(), &entries, None, false);
        let id = self.alloc(Some(ast.node_id()), ast.range());
        let mut result = SecretScanRules {
            id,
            rules: Vec::new(),
            poison: Vec::new(),
        };
        let mut allow_blocks = Vec::new();
        for entry in entries {
            let Entry::Named(allow) = entry else {
                match entry {
                    Entry::Let(_) => {}
                    Entry::Error(error) => result.poison.push(self.retain_poison(
                        error.node_id(),
                        error.range(),
                        PoisonKind::Syntax,
                    )),
                    other => result.poison.push(
                        self.context_poison(&other, "scan file accepts only one `allow` block"),
                    ),
                }
                continue;
            };
            if allow.name() != Some("allow")
                || allow.optional()
                || allow.value().is_some()
                || allow.block().is_none()
            {
                result.poison.push(self.context_poison(
                    &Entry::Named(allow),
                    "scan root shape must be `allow { ... }`",
                ));
                continue;
            }
            let previous = Self::record_singleton(&mut allow_blocks, allow.range());
            if !previous.is_empty() {
                self.duplicate(allow.range(), &previous, "duplicate `allow` block");
                result.poison.push(self.retain_poison(
                    allow.node_id(),
                    allow.range(),
                    PoisonKind::Duplicate,
                ));
                continue;
            }
            let body = allow.block().expect("guarded above");
            let body_entries = body.entries();
            let allow_scope = self.prepare_scope(
                body.node_id(),
                body.range(),
                &body_entries,
                Some(root_scope),
                false,
            );
            for child in body_entries {
                let Entry::Named(rule) = child else {
                    match child {
                        Entry::Let(_) => {}
                        Entry::Error(error) => result.poison.push(self.retain_poison(
                            error.node_id(),
                            error.range(),
                            PoisonKind::Syntax,
                        )),
                        other => {
                            result.poison.push(self.context_poison(
                                &other,
                                "`allow` accepts only repeated `rule` blocks",
                            ))
                        }
                    }
                    continue;
                };
                let rule_id = self.alloc(Some(rule.node_id()), rule.range());
                let mut lowered = ScanRule {
                    id: rule_id,
                    pattern: None,
                    inspect: None,
                    poison: Vec::new(),
                };
                if rule.name() != Some("rule")
                    || rule.optional()
                    || rule.value().is_some()
                    || rule.block().is_none()
                {
                    self.context(
                        rule.range(),
                        "scan record must be `rule { pattern = ..., inspect = ... }`",
                        "use a non-optional `rule` block",
                    );
                    lowered.poison.push(Poison {
                        kind: PoisonKind::Context,
                        range: rule.range(),
                    });
                }
                if let Some(rule_body) = rule.block() {
                    let mut field_origins = BTreeMap::<String, Vec<ByteRange>>::new();
                    let rule_entries = rule_body.entries();
                    let rule_scope = self.prepare_scope(
                        rule_body.node_id(),
                        rule_body.range(),
                        &rule_entries,
                        Some(allow_scope),
                        false,
                    );
                    for field_entry in rule_entries {
                        let Entry::Named(field) = field_entry else {
                            match field_entry {
                                Entry::Let(_) => {}
                                Entry::Error(error) => lowered.poison.push(self.retain_poison(
                                    error.node_id(),
                                    error.range(),
                                    PoisonKind::Syntax,
                                )),
                                other => {
                                    lowered.poison.push(self.context_poison(
                                        &other,
                                        "scan rule contains an unknown field",
                                    ))
                                }
                            }
                            continue;
                        };
                        let name = field.name().unwrap_or("").to_owned();
                        let expected = match name.as_str() {
                            "pattern" => Some(ValueType::ScanPattern),
                            "inspect" => Some(ValueType::Inspect),
                            _ => None,
                        };
                        let Some(expected) = expected else {
                            lowered.poison.push(self.context_poison(
                                &Entry::Named(field),
                                "scan rule contains an unknown field",
                            ));
                            continue;
                        };
                        let mut shape_poison = Vec::new();
                        if field.optional() || field.block().is_some() || field.value().is_none() {
                            self.context(
                                field.range(),
                                "scan-rule field must be a non-optional assignment",
                                "use `pattern = \"...\"` or `inspect = \"path\"`",
                            );
                            shape_poison.push(Poison {
                                kind: PoisonKind::Context,
                                range: field.range(),
                            });
                        }
                        let field_id = self.alloc(Some(field.node_id()), field.range());
                        let value = self.lower_value(
                            field.value(),
                            expected,
                            rule_scope,
                            field.node_id(),
                            field.value_range().unwrap_or(field.range()),
                        );
                        shape_poison.extend(value.poison.clone());
                        let mut named_field = NamedField {
                            id: field_id,
                            name: name.clone(),
                            name_range: field.name_range().unwrap_or(field.range()),
                            range: field.range(),
                            poison: shape_poison,
                            value,
                        };
                        let slot = if name == "pattern" {
                            &mut lowered.pattern
                        } else {
                            &mut lowered.inspect
                        };
                        let previous =
                            Self::record_origin(&mut field_origins, name.clone(), field.range());
                        if !previous.is_empty() {
                            self.duplicate(
                                field.range(),
                                &previous,
                                format!("duplicate scan-rule field `{name}`"),
                            );
                            named_field.poison.push(Poison {
                                kind: PoisonKind::Duplicate,
                                range: field.range(),
                            });
                            self.retain_named_field(named_field);
                        } else {
                            *slot = Some(named_field);
                        }
                    }
                }
                for (present, name) in [
                    (lowered.pattern.is_some(), "pattern"),
                    (lowered.inspect.is_some(), "inspect"),
                ] {
                    if !present {
                        self.context(
                            rule.range(),
                            format!("scan rule is missing required field `{name}`"),
                            "add the required quoted field",
                        );
                        lowered.poison.push(Poison {
                            kind: PoisonKind::Missing,
                            range: rule.range(),
                        });
                    }
                }
                result.rules.push(lowered);
            }
        }
        if allow_blocks.is_empty() {
            self.context(
                ast.range(),
                "scan file is missing its `allow` block",
                "add exactly one `allow` block",
            );
            result.poison.push(Poison {
                kind: PoisonKind::Missing,
                range: ast.range(),
            });
        }
        result
    }

    fn lower_benchmarks(&mut self, ast: AstFile<'a>) -> BenchmarkBaselines {
        let entries = ast.entries();
        let root_scope = self.prepare_scope(ast.node_id(), ast.range(), &entries, None, false);
        let id = self.alloc(Some(ast.node_id()), ast.range());
        let mut result = BenchmarkBaselines {
            id,
            hosts: Vec::new(),
            poison: Vec::new(),
        };
        let mut hosts = BTreeMap::new();
        for entry in entries {
            let Entry::Named(host) = entry else {
                match entry {
                    Entry::Let(_) => {}
                    Entry::Error(error) => result.poison.push(self.retain_poison(
                        error.node_id(),
                        error.range(),
                        PoisonKind::Syntax,
                    )),
                    other => result.poison.push(self.context_poison(
                        &other,
                        "benchmark baseline root accepts only host blocks",
                    )),
                }
                continue;
            };
            let name = host.name().unwrap_or("").to_owned();
            let name_range = host.name_range().unwrap_or(host.range());
            let host_id = self.alloc(Some(host.node_id()), host.range());
            let mut lowered = BenchmarkHost {
                id: host_id,
                name: name.clone(),
                name_range,
                epochs: Vec::new(),
                poison: Vec::new(),
            };
            if host.optional() || host.value().is_some() || host.block().is_none() {
                self.context(
                    host.range(),
                    "benchmark host must be a non-optional named block",
                    "use `host { epoch = \"run-id\" }`",
                );
                lowered.poison.push(Poison {
                    kind: PoisonKind::Context,
                    range: host.range(),
                });
            }
            let previous = Self::record_origin(&mut hosts, name.clone(), name_range);
            if !previous.is_empty() {
                self.duplicate(
                    name_range,
                    &previous,
                    format!("duplicate benchmark host `{name}`"),
                );
                lowered.poison.push(Poison {
                    kind: PoisonKind::Duplicate,
                    range: name_range,
                });
            }
            if let Some(body) = host.block() {
                let body_entries = body.entries();
                let scope = self.prepare_scope(
                    body.node_id(),
                    body.range(),
                    &body_entries,
                    Some(root_scope),
                    false,
                );
                let mut epochs = BTreeMap::new();
                for child in body_entries {
                    let Entry::Named(epoch_field) = child else {
                        match child {
                            Entry::Let(_) => {}
                            Entry::Error(error) => lowered.poison.push(self.retain_poison(
                                error.node_id(),
                                error.range(),
                                PoisonKind::Syntax,
                            )),
                            other => lowered.poison.push(self.context_poison(
                                &other,
                                "benchmark host accepts only epoch assignments",
                            )),
                        }
                        continue;
                    };
                    let epoch = epoch_field.name().unwrap_or("").to_owned();
                    let epoch_range = epoch_field.name_range().unwrap_or(epoch_field.range());
                    let field_id = self.alloc(Some(epoch_field.node_id()), epoch_field.range());
                    let mut field_poison = Vec::new();
                    if epoch_field.optional()
                        || epoch_field.block().is_some()
                        || epoch_field.value().is_none()
                    {
                        self.context(
                            epoch_field.range(),
                            "benchmark epoch record must be a non-optional assignment",
                            "use `deadbeef = \"<matching-run-id>\"`",
                        );
                        field_poison.push(Poison {
                            kind: PoisonKind::Context,
                            range: epoch_field.range(),
                        });
                    }
                    if !validate::is_benchmark_epoch(&epoch) {
                        self.context(
                            epoch_range,
                            "benchmark epoch must be eight lowercase hexadecimal digits",
                            "use the producer-derived eight-digit epoch",
                        );
                        field_poison.push(Poison {
                            kind: PoisonKind::Value,
                            range: epoch_range,
                        });
                    }
                    let previous = Self::record_origin(&mut epochs, epoch.clone(), epoch_range);
                    if !previous.is_empty() {
                        self.duplicate(
                            epoch_range,
                            &previous,
                            format!("duplicate benchmark epoch `{epoch}`"),
                        );
                        field_poison.push(Poison {
                            kind: PoisonKind::Duplicate,
                            range: epoch_range,
                        });
                    }
                    let mut value = self.lower_value(
                        epoch_field.value(),
                        ValueType::BenchmarkRunId,
                        scope,
                        epoch_field.node_id(),
                        epoch_field.value_range().unwrap_or(epoch_field.range()),
                    );
                    if let Some(run_id) = value_text(&value)
                        && !validate::is_benchmark_run_id_for_epoch(run_id, &epoch)
                    {
                        self.context(
                            value.range,
                            "benchmark run ID is malformed or its epoch suffix does not match the key",
                            "use the immutable producer run ID for this epoch",
                        );
                        value.poison.push(Poison {
                            kind: PoisonKind::Value,
                            range: value.range,
                        });
                    }
                    field_poison.extend(value.poison.clone());
                    lowered.epochs.push(NamedField {
                        id: field_id,
                        name: epoch,
                        name_range: epoch_range,
                        value,
                        range: epoch_field.range(),
                        poison: field_poison,
                    });
                }
            }
            result.hosts.push(lowered);
        }
        result
    }

    fn lower_deferred(
        &mut self,
        ast: AstFile<'a>,
        domain: Domain,
        location: &DomainLocation,
    ) -> DeferredDomain {
        let id = self.alloc(Some(ast.node_id()), ast.range());
        let identity = match location {
            DomainLocation::ThemeProfile { theme } => Some(theme.clone()),
            DomainLocation::GroupRoot { group, .. } => Some(group.clone()),
            DomainLocation::Facet { group, package, .. } => Some(format!("{group}/{package}")),
            DomainLocation::OverrideVariant {
                group,
                variant,
                package,
                ..
            } => Some(format!("{group}/{package}@{variant}")),
            DomainLocation::Fixed => None,
        };
        let (syntax, missing) = self.lower_deferred_syntax_children(ast.node_id());
        DeferredDomain {
            id,
            domain,
            identity,
            syntax,
            missing,
            range: ast.range(),
            poison: if self.parse.has_errors() {
                vec![Poison {
                    kind: PoisonKind::Syntax,
                    range: ast.range(),
                }]
            } else {
                Vec::new()
            },
        }
    }

    fn lower_deferred_syntax_children(
        &mut self,
        parent: NodeId,
    ) -> (Vec<DeferredSyntaxNode>, Vec<DeferredMissing>) {
        let elements = self.parse.cst().children(parent).to_vec();
        let mut nodes = Vec::new();
        let mut missing = Vec::new();
        for element in elements {
            match element {
                Element::Node(syntax) => {
                    let kind = self.parse.cst().node_kind(syntax);
                    let range = self.parse.cst().node_range(syntax);
                    let id = self.alloc(Some(syntax), range);
                    let (children, child_missing) = self.lower_deferred_syntax_children(syntax);
                    nodes.push(DeferredSyntaxNode {
                        id,
                        kind,
                        range,
                        children,
                        missing: child_missing,
                        poison: (kind == NodeKind::Error)
                            .then_some(Poison {
                                kind: PoisonKind::Syntax,
                                range,
                            })
                            .into_iter()
                            .collect(),
                    });
                }
                Element::Missing { kind, at } => {
                    let range = ByteRange::new(at, at, self.source.len())
                        .expect("CST missing terminal offset is source-bounded");
                    let id = self.alloc(None, range);
                    missing.push(DeferredMissing {
                        id,
                        expected: kind,
                        range,
                        poison: vec![Poison {
                            kind: PoisonKind::Syntax,
                            range,
                        }],
                    });
                }
                Element::Token(_) => {}
            }
        }
        (nodes, missing)
    }

    fn context_poison(&mut self, entry: &Entry<'a>, summary: &'static str) -> Poison {
        self.context(
            entry.range(),
            summary,
            "remove the entry or use the registered shape for this domain",
        );
        self.retain_poison(entry.node_id(), entry.range(), PoisonKind::Context)
    }
}

fn binding_hir_for(bindings: &[BindingDeclaration], id: BindingId) -> HirId {
    bindings[id.0 as usize].hir_id
}

fn is_plain_single_string(expression: StringExpr<'_>) -> bool {
    let atoms = expression.atoms();
    matches!(
        atoms.as_slice(),
        [Atom::String { data: Some(data), .. }] if !data.has_interpolation()
    )
}

fn value_text(value: &HirValue) -> Option<&str> {
    match &value.kind {
        HirValueKind::String(expression) if !expression.evaluated.poisoned => {
            Some(&expression.evaluated.value)
        }
        _ => None,
    }
}

fn normalized_hostname(value: &str) -> String {
    value
        .strip_suffix('.')
        .unwrap_or(value)
        .to_ascii_lowercase()
}

fn find_group<'a>(groups: &'a [GroupDeclaration], name: &str) -> Option<&'a GroupDeclaration> {
    for group in groups {
        if group.name == name {
            return Some(group);
        }
        if let Some(found) = find_group(&group.children, name) {
            return Some(found);
        }
    }
    None
}

fn requirement_attribute_type(context: RequirementContext, name: &str) -> Option<ValueType> {
    let fact = || match name {
        "pkg" => Some(ValueType::StringExpression),
        "installer" => Some(ValueType::Installer),
        "bin" => Some(ValueType::CommandName),
        "check" => Some(ValueType::Check),
        "version" => Some(ValueType::String),
        "family" => Some(ValueType::StringOrStringList),
        "service" => Some(ValueType::String),
        "scope" => Some(ValueType::ServiceScope),
        "path" => Some(ValueType::MachinePath),
        "description" => Some(ValueType::OneLineString),
        _ => None,
    };
    let deployment = || match name {
        "destination" => Some(ValueType::DestinationPath),
        "deploy" => Some(ValueType::Deploy),
        "privilege" => Some(ValueType::Privilege),
        "sensitivity" => Some(ValueType::Sensitivity),
        "mode" => Some(ValueType::Mode),
        "owner" | "group" => Some(ValueType::String),
        "description" => Some(ValueType::OneLineString),
        "theme" => Some(ValueType::ThemeReference),
        _ => None,
    };
    match context {
        RequirementContext::GroupRoot => (name == "theme").then_some(ValueType::ThemeReference),
        RequirementContext::Facet | RequirementContext::Variant => deployment(),
        RequirementContext::EntityFact => fact(),
        RequirementContext::ResourceFact => {
            if name == "key" {
                Some(ValueType::ResourceKeyReference)
            } else {
                match name {
                    "pkg" => Some(ValueType::StringExpression),
                    "installer" => Some(ValueType::Installer),
                    "check" => Some(ValueType::Check),
                    "version" => Some(ValueType::String),
                    "family" => Some(ValueType::StringOrStringList),
                    "description" => Some(ValueType::OneLineString),
                    _ => None,
                }
            }
        }
        RequirementContext::Extension => fact(),
        RequirementContext::Path => match name {
            "expect" => Some(ValueType::Expect),
            "destination" => Some(ValueType::DestinationPath),
            "deploy" => Some(ValueType::Deploy),
            "privilege" => Some(ValueType::Privilege),
            "sensitivity" => Some(ValueType::Sensitivity),
            "mode" => Some(ValueType::Mode),
            "owner" | "group" => Some(ValueType::String),
            _ => None,
        },
    }
}

fn group_attribute_type(name: &str) -> Option<ValueType> {
    match name {
        "directory" => Some(ValueType::RepositoryDirectory),
        "os" => Some(ValueType::Os),
        "arch" => Some(ValueType::ArchitectureList),
        "description" => Some(ValueType::OneLineString),
        _ => None,
    }
}

fn profile_attribute_type(name: &str) -> Option<ValueType> {
    match name {
        "groups" => Some(ValueType::GroupReferenceList),
        "manager" => Some(ValueType::Manager),
        "os" => Some(ValueType::Os),
        "arch" => Some(ValueType::ArchitectureList),
        "theme" => Some(ValueType::ThemeReference),
        "description" => Some(ValueType::OneLineString),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClassificationError, GroupLayout};
    use dotfile_syntax::parse;
    use std::collections::BTreeSet;

    fn lowered(input: &str, path: &str, domain: Domain) -> (Parse, LoweredFile) {
        let path = RepoPath::new(path).unwrap();
        let source = SourceText::from(input);
        let parsed = parse(&path, &source);
        let lowered = lower(
            &path,
            &source,
            &parsed,
            ClassifiedPath {
                domain,
                location: DomainLocation::Fixed,
            },
        )
        .expect("parse/source pair must match");
        (parsed, lowered)
    }

    fn errors(lowered: &LoweredFile) -> Vec<&Diagnostic> {
        lowered
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .collect()
    }

    fn assert_source_map_ids_are_owned(lowered: &LoweredFile) {
        fn own(ids: &mut BTreeSet<HirId>, id: HirId) {
            assert!(ids.insert(id), "HIR id {id:?} has multiple owners");
        }

        fn value(ids: &mut BTreeSet<HirId>, item: &HirValue) {
            own(ids, item.id);
            if let HirValueKind::List(items) = &item.kind {
                for item in items {
                    value(ids, item);
                }
            }
        }

        fn attribute(ids: &mut BTreeSet<HirId>, attribute: &Attribute) {
            own(ids, attribute.id);
            value(ids, &attribute.value);
        }

        fn field(ids: &mut BTreeSet<HirId>, field: &NamedField) {
            own(ids, field.id);
            value(ids, &field.value);
        }

        fn requirements(ids: &mut BTreeSet<HirId>, entries: &[RequirementEntry]) {
            for entry in entries {
                match entry {
                    RequirementEntry::Binding(_) => {}
                    RequirementEntry::Attribute(item) => attribute(ids, item),
                    RequirementEntry::Entity(item) => {
                        own(ids, item.id);
                        requirements(ids, &item.entries);
                    }
                    RequirementEntry::Resource(item) => {
                        own(ids, item.id);
                        requirements(ids, &item.entries);
                    }
                    RequirementEntry::Extension(item) => {
                        own(ids, item.id);
                        requirements(ids, &item.entries);
                    }
                    RequirementEntry::Path(item) => {
                        own(ids, item.id);
                        requirements(ids, &item.entries);
                    }
                    RequirementEntry::Poison(item) => own(ids, item.id),
                }
            }
        }

        fn groups(ids: &mut BTreeSet<HirId>, items: &[GroupDeclaration]) {
            for item in items {
                own(ids, item.id);
                for attribute_item in &item.attributes {
                    attribute(ids, attribute_item);
                }
                groups(ids, &item.children);
            }
        }

        fn deferred(ids: &mut BTreeSet<HirId>, items: &[DeferredSyntaxNode]) {
            for item in items {
                own(ids, item.id);
                for missing in &item.missing {
                    own(ids, missing.id);
                }
                deferred(ids, &item.children);
            }
        }

        let mut owned = BTreeSet::new();
        for scope in &lowered.hir.scopes {
            own(&mut owned, scope.hir_id);
        }
        for binding in &lowered.hir.bindings {
            own(&mut owned, binding.hir_id);
        }
        match &lowered.hir.root {
            HirRoot::Profiles(root) => {
                own(&mut owned, root.id);
                if let Some(item) = &root.version {
                    attribute(&mut owned, item);
                }
                if let Some(item) = &root.theme {
                    attribute(&mut owned, item);
                }
                groups(&mut owned, &root.groups);
                for profile in &root.profiles {
                    own(&mut owned, profile.id);
                    for item in &profile.attributes {
                        attribute(&mut owned, item);
                    }
                }
            }
            HirRoot::Hosts(root) => {
                own(&mut owned, root.id);
                for host in &root.hosts {
                    own(&mut owned, host.id);
                    for item in [&host.hostnames, &host.role].into_iter().flatten() {
                        field(&mut owned, item);
                    }
                    for item in [&host.profile, &host.theme].into_iter().flatten() {
                        attribute(&mut owned, item);
                    }
                    for item in &host.extensions {
                        field(&mut owned, item);
                    }
                }
            }
            HirRoot::Requirements(root) => {
                own(&mut owned, root.id);
                requirements(&mut owned, &root.entries);
            }
            HirRoot::RecipientKeys(root) => {
                own(&mut owned, root.id);
                for item in &root.entries {
                    field(&mut owned, item);
                }
            }
            HirRoot::SecretScanRules(root) => {
                own(&mut owned, root.id);
                for rule in &root.rules {
                    own(&mut owned, rule.id);
                    for item in [&rule.pattern, &rule.inspect].into_iter().flatten() {
                        field(&mut owned, item);
                    }
                }
            }
            HirRoot::BenchmarkBaselines(root) => {
                own(&mut owned, root.id);
                for host in &root.hosts {
                    own(&mut owned, host.id);
                    for item in &host.epochs {
                        field(&mut owned, item);
                    }
                }
            }
            HirRoot::Deferred(root) => {
                own(&mut owned, root.id);
                for missing in &root.missing {
                    own(&mut owned, missing.id);
                }
                deferred(&mut owned, &root.syntax);
            }
            HirRoot::Unknown(root) => own(&mut owned, root.id),
        }
        for item in &lowered.hir.recovery {
            match item {
                RecoveryNode::Attribute(item) => attribute(&mut owned, item),
                RecoveryNode::NamedField(item) => field(&mut owned, item),
                RecoveryNode::Poison(item) => own(&mut owned, item.id),
            }
        }

        let mapped: BTreeSet<_> = lowered.source_map.hir_origins().map(|(id, _)| id).collect();
        assert_eq!(mapped, owned, "source map contains orphan HIR identities");
    }

    #[test]
    fn complete_profiles_lower_to_owned_hir_and_dynamic_layout() {
        let input = r#"@dotfile-version = "1"
@groups {
    shared { @directory = "shared" }
    linux {
        @os = "linux"
        desktop { @directory = "linux/desktop" }
    }
}
@profiles {
    workstation {
        @groups = [desktop]
        @manager = "pacman"
        @os = "linux"
    }
}
"#;
        let (parsed, mut lowered) = lowered(input, "config/profiles.dotfile", Domain::Profiles);
        assert!(!parsed.has_errors());
        assert!(errors(&lowered).is_empty(), "{:?}", lowered.diagnostics);
        let HirRoot::Profiles(profiles) = &lowered.hir.root else {
            panic!("profiles root");
        };
        let groups = profiles.profiles[0]
            .attribute(AttributeKind::Groups)
            .expect("required @groups");
        let HirValueKind::List(group_references) = &groups.value.kind else {
            panic!("group-reference list");
        };
        assert_eq!(group_references[0].value_type, ValueType::GroupReference);
        assert!(matches!(
            &group_references[0].kind,
            HirValueKind::Reference(reference)
                if reference.namespace == ReferenceNamespace::Group
        ));
        let synthetic = ByteRange::at(0, input.len() as u64).unwrap();
        assert_eq!(
            lowered.allocate_synthetic_hir(synthetic),
            None,
            "only deferred domain lowerers may allocate synthetic HIR"
        );
        let validated = lowered.clone().into_validated(&parsed).unwrap();
        let layout = GroupLayout::from_profiles(&validated).unwrap();
        assert_eq!(layout.entries().len(), 2);
        assert!(
            layout.entries().iter().any(
                |entry| entry.group == "desktop" && entry.directory.as_str() == "linux/desktop"
            )
        );
        assert!(lowered.into_validated(&parsed).is_ok());
    }

    #[test]
    fn scalar_string_or_list_values_normalize_to_one_element_lists() {
        let (_, lowered) = lowered(
            "tool { @family = \"debian\" }\n",
            "shared/package.dotfile",
            Domain::GroupRootRequirements,
        );
        assert!(errors(&lowered).is_empty(), "{:?}", lowered.diagnostics);
        let HirRoot::Requirements(requirements) = &lowered.hir.root else {
            panic!("requirements root");
        };
        let RequirementEntry::Entity(entity) = &requirements.entries[0] else {
            panic!("entity demand");
        };
        let attribute = entity
            .entries
            .iter()
            .find_map(|entry| match entry {
                RequirementEntry::Attribute(attribute)
                    if attribute.kind == AttributeKind::Family =>
                {
                    Some(attribute)
                }
                _ => None,
            })
            .expect("@family");
        let HirValueKind::List(items) = &attribute.value.kind else {
            panic!("normalized list");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].value_type, ValueType::String);
        assert_eq!(value_text(&items[0]), Some("debian"));
    }

    #[test]
    fn deferred_domains_own_a_recursive_source_mapped_syntax_skeleton() {
        fn descendants(nodes: &[DeferredSyntaxNode]) -> usize {
            nodes
                .iter()
                .map(|node| 1 + descendants(&node.children))
                .sum()
        }

        let (parsed, theme_hir) = lowered(
            "roles { foreground = \"#ffffff\" }\n",
            "theme/roles.dotfile",
            Domain::ThemeRoles,
        );
        let HirRoot::Deferred(deferred) = &theme_hir.hir.root else {
            panic!("deferred root");
        };
        assert!(descendants(&deferred.syntax) >= 3);
        assert_eq!(
            theme_hir.source_map.hir_for_syntax(parsed.cst().root()),
            &[deferred.id]
        );
        fn assert_mapped(nodes: &[DeferredSyntaxNode], source_map: &SourceMap) {
            for node in nodes {
                let origin = source_map.source_for_hir(node.id).expect("node origin");
                assert_eq!(origin.range, node.range);
                assert!(origin.syntax.is_some());
                assert_mapped(&node.children, source_map);
            }
        }
        assert_mapped(&deferred.syntax, &theme_hir.source_map);
        assert!(theme_hir.deferred_snapshot_is_authoritative());
        let mut forged = theme_hir.clone();
        let synthetic = ByteRange::at(0, forged.source_len()).unwrap();
        assert!(forged.allocate_synthetic_hir(synthetic).is_some());
        assert!(!forged.deferred_snapshot_is_authoritative());
        assert!(theme_hir.into_validated(&parsed).is_err());

        fn missing_count(nodes: &[DeferredSyntaxNode]) -> usize {
            nodes
                .iter()
                .map(|node| node.missing.len() + missing_count(&node.children))
                .sum()
        }
        let (_, malformed) = lowered(
            "roles { foreground =\n",
            "theme/roles.dotfile",
            Domain::ThemeRoles,
        );
        let HirRoot::Deferred(malformed) = &malformed.hir.root else {
            panic!("deferred root");
        };
        assert!(malformed.missing.len() + missing_count(&malformed.syntax) > 0);
    }

    #[test]
    fn validated_gate_requires_known_parse_and_schema_valid_input() {
        let (parsed, valid) = lowered("", "shared/zsh/package.dotfile", Domain::FacetRequirements);
        assert!(valid.into_validated(&parsed).is_ok());

        let (parsed, schema_invalid) = lowered(
            "@deploy = \"copy\"\n",
            "shared/package.dotfile",
            Domain::GroupRootRequirements,
        );
        assert!(!parsed.has_errors());
        assert!(schema_invalid.into_validated(&parsed).is_err());

        let (parsed, syntax_invalid) = lowered(
            "@deploy = [\n",
            "shared/zsh/package.dotfile",
            Domain::FacetRequirements,
        );
        assert!(parsed.has_errors());
        assert!(syntax_invalid.into_validated(&parsed).is_err());

        let path = RepoPath::new("README.txt").unwrap();
        let source = SourceText::from("");
        let parsed = parse(&path, &source);
        let unowned = lower_path(&path, &source, &parsed, &DomainClassifier::without_groups())
            .expect("parse/source pair must match");
        assert!(unowned.diagnostics.is_empty());
        assert!(unowned.into_validated(&parsed).is_err());
    }

    #[test]
    fn lexer_only_invalid_trivia_cannot_cross_the_validated_gate() {
        let (parsed, lowered) = lowered(
            "\r",
            "shared/zsh/package.dotfile",
            Domain::FacetRequirements,
        );

        // The bare CR belongs to a trivia gap, so the CST itself needs no
        // parser recovery. The immutable Parse diagnostic stream must still
        // make this otherwise schema-valid file unsealable.
        assert!(!parsed.cst().has_error());
        assert!(parsed.has_errors());
        assert!(parsed.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == "lex/encoding" && diagnostic.summary == "bare carriage return"
        }));
        assert!(lowered.diagnostics.is_empty());
        assert!(lowered.into_validated(&parsed).is_err());
    }

    #[test]
    fn source_map_is_bidirectional_for_root_values_and_segments() {
        let input = "@let base = \"org.example\"\nwezterm = $base \".wezterm\"\n";
        let (parsed, lowered) = lowered(
            input,
            "shared/package.dotfile",
            Domain::GroupRootRequirements,
        );
        let root = lowered.hir.root.hir_id();
        assert!(
            lowered
                .source_map
                .hir_for_syntax(parsed.cst().root())
                .contains(&root)
        );
        let literal = lowered.hir.bindings[0].evaluated.segments[0].source_range;
        assert!(!lowered.source_map.hir_for_range(literal).is_empty());
        for (hir, origin) in lowered.source_map.hir_origins() {
            assert!(
                lowered
                    .source_map
                    .hir_for_range(origin.range)
                    .contains(&hir)
            );
            if let Some(syntax) = origin.syntax {
                assert!(lowered.source_map.hir_for_syntax(syntax).contains(&hir));
            }
        }
    }

    #[test]
    fn every_source_map_identity_has_one_owned_hir_object() {
        let cases = [
            (
                concat!(
                    "@dotfile-version = \"1\"\n",
                    "@dotfile-version = \"1\"\n",
                    "@groups { shared { @directory = \"shared\" } bad = \"x\" }\n",
                    "@groups {}\n",
                    "@profiles {}\n",
                    "@theme = mocha\n",
                    "@theme = latte\n",
                ),
                "config/profiles.dotfile",
                Domain::Profiles,
            ),
            (
                concat!(
                    "archie {\n",
                    "hostnames = [\"archie\"]\n",
                    "hostnames = [\"duplicate\"]\n",
                    "role = \"desktop\"\n",
                    "role = \"server\"\n",
                    "@profile = workstation\n",
                    "@profile = server\n",
                    "nested {}\n",
                    "}\n",
                    "@theme = mocha\n",
                ),
                "config/hosts.dotfile",
                Domain::Hosts,
            ),
            (
                concat!(
                    "allow { rule {\n",
                    "pattern = \"shared/**\"\n",
                    "pattern = \"linux/**\"\n",
                    "inspect = \"path\"\n",
                    "unknown = \"x\"\n",
                    "} }\n",
                    "allow {}\n",
                ),
                "config/scan.dotfile",
                Domain::SecretScanRules,
            ),
            (
                "recipients {}\nrecipients {}\n",
                "config/keys.dotfile",
                Domain::RecipientKeys,
            ),
            (
                "@theme = mocha\n",
                "benchmarks/baselines.dotfile",
                Domain::BenchmarkBaselines,
            ),
            (
                "@font { @key = key\nnested {} }\n",
                "shared/package.dotfile",
                Domain::GroupRootRequirements,
            ),
            (
                "roles { foreground =\n",
                "theme/roles.dotfile",
                Domain::ThemeRoles,
            ),
        ];

        for (input, path, domain) in cases {
            let (_, lowered) = lowered(input, path, domain);
            assert_source_map_ids_are_owned(&lowered);
        }
    }

    #[test]
    fn duplicate_and_invalid_singletons_are_retained_in_recovery_hir() {
        let input = concat!(
            "archie {\n",
            "hostnames = [\"archie\"]\n",
            "hostnames = [\"duplicate\"]\n",
            "role = \"desktop\"\n",
            "@profile = workstation\n",
            "@profile = server\n",
            "nested {}\n",
            "}\n",
        );
        let (parsed, lowered) = lowered(input, "config/hosts.dotfile", Domain::Hosts);
        assert!(!parsed.has_errors());
        assert!(lowered.hir.recovery.iter().any(|item| matches!(
            item,
            RecoveryNode::NamedField(field) if field.name == "hostnames"
        )));
        assert!(lowered.hir.recovery.iter().any(|item| matches!(
            item,
            RecoveryNode::Attribute(attribute) if attribute.kind == AttributeKind::Profile
        )));
        assert!(lowered.hir.recovery.iter().any(|item| matches!(
            item,
            RecoveryNode::Poison(node)
                if node.poison.iter().any(|poison| poison.kind == PoisonKind::Context)
        )));
        for item in &lowered.hir.recovery {
            let id = item.hir_id();
            let origin = lowered.source_map.source_for_hir(id).unwrap();
            assert!(lowered.source_map.hir_for_range(origin.range).contains(&id));
            if let Some(syntax) = origin.syntax {
                assert!(lowered.source_map.hir_for_syntax(syntax).contains(&id));
            }
        }
        assert_source_map_ids_are_owned(&lowered);
        let mut recovery_only = lowered;
        recovery_only.diagnostics.clear();
        recovery_only.hir.poison.clear();
        assert!(!recovery_only.hir.recovery.is_empty());
        assert!(recovery_only.into_validated(&parsed).is_err());
    }

    #[test]
    fn later_duplicate_attributes_and_aliases_report_every_prior_origin() {
        let attribute_input = concat!(
            "@description = \"first\"\n",
            "@description = \"second\"\n",
            "@description = \"third\"\n",
        );
        let (_, attributes) = lowered(
            attribute_input,
            "shared/zsh/package.dotfile",
            Domain::FacetRequirements,
        );
        let duplicates = attributes
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == "schema/duplicate"
                    && diagnostic.summary == "duplicate attribute `@description`"
            })
            .collect::<Vec<_>>();
        assert_eq!(duplicates.len(), 2, "{:#?}", attributes.diagnostics);
        assert_eq!(duplicates[0].related_spans.len(), 1);
        assert_eq!(duplicates[1].related_spans.len(), 2);
        assert!(
            duplicates[1].related_spans[0].start_byte < duplicates[1].related_spans[1].start_byte
        );
        assert!(duplicates[1].related_spans[1].start_byte < duplicates[1].primary_span.start_byte);
        assert_source_map_ids_are_owned(&attributes);

        let alias_input = concat!(
            "first { hostnames = [\"same\"], role = \"desktop\", @profile = workstation }\n",
            "second { hostnames = [\"same\"], role = \"desktop\", @profile = workstation }\n",
            "third { hostnames = [\"same\"], role = \"desktop\", @profile = workstation }\n",
        );
        let (_, aliases) = lowered(alias_input, "config/hosts.dotfile", Domain::Hosts);
        let duplicates = aliases
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == "schema/duplicate"
                    && diagnostic.summary == "duplicate hostname alias `same`"
            })
            .collect::<Vec<_>>();
        assert_eq!(duplicates.len(), 2, "{:#?}", aliases.diagnostics);
        assert_eq!(duplicates[0].related_spans.len(), 1);
        assert_eq!(duplicates[1].related_spans.len(), 2);
        assert!(
            duplicates[1].related_spans[0].start_byte < duplicates[1].related_spans[1].start_byte
        );
        assert!(duplicates[1].related_spans[1].start_byte < duplicates[1].primary_span.start_byte);
        assert_source_map_ids_are_owned(&aliases);
    }

    #[test]
    fn binding_initializers_see_outer_before_inner_shadow_starts() {
        let input = r#"@let x = "outer"
tool {
    @let x = $x "/inner"
    @pkg = $x
}
"#;
        let (_, lowered) = lowered(
            input,
            "shared/package.dotfile",
            Domain::GroupRootRequirements,
        );
        assert!(errors(&lowered).is_empty(), "{:?}", lowered.diagnostics);
        let inner = lowered
            .hir
            .bindings
            .iter()
            .find(|binding| binding.name == "x" && binding.scope != ScopeId(0))
            .unwrap();
        assert_eq!(inner.evaluated.value, "outer/inner");
        assert!(inner.used);
        assert!(inner.evaluated.segments.iter().any(|segment| {
            segment
                .binding_edges
                .iter()
                .any(|edge| edge.binding == BindingId(0))
        }));
    }

    #[test]
    fn earlier_outer_binding_wins_over_later_local_shadow() {
        let input = r#"@let x = "outer"
tool {
    @let y = $x
    @let x = "inner"
    @pkg = $y
}
"#;
        let (_, lowered) = lowered(
            input,
            "shared/package.dotfile",
            Domain::GroupRootRequirements,
        );
        assert!(errors(&lowered).is_empty(), "{:?}", lowered.diagnostics);
        let y = lowered
            .hir
            .bindings
            .iter()
            .find(|binding| binding.name == "y")
            .unwrap();
        assert_eq!(y.evaluated.value, "outer");
        assert!(lowered.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "schema/binding"
                && diagnostic.detail.as_deref() == Some("unused_binding")
                && diagnostic.primary_span.start_byte
                    == lowered
                        .hir
                        .bindings
                        .iter()
                        .find(|binding| binding.name == "x" && binding.scope != ScopeId(0))
                        .unwrap()
                        .name_range
                        .start()
        }));
    }

    #[test]
    fn invalid_nested_string_list_items_resolve_their_local_binding_provenance() {
        let input = concat!(
            "fontconfig {\n",
            "@let local = \"Hack\"\n",
            "@family = [\"${local}\"]\n",
            "}\n",
        );
        let (_, lowered) = lowered(
            input,
            "shared/package.dotfile",
            Domain::GroupRootRequirements,
        );
        assert!(lowered.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "schema/context"
                && diagnostic.summary == "`string` requires one uninterpolated quoted string"
        }));
        assert!(!lowered.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "schema/binding" && diagnostic.summary.contains("not declared")
        }));
        let binding = lowered
            .hir
            .bindings
            .iter()
            .find(|binding| binding.name == "local")
            .unwrap();
        assert!(binding.used);
        let HirRoot::Requirements(root) = &lowered.hir.root else {
            panic!("requirements root");
        };
        let RequirementEntry::Entity(entity) = &root.entries[0] else {
            panic!("entity demand");
        };
        let family = entity
            .entries
            .iter()
            .find_map(|entry| match entry {
                RequirementEntry::Attribute(attribute)
                    if attribute.kind == AttributeKind::Family =>
                {
                    Some(attribute)
                }
                _ => None,
            })
            .unwrap();
        let HirValueKind::List(items) = &family.value.kind else {
            panic!("family list");
        };
        let HirValueKind::String(item) = &items[0].kind else {
            panic!("family string item");
        };
        assert_eq!(item.evaluated.value, "Hack");
        assert!(item.evaluated.segments.iter().any(|segment| {
            segment
                .binding_edges
                .iter()
                .any(|edge| edge.binding == binding.id)
        }));
    }

    #[test]
    fn use_before_late_declaration_reports_reference_and_prologue() {
        let input = "tool { @pkg = $later }\n@let later = \"tool\"\n";
        let (_, lowered) = lowered(
            input,
            "shared/package.dotfile",
            Domain::GroupRootRequirements,
        );
        let summaries: Vec<_> = lowered
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.summary.as_str())
            .collect();
        assert!(summaries.contains(&"binding is used before its declaration"));
        assert!(summaries.contains(&"binding declaration appears after the block prologue"));
    }

    #[test]
    fn source_version_mismatch_uses_registered_required_detail() {
        let (_, lowered) = lowered(
            "@dotfile-version = \"2\"\n@groups {}\n@profiles {}\n",
            "config/profiles.dotfile",
            Domain::Profiles,
        );
        let diagnostic = lowered
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.detail.as_deref() == Some("unsupported_dotfile_version"))
            .expect("registered version diagnostic");
        assert_eq!(diagnostic.code, "schema/context");
        assert_eq!(diagnostic.actual["version"], "2");
    }

    #[test]
    fn source_version_requires_the_exact_bootstrap_value_spelling() {
        for declaration in [
            "@dotfile-version = \"\\u{31}\"",
            "@dotfile-version =\n\"1\"",
        ] {
            let input = format!(
                "{declaration}\n@groups {{ shared {{ @directory = \"shared\" }} }}\n@profiles {{}}\n"
            );
            let (_, lowered) = lowered(&input, "config/profiles.dotfile", Domain::Profiles);
            assert!(
                lowered.diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == "schema/context"
                        && diagnostic.summary
                            == "`@dotfile-version` must use the exact source-version preamble shape"
                }),
                "declaration: {declaration}"
            );
        }

        for input in [
            "@dotfile-version = \"1\", @groups { shared { @directory = \"shared\" } }\n@profiles {}\n",
            "@dotfile-version = \"1\" @groups { shared { @directory = \"shared\" } }\n@profiles {}\n",
        ] {
            let (parsed, lowered) = lowered(input, "config/profiles.dotfile", Domain::Profiles);
            assert!(lowered.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "schema/context"
                    && diagnostic.summary
                        == "`@dotfile-version` must use the exact source-version preamble shape"
            }));
            assert!(lowered.into_validated(&parsed).is_err());
        }

        let valid = concat!(
            "@dotfile-version = \"1\" # pinned\n",
            "@groups { shared { @directory = \"shared\" } }\n",
            "@profiles {}\n",
        );
        let (parsed, lowered) = lowered(valid, "config/profiles.dotfile", Domain::Profiles);
        assert!(errors(&lowered).is_empty(), "{:?}", lowered.diagnostics);
        assert!(lowered.into_validated(&parsed).is_ok());
    }

    #[test]
    fn host_id_may_repeat_as_its_own_explicit_alias() {
        let input = r#"archie {
    hostnames = ["archie", "archie.local"]
    role = "desktop"
    @profile = workstation
}
"#;
        let (_, lowered) = lowered(input, "config/hosts.dotfile", Domain::Hosts);
        assert!(errors(&lowered).is_empty(), "{:?}", lowered.diagnostics);
    }

    #[test]
    fn source_paths_are_checked_for_nfc_during_schema_lowering() {
        let input = "./\"cafe\u{301}\" { @deploy = \"none\" }\n";
        let (_, lowered) = lowered(
            input,
            "shared/zsh/package.dotfile",
            Domain::FacetRequirements,
        );
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary == "decoded source path is not Unicode NFC")
        );
    }

    #[test]
    fn resource_scan_and_benchmark_record_shapes_are_closed() {
        let (_, nested_resource) = lowered(
            "@font { @key = hack, nested }\n",
            "shared/package.dotfile",
            Domain::GroupRootRequirements,
        );
        assert!(nested_resource.diagnostics.iter().any(|diagnostic| {
            diagnostic.summary == "entity demands are not legal in this requirement context"
        }));

        let (_, scan) = lowered(
            "allow { rule { ?pattern = \"shared/**\", inspect = \"path\" } }\n",
            "config/scan.dotfile",
            Domain::SecretScanRules,
        );
        assert!(
            scan.diagnostics.iter().any(|diagnostic| diagnostic.summary
                == "scan-rule field must be a non-optional assignment")
        );

        let (_, benchmark) = lowered(
            "archie { ?10db7d1f = \"2026-08-13T11-34-32Z-10db7d1f\" }\n",
            "benchmarks/baselines.dotfile",
            Domain::BenchmarkBaselines,
        );
        assert!(benchmark.diagnostics.iter().any(|diagnostic| {
            diagnostic.summary == "benchmark epoch record must be a non-optional assignment"
        }));
    }

    #[test]
    fn immediate_attribute_context_matrix_is_closed() {
        const ALL: &[&str] = &[
            "dotfile-version",
            "pkg",
            "installer",
            "bin",
            "check",
            "version",
            "family",
            "service",
            "scope",
            "path",
            "key",
            "destination",
            "deploy",
            "privilege",
            "sensitivity",
            "mode",
            "owner",
            "group",
            "expect",
            "directory",
            "os",
            "arch",
            "groups",
            "manager",
            "profile",
            "theme",
            "description",
        ];
        let requirement_cases: &[(RequirementContext, &[&str])] = &[
            (RequirementContext::GroupRoot, &["theme"]),
            (
                RequirementContext::Facet,
                &[
                    "destination",
                    "deploy",
                    "privilege",
                    "sensitivity",
                    "mode",
                    "owner",
                    "group",
                    "description",
                    "theme",
                ],
            ),
            (
                RequirementContext::Variant,
                &[
                    "destination",
                    "deploy",
                    "privilege",
                    "sensitivity",
                    "mode",
                    "owner",
                    "group",
                    "description",
                    "theme",
                ],
            ),
            (
                RequirementContext::EntityFact,
                &[
                    "pkg",
                    "installer",
                    "bin",
                    "check",
                    "version",
                    "family",
                    "service",
                    "scope",
                    "path",
                    "description",
                ],
            ),
            (
                RequirementContext::ResourceFact,
                &[
                    "key",
                    "pkg",
                    "installer",
                    "check",
                    "version",
                    "family",
                    "description",
                ],
            ),
            (
                RequirementContext::Extension,
                &[
                    "pkg",
                    "installer",
                    "bin",
                    "check",
                    "version",
                    "family",
                    "service",
                    "scope",
                    "path",
                    "description",
                ],
            ),
            (
                RequirementContext::Path,
                &[
                    "destination",
                    "deploy",
                    "privilege",
                    "sensitivity",
                    "mode",
                    "owner",
                    "group",
                    "expect",
                ],
            ),
        ];
        for (context, legal) in requirement_cases {
            for name in ALL {
                assert_eq!(
                    requirement_attribute_type(*context, name).is_some(),
                    legal.contains(name),
                    "context={context:?}, attribute=@{name}"
                );
            }
        }

        for name in ALL {
            assert_eq!(
                group_attribute_type(name).is_some(),
                ["directory", "os", "arch", "description"].contains(name),
                "group attribute=@{name}"
            );
            assert_eq!(
                profile_attribute_type(name).is_some(),
                ["groups", "manager", "os", "arch", "theme", "description"].contains(name),
                "profile attribute=@{name}"
            );
        }

        // Resource extensions use the resource subset and never accept key,
        // while entity extensions use every extension entry above.
        for name in ALL {
            let resource_extension = matches!(
                *name,
                "pkg" | "installer" | "check" | "version" | "family" | "description"
            );
            let generic = requirement_attribute_type(RequirementContext::Extension, name).is_some();
            assert!(!resource_extension || generic);
            if *name == "key" {
                assert!(!resource_extension);
            }
        }
    }

    #[test]
    fn lowering_rejects_a_parse_from_different_path_or_source_bytes() {
        let path = RepoPath::new("shared/package.dotfile").unwrap();
        let other_path = RepoPath::new("shared/other.dotfile").unwrap();
        let original = SourceText::from("alpha = one\n");
        let different_same_length = SourceText::from("bravo = two\n");
        assert_eq!(original.len(), different_same_length.len());
        let parsed = parse(&path, &original);

        assert!(matches!(
            lower(
                &path,
                &different_same_length,
                &parsed,
                ClassifiedPath {
                    domain: Domain::GroupRootRequirements,
                    location: DomainLocation::Fixed,
                },
            ),
            Err(LoweringError::MismatchedParse)
        ));
        assert!(matches!(
            lower_path(
                &path,
                &different_same_length,
                &parsed,
                &DomainClassifier::without_groups(),
            ),
            Err(LoweringError::MismatchedParse)
        ));
        assert!(matches!(
            lower(
                &other_path,
                &original,
                &parsed,
                ClassifiedPath {
                    domain: Domain::GroupRootRequirements,
                    location: DomainLocation::Fixed,
                },
            ),
            Err(LoweringError::MismatchedParse)
        ));
        assert!(matches!(
            lower_path(
                &other_path,
                &original,
                &parsed,
                &DomainClassifier::without_groups(),
            ),
            Err(LoweringError::MismatchedParse)
        ));
    }

    #[test]
    fn validated_gate_rechecks_the_parse_path_and_source_identity() {
        let path = RepoPath::new("shared/package.dotfile").unwrap();
        let source = SourceText::from("");
        let parsed = parse(&path, &source);
        let lowered = lower(
            &path,
            &source,
            &parsed,
            ClassifiedPath {
                domain: Domain::GroupRootRequirements,
                location: DomainLocation::Fixed,
            },
        )
        .unwrap();
        let other_source = SourceText::from("\n");
        let other_parse = parse(&path, &other_source);
        assert!(lowered.into_validated(&other_parse).is_err());

        let lowered = lower(
            &path,
            &source,
            &parsed,
            ClassifiedPath {
                domain: Domain::GroupRootRequirements,
                location: DomainLocation::Fixed,
            },
        )
        .unwrap();
        let other_path = RepoPath::new("shared/other.dotfile").unwrap();
        let other_parse = parse(&other_path, &source);
        assert!(lowered.into_validated(&other_parse).is_err());
    }

    #[test]
    fn poisoned_profiles_cannot_seed_dynamic_classification() {
        for input in [
            "@dotfile-version = \"1\"\n@groups { shared { @directory = \"../bad\" } }\n@profiles {}\n",
            "@dotfile-version = \"1\"\n@groups { shared { @directory = \"shared\" } }\n",
            "@dotfile-version = \"2\"\n@groups { shared { @directory = \"shared\" } }\n@profiles {}\n",
            "@dotfile-version = \"1\"\n@groups {\nshared {\n@directory = \"shared\"\n@os = \"windows\"\n}\n}\n@profiles {}\n",
            "@dotfile-version = \"1\"\n@groups { shared { @directory = \"shared\" } broken = \"x\" }\n@profiles {}\n",
            "@dotfile-version = \"1\"\n@let hidden = \"shared\"\n@groups { shared { @directory = \"shared\" } }\n@profiles {}\n",
            "@dotfile-version = \"1\"\n@groups { @let hidden = \"shared\"\nshared { @directory = \"shared\" } }\n@profiles {}\n",
            "@dotfile-version = \"1\"\n@groups { shared { @directory = \"shared\"\n@arch = [\"bogus\"] } }\n@profiles {}\n",
            "@dotfile-version = \"1\"\n@groups { shared { @directory = \"shared\" } }\n@profiles { bad { @groups = [\"shared\"]\n@manager = \"apt\"\n@os = \"linux\" } }\n",
            "@dotfile-version = \"1\"\n@groups { shared { @directory = \"shared\" } }\n@profiles { bad { @groups = [shared, shared]\n@manager = \"apt\"\n@os = \"linux\" } }\n",
        ] {
            let (parsed, lowered) = lowered(input, "config/profiles.dotfile", Domain::Profiles);
            assert!(
                lowered.into_validated(&parsed).is_err(),
                "poisoned profiles unexpectedly sealed: {input}"
            );
        }
    }

    #[test]
    fn group_layout_rejects_a_validated_non_profiles_file() {
        let (parsed, requirements) =
            lowered("", "shared/zsh/package.dotfile", Domain::FacetRequirements);
        let validated = requirements.into_validated(&parsed).unwrap();
        assert_eq!(
            GroupLayout::from_profiles(&validated),
            Err(ClassificationError::PoisonedProfiles)
        );
    }
}
