use std::collections::HashSet;

use super::context::Context;
use dotfmt_core::edit::{Analysis, Applicability, Edit, Finding, Fix, Severity, Tag, apply};
use oxc_ast::AstKind;
use oxc_ast::ast::{
    ArrayExpressionElement, BindingPattern, Expression, ObjectPropertyKind, UnaryOperator,
    VariableDeclarationKind,
};
use oxc_semantic::SymbolFlags;

pub(crate) fn analyze<'a, 'p>(context: &Context<'a, 'p>, analysis: &mut Analysis) {
    unused_variables(context, analysis);
    unused_parameters(context, analysis);
    unused_type_parameters(context, analysis);
    unused_private_members(context, analysis);
}

fn unused_variables<'a, 'p>(context: &Context<'a, 'p>, analysis: &mut Analysis) {
    let scoping = context.scoping();
    for symbol_id in scoping.symbol_ids() {
        let flags = scoping.symbol_flags(symbol_id);
        if !flags.intersects(SymbolFlags::Variable | SymbolFlags::ConstVariable)
            || flags.contains(SymbolFlags::Ambient)
            || !scoping.get_resolved_reference_ids(symbol_id).is_empty()
        {
            continue;
        }
        let declaration = scoping.symbol_declaration(symbol_id);
        let AstKind::VariableDeclarator(declarator) = context.semantic.nodes().kind(declaration)
        else {
            continue;
        };
        let AstKind::VariableDeclaration(variable) =
            context.semantic.nodes().parent_kind(declaration)
        else {
            continue;
        };
        if variable.declare
            || !matches!(
                variable.kind,
                VariableDeclarationKind::Var
                    | VariableDeclarationKind::Let
                    | VariableDeclarationKind::Const
            )
            || context.is_exported(scoping.symbol_span(symbol_id))
        {
            continue;
        }
        let span = scoping.symbol_span(symbol_id);
        let start = span.start as usize;
        let end = span.end as usize;
        let in_for = matches!(
            context.semantic.nodes().parent_kind(variable.node_id()),
            AstKind::ForStatement(_) | AstKind::ForInStatement(_) | AstKind::ForOfStatement(_)
        );
        let destructuring = !matches!(declarator.id, BindingPattern::BindingIdentifier(_));
        let single = variable.declarations.len() == 1;
        let comments =
            context.has_adjacent_comment(variable.span.start as usize, variable.span.end as usize);
        let pure_initializer = declarator.init.as_ref().is_none_or(is_side_effect_free);
        let message = if pure_initializer {
            format!(
                "`{}` is declared but never used",
                scoping.symbol_name(symbol_id)
            )
        } else {
            format!(
                "`{}` is declared but never used; its initializer has side effects",
                scoping.symbol_name(symbol_id)
            )
        };
        let severity = unused_local_severity(context.tsconfig.no_unused_locals);
        let mut finding = Finding::new("unused-variable", message, severity, start, end)
            .with_tag(Tag::Unnecessary);
        if let Some(related) = context.type_diagnostic_related(start, end) {
            finding.related.push(related.into());
        }
        analysis.findings.push(finding.clone());

        if !context.options.fix
            || context.is_declaration
            || context.has_eval
            || destructuring
            || comments
            || !single
            || in_for
            || (context.is_script
                && matches!(variable.kind, VariableDeclarationKind::Var)
                && !has_function_body(context, variable.node_id()))
        {
            continue;
        }
        // Removing a declaration is destructive even when its initializer is
        // pure, so the fix is unsafe and only selected by unsafe fix modes.
        if !pure_initializer && !context.options.unsafe_fixes {
            continue;
        }
        let applicability = Applicability::Unsafe;
        if context
            .blocked_by_type_diagnostic(variable.span.start as usize, variable.span.end as usize)
            .is_some()
        {
            continue;
        }
        let (start, end) =
            context.statement_edit(variable.span.start as usize, variable.span.end as usize);
        let edit = Edit::delete(start, end);
        let Ok(candidate) = apply(context.text, std::slice::from_ref(&edit)) else {
            continue;
        };
        // The same guard as export fixes: a deletion that does not re-parse
        // (for example a partial statement inside a namespace) must not be
        // offered, because applying it would abort every other finding.
        if !crate::project::reparses(context.path, &candidate) {
            continue;
        }
        let fix = Fix::new(
            format!(
                "Remove unused variable `{}`",
                scoping.symbol_name(symbol_id)
            ),
            "quickfix",
            applicability,
            vec![edit],
        )
        .with_finding(finding);
        analysis.fixes.push(fix);
    }
}

