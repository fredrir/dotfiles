use std::collections::BTreeSet;

use dotfmt_core::edit::{Edit, Finding, Related, Severity, Tag};
use oxc_ast::ast::{
    ImportDeclaration, ImportDeclarationSpecifier, Statement, TSImportEqualsDeclaration,
    TSModuleReference,
};
use oxc_span::GetSpan;

use super::context::Context;

pub(crate) struct ImportAnalysis {
    pub findings: Vec<Finding>,
    pub changes: Vec<ImportChange>,
}

pub(crate) struct ImportChange {
    pub decl_start: usize,
    pub delete: bool,
    pub edit: Edit,
    pub finding: Option<Finding>,
    pub title: String,
}

pub(crate) fn analyze<'a, 'p>(context: &Context<'a, 'p>) -> ImportAnalysis {
    let mut analysis = ImportAnalysis {
        findings: Vec::new(),
        changes: Vec::new(),
    };
    let protected: BTreeSet<String> = context.protected_jsx_names().into_iter().collect();
    for statement in &context.program.body {
        match statement {
            Statement::ImportDeclaration(declaration) => {
                analyze_declaration(context, declaration, &protected, &mut analysis);
            }
            Statement::TSImportEqualsDeclaration(declaration) => {
                analyze_import_equals(context, declaration, &mut analysis);
            }
            _ => {}
        }
    }
    analysis
}

fn analyze_import_equals<'a, 'p>(
    context: &Context<'a, 'p>,
    declaration: &'p TSImportEqualsDeclaration<'a>,
    analysis: &mut ImportAnalysis,
) {
    let Some(symbol_id) = declaration.id.symbol_id.get() else {
        return;
    };
    if !context.is_unused(symbol_id) {
        return;
    }
    let start = declaration.id.span.start as usize;
    let end = declaration.id.span.end as usize;
    let module_span = declaration.module_reference.span();
    let mut finding = Finding::new(
        "unused-import",
        format!("`{}` is imported but never used", declaration.id.name),
        Severity::Warning,
        start,
        end,
    )
    .with_tag(Tag::Unnecessary);
    let source = match &declaration.module_reference {
        TSModuleReference::ExternalModuleReference(reference) => {
            reference.expression.value.as_str()
        }
        _ => None,
    };
    match source {
        Some(source) => finding.related.push(Related {
            start: module_span.start as usize,
            end: module_span.end as usize,
            message: format!("imported from \"{source}\""),
        }),
        None => finding.related.push(Related {
            start: module_span.start as usize,
            end: module_span.end as usize,
            message: "imported here".to_owned(),
        }),
    }
    if let Some(related) = context.type_diagnostic_related(start, end) {
        finding.related.push(related.into());
    }
    analysis.findings.push(finding);
}

