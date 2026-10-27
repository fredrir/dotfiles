use std::collections::HashSet;

use dot_code_core::{Action, Edit, Request, Trigger};
use oxc_ast::AstKind;
use oxc_ast::ast::{Program, Statement, TSType, TSTypeLiteral, TSTypeParameterDeclaration};
use oxc_span::{GetSpan, Span};

use crate::layout::{self, Layout};
use crate::names;
use crate::path::statements;
use crate::settings::{Export, Extract};

// Sub-kinds keep these apart from the built-in TypeScript extract refactors.
pub const INTERFACE: &str = "refactor.extract.interface.dotCode";
pub const TYPE: &str = "refactor.extract.type.dotCode";

pub fn actions<'a>(
    program: &Program<'a>,
    path: &[AstKind<'a>],
    request: &Request,
    settings: &Extract,
) -> Vec<Action> {
    let interface = request.wants(INTERFACE);
    let alias = request.wants(TYPE);
    let empty = request.selection.is_empty();
    if !(interface || alias) || (empty && request.trigger == Trigger::Automatic) {
        return Vec::new();
    }
    let Some((target, ancestors)) = target(path, empty) else {
        return Vec::new();
    };
    let Some(extraction) = Extraction::plan(program, target, ancestors, request.source, settings)
    else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    if interface && let AstKind::TSTypeLiteral(literal) = target {
        actions.push(extraction.action(Declaration::Interface(literal)));
    }
    if alias {
        actions.push(extraction.action(Declaration::Alias));
    }
    actions
}

/// The type to extract and its ancestors: the innermost object type around a cursor,
/// the innermost type around a selection, or the object type of the parameter under the cursor.
fn target<'p, 'a>(
    path: &'p [AstKind<'a>],
    empty: bool,
) -> Option<(AstKind<'a>, &'p [AstKind<'a>])> {
    let found = if empty {
        path.iter()
            .rposition(|kind| matches!(kind, AstKind::TSTypeLiteral(_)))
    } else {
        path.iter().rposition(|kind| is_type(*kind))
    };
    if let Some(index) = found {
        let ancestors = &path[..index];
        let whole_alias = matches!(
            ancestors.last(),
            Some(AstKind::TSTypeAliasDeclaration(alias)) if alias.type_annotation.span() == path[index].span()
        );
        return (!whole_alias).then_some((path[index], ancestors));
    }
    if !empty {
        return None;
    }
    let index = path
        .iter()
        .rposition(|kind| matches!(kind, AstKind::FormalParameter(_)))?;
    let AstKind::FormalParameter(parameter) = path[index] else {
        return None;
    };
    match &parameter.type_annotation.as_ref()?.type_annotation {
        TSType::TSTypeLiteral(literal) => Some((AstKind::TSTypeLiteral(literal), &path[..=index])),
        _ => None,
    }
}

fn is_type(kind: AstKind) -> bool {
    matches!(
        kind,
        AstKind::TSAnyKeyword(_)
            | AstKind::TSBigIntKeyword(_)
            | AstKind::TSBooleanKeyword(_)
            | AstKind::TSIntrinsicKeyword(_)
            | AstKind::TSNeverKeyword(_)
            | AstKind::TSNullKeyword(_)
            | AstKind::TSNumberKeyword(_)
            | AstKind::TSObjectKeyword(_)
            | AstKind::TSStringKeyword(_)
            | AstKind::TSSymbolKeyword(_)
            | AstKind::TSUndefinedKeyword(_)
            | AstKind::TSUnknownKeyword(_)
            | AstKind::TSVoidKeyword(_)
            | AstKind::TSArrayType(_)
            | AstKind::TSConditionalType(_)
            | AstKind::TSConstructorType(_)
            | AstKind::TSFunctionType(_)
            | AstKind::TSImportType(_)
            | AstKind::TSIndexedAccessType(_)
            | AstKind::TSIntersectionType(_)
            | AstKind::TSLiteralType(_)
            | AstKind::TSMappedType(_)
            | AstKind::TSParenthesizedType(_)
            | AstKind::TSTemplateLiteralType(_)
            | AstKind::TSTupleType(_)
            | AstKind::TSTypeLiteral(_)
            | AstKind::TSTypeOperator(_)
            | AstKind::TSTypeQuery(_)
            | AstKind::TSTypeReference(_)
            | AstKind::TSUnionType(_)
    )
}

enum Declaration<'a> {
    Interface(&'a TSTypeLiteral<'a>),
    Alias,
}