fn unused_parameters<'a, 'p>(context: &Context<'a, 'p>, analysis: &mut Analysis) {
    if context.is_declaration {
        return;
    }
    let scoping = context.scoping();
    for symbol_id in scoping.symbol_ids() {
        let flags = scoping.symbol_flags(symbol_id);
        if !flags.contains(SymbolFlags::FunctionScopedVariable)
            || flags.contains(SymbolFlags::Ambient)
            || !scoping.get_resolved_reference_ids(symbol_id).is_empty()
        {
            continue;
        }
        let name = scoping.symbol_name(symbol_id);
        if name == "this" || name.starts_with('_') {
            continue;
        }
        let declaration = scoping.symbol_declaration(symbol_id);
        let parameter = match context.semantic.nodes().kind(declaration) {
            AstKind::FormalParameter(parameter) => {
                if parameter.accessibility.is_some()
                    || parameter.readonly
                    || parameter.r#override
                    || !parameter.decorators.is_empty()
                {
                    continue;
                }
                declaration
            }
            AstKind::FormalParameterRest(parameter) => {
                if !parameter.decorators.is_empty() {
                    continue;
                }
                declaration
            }
            _ => continue,
        };
        if !param_belongs_to_value_signature(context, parameter) {
            continue;
        }
        let span = scoping.symbol_span(symbol_id);
        let severity = unused_local_severity(context.tsconfig.no_unused_parameters);
        let mut finding = Finding::new(
            "unused-parameter",
            format!("`{name}` is a parameter that is never used"),
            severity,
            span.start as usize,
            span.end as usize,
        )
        .with_tag(Tag::Unnecessary);
        if let Some(related) =
            context.type_diagnostic_related(span.start as usize, span.end as usize)
        {
            finding.related.push(related.into());
        }
        analysis.findings.push(finding);
    }
}

fn unused_type_parameters<'a, 'p>(context: &Context<'a, 'p>, analysis: &mut Analysis) {
    if context.is_declaration {
        return;
    }
    let scoping = context.scoping();
    for symbol_id in scoping.symbol_ids() {
        let flags = scoping.symbol_flags(symbol_id);
        if !flags.contains(SymbolFlags::TypeParameter)
            || !scoping.get_resolved_reference_ids(symbol_id).is_empty()
        {
            continue;
        }
        let declaration = scoping.symbol_declaration(symbol_id);
        if !matches!(
            context.semantic.nodes().kind(declaration),
            AstKind::TSTypeParameter(_)
        ) {
            continue;
        }
        let span = scoping.symbol_span(symbol_id);
        analysis.findings.push(
            Finding::new(
                "unused-type-parameter",
                format!(
                    "`{}` is a type parameter that is never used",
                    scoping.symbol_name(symbol_id)
                ),
                Severity::Warning,
                span.start as usize,
                span.end as usize,
            )
            .with_tag(Tag::Unnecessary),
        );
    }
}

fn unused_private_members<'a, 'p>(context: &Context<'a, 'p>, analysis: &mut Analysis) {
    if context.is_declaration {
        return;
    }
    let classes = context.semantic.classes();
    for (class_id, _) in classes.iter_enumerated() {
        let used: HashSet<_> = classes
            .iter_private_identifiers(class_id)
            .flat_map(|reference| reference.element_ids.iter().copied())
            .collect();
        for (element_id, element) in classes.elements[class_id].iter_enumerated() {
            if !element.is_private || used.contains(&element_id) {
                continue;
            }
            let name = element.name.display().to_string();
            if name.starts_with('_') {
                continue;
            }
            analysis.findings.push(
                Finding::new(
                    "unused-class-member",
                    format!("Private class member `#{name}` is never used"),
                    Severity::Warning,
                    element.span.start as usize,
                    element.span.end as usize,
                )
                .with_tag(Tag::Unnecessary),
            );
        }
    }
}