fn analyze_declaration<'a, 'p>(
    context: &Context<'a, 'p>,
    declaration: &'p ImportDeclaration<'a>,
    protected: &BTreeSet<String>,
    analysis: &mut ImportAnalysis,
) {
    let start = declaration.span.start as usize;
    let end = declaration.span.end as usize;
    if context.is_ignored(start, end) {
        return;
    }
    let Some(specifiers) = declaration.specifiers.as_ref() else {
        return;
    };
    if specifiers.is_empty() {
        return;
    }

    let source_span = declaration.source.span;
    let mut findings = Vec::new();
    let mut used = Vec::with_capacity(specifiers.len());
    for specifier in specifiers {
        let local = local_of(specifier);
        let is_protected = protected.contains(local.name.as_str());
        let is_used = is_protected
            || local
                .symbol_id
                .get()
                .is_none_or(|symbol_id| !context.is_unused(symbol_id));
        used.push(is_used);
        if !is_used {
            let local_start = local.span.start as usize;
            let local_end = local.span.end as usize;
            let mut finding = Finding::new(
                "unused-import",
                format!("`{}` is imported but never used", local.name),
                Severity::Warning,
                local_start,
                local_end,
            )
            .with_tag(Tag::Unnecessary)
            .with_related(Related {
                start: source_span.start as usize,
                end: source_span.end as usize,
                message: format!("imported from \"{}\"", source_value(declaration)),
            });
            if let Some(related) = context.type_diagnostic_related(local_start, local_end) {
                finding.related.push(related.into());
            }
            findings.push(finding);
        }
    }

    let all_unused = used.iter().all(|used| !used);
    let declaration_is_type = declaration.import_kind.is_type();
    let has_inline_type = specifiers.iter().any(|specifier| {
        matches!(
            specifier,
            ImportDeclarationSpecifier::ImportSpecifier(specifier)
                if specifier.import_kind.is_type()
        )
    });
    let mut type_only_used = 0usize;
    let mut value_used = 0usize;
    for (specifier, used) in specifiers.iter().zip(&used) {
        if !used {
            continue;
        }
        let local = local_of(specifier);
        let is_explicit_type = declaration_is_type
            || matches!(
                specifier,
                ImportDeclarationSpecifier::ImportSpecifier(specifier)
                    if specifier.import_kind.is_type()
            );
        if is_explicit_type
            || (!protected.contains(local.name.as_str())
                && local
                    .symbol_id
                    .get()
                    .is_some_and(|symbol_id| context.references_are_type_only(symbol_id)))
        {
            type_only_used += 1;
        } else {
            value_used += 1;
        }
    }

    let convertible = !declaration_is_type
        && !context.is_declaration
        && type_only_used > 0
        && value_used == 0
        && !has_inline_type
        && context.tsconfig.allows_type_only_import_conversion()
        && convertible_shape(specifiers);
    let report_type_only = !declaration_is_type
        && !context.is_declaration
        && context.tsconfig.allows_type_only_import_conversion()
        && type_only_used > 0
        && (value_used > 0 || has_inline_type || !convertible_shape(specifiers));

    if report_type_only && !convertible {
        for (specifier, used) in specifiers.iter().zip(&used) {
            if !used {
                continue;
            }
            let local = local_of(specifier);
            let is_explicit_type = declaration_is_type
                || matches!(
                    specifier,
                    ImportDeclarationSpecifier::ImportSpecifier(specifier)
                        if specifier.import_kind.is_type()
                );
            if is_explicit_type
                || !local
                    .symbol_id
                    .get()
                    .is_some_and(|symbol_id| context.references_are_type_only(symbol_id))
            {
                continue;
            }
            findings.push(
                Finding::new(
                    "type-only-import",
                    if value_used > 0 {
                        format!(
                            "`{}` is only used as a type, but this import also has value bindings; replace it with an explicit `type` specifier",
                            local.name
                        )
                    } else {
                        format!(
                            "`{}` is only used as a type; this import cannot be converted as a whole",
                            local.name
                        )
                    },
                    Severity::Information,
                    local.span.start as usize,
                    local.span.end as usize,
                )
                .with_tag(Tag::Unnecessary)
                .with_related(Related {
                    start: source_span.start as usize,
                    end: source_span.end as usize,
                    message: format!("imported from \"{}\"", source_value(declaration)),
                }),
            );
        }
    } else if convertible {
        let local = specifiers
            .iter()
            .zip(&used)
            .find(|(specifier, used)| {
                **used
                    && !matches!(
                        specifier,
                        ImportDeclarationSpecifier::ImportSpecifier(specifier)
                            if specifier.import_kind.is_type()
                    )
            })
            .map_or_else(
                || local_of(&specifiers[0]),
                |(specifier, _)| local_of(specifier),
            );
        findings.push(
            Finding::new(
                "type-only-import",
                format!(
                    "`{}` is only used as a type; convert this import to a type-only import",
                    local.name
                ),
                Severity::Information,
                local.span.start as usize,
                local.span.end as usize,
            )
            .with_tag(Tag::Unnecessary)
            .with_related(Related {
                start: source_span.start as usize,
                end: source_span.end as usize,
                message: format!("imported from \"{}\"", source_value(declaration)),
            }),
        );
    }

    analysis.findings.extend(findings.iter().cloned());
    if !context.options.fix || context.is_declaration || context.has_eval {
        return;
    }
    if declaration.phase.is_some()
        || context.has_comment_in(start, end)
        || context.has_adjacent_comment(start, end)
    {
        return;
    }
    if context.blocked_by_type_diagnostic(start, end).is_some() {
        return;
    }

    let kept: Vec<&ImportDeclarationSpecifier<'a>> = specifiers
        .iter()
        .zip(&used)
        .filter_map(|(specifier, used)| used.then_some(specifier))
        .collect();
    if kept.len() == specifiers.len() && !convertible {
        return;
    }

    let edit = if all_unused {
        let declaration_module = declaration_is_type
            || (context.is_typescript
                && context.tsconfig.elides_unused_imports()
                && source_is_declaration_module(context, declaration));
        if declaration_module {
            let (start, end) = context.statement_edit(start, end);
            Edit::delete(start, end)
        } else {
            let suffix = &context.text[source_span.start as usize..end];
            Edit::replace(start, end, format!("import {suffix}"))
        }
    } else {
        let rebuilt = rebuild(context, declaration, &kept, convertible);
        Edit::replace(start, end, rebuilt)
    };
    let title = match (
        findings
            .iter()
            .any(|finding| finding.code == "unused-import"),
        convertible,
    ) {
        (true, true) => "Remove unused imports and convert to a type-only import",
        (true, false) => "Remove unused imports",
        (false, true) => "Convert to a type-only import",
        (false, false) => return,
    };
    let delete = edit.start < start || edit.new_text.is_empty();
    analysis.changes.push(ImportChange {
        decl_start: start,
        delete,
        edit,
        finding: findings.first().cloned(),
        title: title.to_owned(),
    });
}

