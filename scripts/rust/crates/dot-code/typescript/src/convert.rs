use dot_code_core::{Action, Edit, Request, Trigger};
use oxc_ast::AstKind;
use oxc_ast::ast::{
    Declaration, Program, Statement, TSInterfaceDeclaration, TSType, TSTypeAliasDeclaration,
    TSTypeParameterDeclaration,
};
use oxc_span::Span;

use crate::layout::{self, Layout};
use crate::names;
use crate::path::statements;

pub const INTERFACE: &str = "refactor.rewrite.typeDefinition.interface";
pub const TYPE: &str = "refactor.rewrite.typeDefinition.type";

const TO_INTERFACE: &str = "Convert to interface";
const TO_TYPE: &str = "Convert to type alias";

pub fn actions<'a>(program: &Program<'a>, path: &[AstKind<'a>], request: &Request) -> Vec<Action> {
    let Some(index) = path.iter().rposition(|kind| {
        matches!(
            kind,
            AstKind::TSTypeAliasDeclaration(_) | AstKind::TSInterfaceDeclaration(_)
        )
    }) else {
        return Vec::new();
    };
    let action = match path[index] {
        AstKind::TSTypeAliasDeclaration(alias)
            if request.wants(INTERFACE)
                && reachable(
                    request,
                    alias.span,
                    head_end(alias.id.span, alias.type_parameters.as_deref()),
                ) =>
        {
            to_interface(alias, request)
        }
        AstKind::TSInterfaceDeclaration(interface)
            if request.wants(TYPE)
                && reachable(
                    request,
                    interface.span,
                    head_end(interface.id.span, interface.type_parameters.as_deref()),
                ) =>
        {
            to_type(program, interface, &path[..index], request)
        }
        _ => None,
    };
    action.into_iter().collect()
}

/// Invoked anywhere in the declaration; the lightbulb only shows on its head.
fn reachable(request: &Request, declaration: Span, head_end: usize) -> bool {
    request.trigger == Trigger::Invoked
        || (declaration.start as usize <= request.selection.start
            && request.selection.end <= head_end)
}

fn head_end(id: Span, parameters: Option<&TSTypeParameterDeclaration>) -> usize {
    parameters.map_or(id.end, |parameters| parameters.span.end) as usize
}

fn unavailable(
    request: &Request,
    title: &'static str,
    kind: &'static str,
    reason: &'static str,
) -> Option<Action> {
    request
        .explains()
        .then(|| Action::disabled(title, kind, reason))
}

fn to_interface(alias: &TSTypeAliasDeclaration, request: &Request) -> Option<Action> {
    let source = request.source;
    let own: Vec<&str> = alias
        .type_parameters
        .iter()
        .flat_map(|declaration| declaration.params.iter())
        .map(|parameter| parameter.name.name.as_str())
        .collect();
    let parts = match &alias.type_annotation {
        TSType::TSIntersectionType(intersection) => &intersection.types[..],
        other => std::slice::from_ref(other),
    };
    let mut heritage = Vec::new();
    let mut body = None;
    for part in parts {
        match part {
            TSType::TSTypeLiteral(literal) if body.is_none() => {
                body = Some(literal.span.source_text(source));
            }
            TSType::TSTypeReference(reference)
                if names::root(&reference.type_name).is_some_and(|name| !own.contains(&name)) =>
            {
                heritage.push(reference.span.source_text(source));
            }
            _ => {
                return unavailable(
                    request,
                    TO_INTERFACE,
                    INTERFACE,
                    "Only object types and type references form an interface",
                );
            }
        }
    }
    let head = layout::swap_keyword(
        &source[alias.span.start as usize..alias.id.span.start as usize],
        "type",
        "interface",
    )?;
    let name = &source
        [alias.id.span.start as usize..head_end(alias.id.span, alias.type_parameters.as_deref())];
    let extends = if heritage.is_empty() {
        String::new()
    } else {
        format!(" extends {}", heritage.join(", "))
    };
    let text = format!("{head}{name}{extends} {}", body.unwrap_or("{}"));
    Some(Action::new(
        TO_INTERFACE,
        INTERFACE,
        vec![Edit {
            range: layout::with_terminator(source, alias.span),
            text,
        }],
    ))
}

fn to_type(
    program: &Program,
    interface: &TSInterfaceDeclaration,
    ancestors: &[AstKind],
    request: &Request,
) -> Option<Action> {
    let source = request.source;
    if ancestors.iter().any(|kind| {
        matches!(
            kind,
            AstKind::TSGlobalDeclaration(_) | AstKind::TSExternalModuleDeclaration(_)
        )
    }) {
        return unavailable(
            request,
            TO_TYPE,
            TYPE,
            "Augmentations rely on declaration merging",
        );
    }
    if matches!(ancestors.last(), Some(AstKind::ExportDefaultDeclaration(_))) {
        return unavailable(
            request,
            TO_TYPE,
            TYPE,
            "A type alias cannot be a default export",
        );
    }
    if merged(interface, ancestors) {
        return unavailable(
            request,
            TO_TYPE,
            TYPE,
            "Merged declarations cannot become a type alias",
        );
    }
    let head = layout::swap_keyword(
        &source[interface.span.start as usize..interface.id.span.start as usize],
        "interface",
        "type",
    )?;
    let name = &source[interface.id.span.start as usize
        ..head_end(interface.id.span, interface.type_parameters.as_deref())];
    let mut parts: Vec<&str> = interface
        .extends
        .iter()
        .map(|heritage| heritage.span.source_text(source))
        .collect();
    let empty =
        interface.body.body.is_empty() && !layout::has_comment(program, interface.body.span);
    if parts.is_empty() || !empty {
        parts.push(interface.body.span.source_text(source));
    }
    let text = format!(
        "{head}{name} = {}{}",
        parts.join(" & "),
        Layout::detect(source, program).terminator()
    );
    Some(Action::new(
        TO_TYPE,
        TYPE,
        vec![Edit {
            range: layout::with_terminator(source, interface.span),
            text,
        }],
    ))
}

fn merged(interface: &TSInterfaceDeclaration, ancestors: &[AstKind]) -> bool {
    let name = interface.id.name.as_str();
    let Some(list) = ancestors.iter().rev().find_map(|kind| statements(*kind)) else {
        return false;
    };
    list.iter()
        .filter(|statement| merges(statement) && names::declared(statement).contains(&name))
        .count()
        > 1
}

fn merges(statement: &Statement) -> bool {
    let declaration = match statement {
        Statement::ExportDeclaration(export) => Some(&export.declaration),
        other => other.as_declaration(),
    };
    matches!(
        declaration,
        Some(Declaration::TSInterfaceDeclaration(_) | Declaration::ClassDeclaration(_))
    )
}