/// tsc reports unused locals/parameters as errors only when the option is on;
/// otherwise the finding stays informational so opt-in fixes can still use it.
fn unused_local_severity(enabled: Option<bool>) -> Severity {
    if enabled == Some(true) {
        Severity::Warning
    } else {
        Severity::Information
    }
}

/// Whether the parameter's own signature is a value function with a body.
///
/// Ancestors are walked from the parameter and the first function-like node
/// decides. This keeps parameters of `TSFunctionType`, call/method/construct
/// signatures, index signatures, `declare function` and abstract methods from
/// inheriting the body of an enclosing real function.
fn param_belongs_to_value_signature(
    context: &Context<'_, '_>,
    node_id: oxc_semantic::NodeId,
) -> bool {
    for ancestor in context.semantic.nodes().ancestors(node_id) {
        match ancestor.kind() {
            AstKind::Function(function) => return function.body.is_some(),
            AstKind::ArrowFunctionExpression(_) => return true,
            AstKind::MethodDefinition(definition) => return definition.value.body.is_some(),
            AstKind::TSFunctionType(_)
            | AstKind::TSConstructorType(_)
            | AstKind::TSCallSignatureDeclaration(_)
            | AstKind::TSMethodSignature(_)
            | AstKind::TSConstructSignatureDeclaration(_)
            | AstKind::TSIndexSignature(_) => return false,
            _ => {}
        }
    }
    false
}

fn has_function_body(context: &Context<'_, '_>, node_id: oxc_semantic::NodeId) -> bool {
    for ancestor in context.semantic.nodes().ancestors(node_id) {
        match ancestor.kind() {
            AstKind::Function(function) => return function.body.is_some(),
            AstKind::ArrowFunctionExpression(_) => return true,
            _ => {}
        }
    }
    false
}

fn is_side_effect_free(expression: &Expression<'_>) -> bool {
    // Only expressions whose evaluation cannot run user code or observe an
    // uninitialized binding. Anything that can coerce, iterate, touch a getter,
    // or throw (identifiers, binary/unary operators, spreads, template
    // interpolation) is intentionally excluded: `--fix` must never remove a
    // side effect. The unsafe fix remains available explicitly.
    match expression {
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::ArrowFunctionExpression(_)
        | Expression::FunctionExpression(_)
        | Expression::ThisExpression(_)
        | Expression::ImportMeta(_)
        | Expression::NewTarget(_) => true,
        Expression::TemplateLiteral(template) => template.expressions.is_empty(),
        Expression::ArrayExpression(array) => array.elements.iter().all(|element| match element {
            ArrayExpressionElement::Elision(_) => true,
            ArrayExpressionElement::SpreadElement(_) => false,
            _ => element.as_expression().is_some_and(is_side_effect_free),
        }),
        Expression::ObjectExpression(object) => {
            object.properties.iter().all(|property| match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    !property.computed && is_side_effect_free(&property.value)
                }
                ObjectPropertyKind::SpreadProperty(_) => false,
            })
        }
        Expression::UnaryExpression(unary) => {
            unary.operator != UnaryOperator::Delete
                && matches!(
                    unary.argument,
                    Expression::BooleanLiteral(_)
                        | Expression::NullLiteral(_)
                        | Expression::NumericLiteral(_)
                        | Expression::BigIntLiteral(_)
                        | Expression::StringLiteral(_)
                )
        }
        Expression::ParenthesizedExpression(parenthesized) => {
            is_side_effect_free(&parenthesized.expression)
        }
        Expression::TSAsExpression(assertion) => is_side_effect_free(&assertion.expression),
        Expression::TSSatisfiesExpression(assertion) => is_side_effect_free(&assertion.expression),
        Expression::TSTypeAssertion(assertion) => is_side_effect_free(&assertion.expression),
        Expression::TSNonNullExpression(expression) => is_side_effect_free(&expression.expression),
        Expression::TSInstantiationExpression(expression) => {
            is_side_effect_free(&expression.expression)
        }
        _ => false,
    }
}