fn source_is_declaration_module(
    context: &Context<'_, '_>,
    declaration: &ImportDeclaration<'_>,
) -> bool {
    let value = source_value(declaration);
    if !(value.starts_with('.') || value.starts_with('/')) {
        return false;
    }
    let base = context
        .path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(value);
    for candidate in [
        base.with_extension("d.ts"),
        base.with_extension("d.mts"),
        base.with_extension("d.cts"),
        base.join("index.d.ts"),
    ] {
        if candidate.is_file() {
            return true;
        }
    }
    false
}

fn rebuild<'a, 'p>(
    context: &Context<'a, 'p>,
    declaration: &ImportDeclaration<'a>,
    kept: &[&ImportDeclarationSpecifier<'a>],
    type_only: bool,
) -> String {
    if kept.is_empty() {
        let suffix =
            &context.text[declaration.source.span.start as usize..declaration.span.end as usize];
        return format!("import {suffix}");
    }

    let mut default = None;
    let mut namespace = None;
    let mut named = Vec::new();
    for specifier in kept {
        match specifier {
            ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                default = Some(specifier);
            }
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                namespace = Some(specifier);
            }
            ImportDeclarationSpecifier::ImportSpecifier(specifier) => named.push(specifier.span),
        }
    }

    let mut output = String::from("import ");
    if type_only || declaration.import_kind.is_type() {
        output.push_str("type ");
    }
    if let Some(default) = default {
        output.push_str(
            &context.text[default.local.span.start as usize..default.local.span.end as usize],
        );
    }
    if let Some(namespace) = namespace {
        if default.is_some() {
            output.push_str(", ");
        }
        output.push_str(&context.text[namespace.span.start as usize..namespace.span.end as usize]);
    }
    if !named.is_empty() {
        if default.is_some() || namespace.is_some() {
            output.push_str(", ");
        }
        output.push_str("{ ");
        for (index, span) in named.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            output.push_str(&context.text[span.start as usize..span.end as usize]);
        }
        output.push_str(" }");
    }
    output.push_str(" from ");
    output.push_str(
        &context.text[declaration.source.span.start as usize..declaration.span.end as usize],
    );
    output
}

fn convertible_shape(specifiers: &[ImportDeclarationSpecifier<'_>]) -> bool {
    let mut default = false;
    let mut namespace = false;
    let mut named = false;
    for specifier in specifiers {
        match specifier {
            ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => default = true,
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => namespace = true,
            ImportDeclarationSpecifier::ImportSpecifier(_) => named = true,
        }
    }
    !(default && (namespace || named)) && !(namespace && named)
}

fn source_value<'a>(declaration: &'a ImportDeclaration<'a>) -> &'a str {
    declaration.source.value.as_str().unwrap_or("<invalid>")
}

fn local_of<'a>(
    specifier: &'a ImportDeclarationSpecifier<'a>,
) -> &'a oxc_ast::ast::BindingIdentifier<'a> {
    match specifier {
        ImportDeclarationSpecifier::ImportSpecifier(specifier) => &specifier.local,
        ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => &specifier.local,
        ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => &specifier.local,
    }
}
