use std::collections::HashSet;

use oxc_ast::ast::{
    BindingIdentifier, Declaration, IdentifierReference, Program, Statement, TSTypeName,
    TSTypeQuery, TSTypeQueryExprName, TSTypeReference,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::Span;

/// Every identifier the program declares or references.
pub fn taken<'a>(program: &Program<'a>) -> HashSet<&'a str> {
    let mut collector = Taken(HashSet::new());
    collector.visit_program(program);
    collector.0
}

struct Taken<'a>(HashSet<&'a str>);

impl<'a> Visit<'a> for Taken<'a> {
    fn visit_binding_identifier(&mut self, it: &BindingIdentifier<'a>) {
        self.0.insert(it.name.as_str());
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        self.0.insert(it.name.as_str());
    }
}

/// Root names of the type references and `typeof` queries inside `span`.
pub fn referenced<'a>(program: &Program<'a>, span: Span) -> HashSet<&'a str> {
    let mut collector = Referenced {
        span,
        names: HashSet::new(),
    };
    collector.visit_program(program);
    collector.names
}

struct Referenced<'a> {
    span: Span,
    names: HashSet<&'a str>,
}

impl Referenced<'_> {
    fn inside(&self, span: Span) -> bool {
        self.span.start <= span.start && span.end <= self.span.end
    }
}

impl<'a> Visit<'a> for Referenced<'a> {
    fn visit_ts_type_reference(&mut self, it: &TSTypeReference<'a>) {
        if self.inside(it.span)
            && let Some(name) = root(&it.type_name)
        {
            self.names.insert(name);
        }
        walk::walk_ts_type_reference(self, it);
    }

    fn visit_ts_type_query(&mut self, it: &TSTypeQuery<'a>) {
        if self.inside(it.span)
            && let Some(name) = match &it.expr_name {
                TSTypeQueryExprName::TSImportType(_) => None,
                other => other.as_ts_type_name().and_then(root),
            }
        {
            self.names.insert(name);
        }
        walk::walk_ts_type_query(self, it);
    }
}

pub fn root<'a>(name: &TSTypeName<'a>) -> Option<&'a str> {
    match name {
        TSTypeName::IdentifierReference(identifier) => Some(identifier.name.as_str()),
        TSTypeName::QualifiedName(qualified) => root(&qualified.left),
        TSTypeName::ThisExpression(_) => None,
    }
}

/// Names a statement brings into its scope.
pub fn declared<'a>(statement: &Statement<'a>) -> Vec<&'a str> {
    match statement {
        Statement::ExportDeclaration(export) => from_declaration(&export.declaration),
        other => other
            .as_declaration()
            .map_or_else(Vec::new, from_declaration),
    }
}

fn from_declaration<'a>(declaration: &Declaration<'a>) -> Vec<&'a str> {
    match declaration {
        Declaration::VariableDeclaration(variables) => variables
            .declarations
            .iter()
            .filter_map(|declarator| declarator.id.get_identifier_name())
            .map(|name| name.as_str())
            .collect(),
        Declaration::FunctionDeclaration(function) => {
            function.id.iter().map(|id| id.name.as_str()).collect()
        }
        Declaration::ClassDeclaration(class) => {
            class.id.iter().map(|id| id.name.as_str()).collect()
        }
        Declaration::TSTypeAliasDeclaration(alias) => vec![alias.id.name.as_str()],
        Declaration::TSInterfaceDeclaration(interface) => vec![interface.id.name.as_str()],
        Declaration::TSEnumDeclaration(enumeration) => vec![enumeration.id.name.as_str()],
        _ => Vec::new(),
    }
}