struct Extraction<'s> {
    source: &'s str,
    layout: Layout,
    target: Span,
    insert_at: usize,
    at_line_start: bool,
    indent: &'s str,
    base: &'s str,
    verbatim: bool,
    export: bool,
    name: String,
    parameters: Vec<String>,
    arguments: Vec<String>,
    rename: bool,
}

impl<'s> Extraction<'s> {
    fn plan<'a>(
        program: &Program<'a>,
        target: AstKind<'a>,
        ancestors: &[AstKind<'a>],
        source: &'s str,
        settings: &Extract,
    ) -> Option<Self> {
        let span = target.span();
        let referenced = names::referenced(program, span);
        let containers: Vec<(usize, &[Statement])> = ancestors
            .iter()
            .enumerate()
            .filter_map(|(index, kind)| statements(*kind).map(|list| (index, list)))
            .collect();
        // Innermost scope declaring something the type uses, else the module.
        let &(level, list) = containers
            .iter()
            .rev()
            .find(|(_, list)| {
                list.iter().any(|statement| {
                    names::declared(statement)
                        .iter()
                        .any(|name| referenced.contains(name))
                })
            })
            .or_else(|| containers.first())?;
        let anchor = list.iter().find(|statement| {
            let statement = statement.span();
            statement.start <= span.start && span.end <= statement.end
        })?;

        let mut parameters = Vec::new();
        let mut arguments = Vec::new();
        for kind in &ancestors[level + 1..] {
            for (name, constraint) in type_parameters(*kind) {
                if referenced.contains(name) && !arguments.iter().any(|known| known == name) {
                    arguments.push(name.to_string());
                    parameters.push(match constraint {
                        Some(constraint) => {
                            format!("{name} extends {}", constraint.source_text(source))
                        }
                        None => name.to_string(),
                    });
                }
            }
        }

        let start = leading_comments_start(program, source, anchor.span());
        let line = layout::line_start(source, start);
        let at_line_start = source[line..start].trim().is_empty();
        let insert_at = if at_line_start { line } else { start };

        let exported = matches!(
            anchor,
            Statement::ExportDeclaration(_) | Statement::ExportDefaultDeclaration(_)
        );
        let exportable = matches!(
            ancestors[level],
            AstKind::Program(_) | AstKind::TSModuleBlock(_)
        );
        let export = exportable
            && match settings.export {
                Export::Never => false,
                Export::Always => true,
                Export::Inherit => exported,
            };

        Some(Self {
            source,
            layout: Layout::detect(source, program),
            target: span,
            insert_at,
            at_line_start,
            indent: layout::indentation(source, start),
            base: layout::indentation(source, span.end as usize - 1),
            verbatim: layout::has_comment(program, span),
            export,
            name: name(&settings.name, ancestors, &names::taken(program)),
            parameters,
            arguments,
            rename: settings.rename,
        })
    }

    fn action(&self, declaration: Declaration) -> Action {
        let generics = if self.parameters.is_empty() {
            String::new()
        } else {
            format!("<{}>", self.parameters.join(", "))
        };
        let export = if self.export { "export " } else { "" };
        let (title, kind, keyword, body) = match declaration {
            Declaration::Interface(literal) => (
                "Extract to interface",
                INTERFACE,
                "interface ",
                format!(" {}", self.interface_body(literal)),
            ),
            Declaration::Alias => (
                "Extract to type alias",
                TYPE,
                "type ",
                format!(
                    " = {}{}",
                    layout::reindent(self.target.source_text(self.source), self.base, self.indent),
                    self.layout.terminator()
                ),
            ),
        };
        let declaration = format!("{export}{keyword}{}{generics}{body}", self.name);
        let newline = self.layout.newline;
        let (text, lead) = if self.at_line_start {
            (
                format!("{}{declaration}{newline}{newline}", self.indent),
                self.indent.len(),
            )
        } else {
            (format!("{declaration}{newline}{newline}{}", self.indent), 0)
        };
        let reference = if self.arguments.is_empty() {
            self.name.clone()
        } else {
            format!("{}<{}>", self.name, self.arguments.join(", "))
        };
        let edits = vec![
            Edit::insert(self.insert_at, text),
            Edit {
                range: self.target.start as usize..self.target.end as usize,
                text: reference,
            },
        ];
        Action {
            rename: self
                .rename
                .then_some(self.insert_at + lead + export.len() + keyword.len()),
            ..Action::new(title, kind, edits)
        }
    }

    fn interface_body(&self, literal: &TSTypeLiteral) -> String {
        let text = literal.span.source_text(self.source);
        if self.verbatim || literal.members.is_empty() || text.contains('\n') {
            return layout::reindent(text, self.base, self.indent);
        }
        let newline = self.layout.newline;
        let mut body = String::from("{");
        for member in &literal.members {
            body.push_str(newline);
            body.push_str(self.indent);
            body.push_str(&self.layout.unit);
            body.push_str(
                member
                    .span()
                    .source_text(self.source)
                    .trim_end_matches([';', ','])
                    .trim_end(),
            );
            body.push_str(self.layout.terminator());
        }
        body.push_str(newline);
        body.push_str(self.indent);
        body.push('}');
        body
    }
}

/// Where a statement starts once the comments directly above it are counted in.
fn leading_comments_start(program: &Program, source: &str, statement: Span) -> usize {
    let mut comments: Vec<Span> = program
        .comments
        .iter()
        .filter(|comment| comment.is_leading() && comment.attached_to == statement.start)
        .map(|comment| comment.span)
        .collect();
    comments.sort_by_key(|comment| std::cmp::Reverse(comment.start));
    let mut start = statement.start as usize;
    for comment in comments {
        let gap = &source[comment.end as usize..start];
        if !gap.trim().is_empty() || gap.matches('\n').count() > 1 {
            break;
        }
        start = comment.start as usize;
    }
    start
}

fn type_parameters<'a>(kind: AstKind<'a>) -> Vec<(&'a str, Option<Span>)> {
    let declaration: Option<&TSTypeParameterDeclaration> = match kind {
        AstKind::TSMappedType(mapped) => {
            return vec![(mapped.key.name.as_str(), Some(mapped.constraint.span()))];
        }
        AstKind::Function(it) => it.type_parameters.as_deref(),
        AstKind::ArrowFunctionExpression(it) => it.type_parameters.as_deref(),
        AstKind::Class(it) => it.type_parameters.as_deref(),
        AstKind::TSTypeAliasDeclaration(it) => it.type_parameters.as_deref(),
        AstKind::TSInterfaceDeclaration(it) => it.type_parameters.as_deref(),
        AstKind::TSMethodSignature(it) => it.type_parameters.as_deref(),
        AstKind::TSCallSignatureDeclaration(it) => it.type_parameters.as_deref(),
        AstKind::TSConstructSignatureDeclaration(it) => it.type_parameters.as_deref(),
        AstKind::TSFunctionType(it) => it.type_parameters.as_deref(),
        AstKind::TSConstructorType(it) => it.type_parameters.as_deref(),
        _ => None,
    };
    declaration.map_or_else(Vec::new, |declaration| {
        declaration
            .params
            .iter()
            .map(|parameter| {
                (
                    parameter.name.name.as_str(),
                    parameter.constraint.as_ref().map(GetSpan::span),
                )
            })
            .collect()
    })
}

fn name(template: &str, ancestors: &[AstKind], taken: &HashSet<&str>) -> String {
    let owner = ancestors
        .iter()
        .rev()
        .find_map(|kind| owner(*kind))
        .map(capitalize)
        .unwrap_or_default();
    let base = template.replace("${name}", &owner);
    let base = if is_identifier(&base) {
        base
    } else {
        "NewType".to_string()
    };
    if !taken.contains(base.as_str()) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}{suffix}");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn owner<'a>(kind: AstKind<'a>) -> Option<&'a str> {
    match kind {
        AstKind::Function(function) => function.id.as_ref().map(|id| id.name.as_str()),
        AstKind::Class(class) => class.id.as_ref().map(|id| id.name.as_str()),
        AstKind::VariableDeclarator(declarator) => declarator
            .id
            .get_identifier_name()
            .map(|name| name.as_str()),
        AstKind::TSTypeAliasDeclaration(alias) => Some(alias.id.name.as_str()),
        AstKind::TSInterfaceDeclaration(interface) => Some(interface.id.name.as_str()),
        _ => None,
    }
}

fn capitalize(name: &str) -> String {
    let mut characters = name.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

fn is_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_' || first == '$')
        && characters.all(|rest| rest.is_alphanumeric() || rest == '_' || rest == '$')
}
