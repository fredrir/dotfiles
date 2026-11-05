use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Expression, JSXExpression, JSXExpressionContainer, JSXText, ObjectProperty, PropertyKey,
};
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};

pub fn detect_code_removal(before: &str, after: &str, source_type: SourceType) -> Option<String> {
    let removal = oxc_formatter::detect_code_removal(before, after, source_type)?;
    if !is_known_false_positive(&removal) {
        return Some(removal);
    }
    let before_signature = Signature::collect(before, source_type)?;
    let after_signature = Signature::collect(after, source_type)?;
    (before_signature != after_signature).then_some(removal)
}

fn is_known_false_positive(removal: &str) -> bool {
    removal.lines().all(|line| {
        line.contains("JSX_TEXT(")
            || line.contains("OBJECT_LIKE_KEYS(")
            || line.contains("ObjectProperty(")
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    jsx_words: Vec<String>,
    properties: Vec<(String, String)>,
}

impl Signature {
    fn collect(source: &str, source_type: SourceType) -> Option<Self> {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, source_type).parse();
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut visitor = SignatureVisitor {
            source,
            ..SignatureVisitor::default()
        };
        visitor.visit_program(&parsed.program);
        let mut properties = visitor.properties;
        properties.sort();
        let jsx_words = visitor
            .jsx_words
            .join(" ")
            .split_whitespace()
            .map(std::borrow::ToOwned::to_owned)
            .collect();
        Some(Self {
            jsx_words,
            properties,
        })
    }
}

#[derive(Default)]
struct SignatureVisitor<'s> {
    source: &'s str,
    jsx_words: Vec<String>,
    properties: Vec<(String, String)>,
}

impl<'a> Visit<'a> for SignatureVisitor<'_> {
    fn visit_jsx_text(&mut self, text: &JSXText<'a>) {
        self.jsx_words.extend(
            text.value
                .split_whitespace()
                .map(std::borrow::ToOwned::to_owned),
        );
    }

    fn visit_jsx_expression_container(&mut self, container: &JSXExpressionContainer<'a>) {
        if matches!(
            &container.expression,
            JSXExpression::StringLiteral(value) if value.value == " "
        ) {
            self.jsx_words.push(" ".to_owned());
        }
        oxc_ast_visit::walk::walk_jsx_expression_container(self, container);
    }

    fn visit_object_property(&mut self, property: &ObjectProperty<'a>) {
        self.properties.push((
            key_text(&property.key, self.source),
            value_text(&property.value, self.source),
        ));
        oxc_ast_visit::walk::walk_object_property(self, property);
    }
}

fn key_text(key: &PropertyKey<'_>, source: &str) -> String {
    match key {
        PropertyKey::StaticIdentifier(identifier) => identifier.name.to_string(),
        PropertyKey::PrivateIdentifier(identifier) => {
            format!("#{}", identifier.name)
        }
        PropertyKey::StringLiteral(value) => normalized_words(value.value.as_str().unwrap_or("")),
        _ => source_text(key.span(), source),
    }
}

fn value_text(value: &Expression<'_>, source: &str) -> String {
    match value {
        Expression::StringLiteral(value) => normalized_words(value.value.as_str().unwrap_or("")),
        Expression::NumericLiteral(value) => value
            .raw
            .as_ref()
            .map_or_else(String::new, |raw| raw.as_str().to_owned()),
        Expression::BooleanLiteral(value) => value.value.to_string(),
        Expression::NullLiteral(_) => "null".to_owned(),
        Expression::ObjectExpression(_) => "<object>".to_owned(),
        Expression::ArrayExpression(_) => "<array>".to_owned(),
        Expression::ArrowFunctionExpression(_) => "<arrow>".to_owned(),
        Expression::FunctionExpression(_) => "<function>".to_owned(),
        _ => {
            let text = source_text(value.span(), source);
            if text.is_empty() {
                "<expr>".to_owned()
            } else {
                text
            }
        }
    }
}

fn source_text(span: oxc_span::Span, source: &str) -> String {
    source
        .get(span.start as usize..span.end as usize)
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized_words(value: &str) -> String {
    let mut words: Vec<&str> = value.split_whitespace().collect();
    words.sort_unstable();
    words.dedup();
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use oxc_span::SourceType;

    use super::detect_code_removal;

    fn tsx() -> SourceType {
        SourceType::tsx()
    }

    #[test]
    fn accepts_folded_jsx_space() {
        assert_eq!(
            detect_code_removal(
                "export const X = <span>a{\" \"}b</span>;\n",
                "export const X = <span>a b</span>;\n",
                tsx(),
            ),
            None
        );
    }

    #[test]
    fn rejects_jsx_space_that_was_the_only_separator() {
        assert!(
            detect_code_removal(
                "export const X = <span>a{\" \"}b</span>;\n",
                "export const X = <span>ab</span>;\n",
                tsx(),
            )
            .is_some()
        );
    }

    #[test]
    fn accepts_sorted_object_value() {
        assert_eq!(
            detect_code_removal(
                "const x = { size: \"text-sm p-2\" };\n",
                "const x = { size: \"p-2 text-sm\" };\n",
                tsx(),
            ),
            None
        );
    }

    #[test]
    fn accepts_sorted_nested_object_value() {
        assert_eq!(
            detect_code_removal(
                "const button = cva(\"flex items-center\", { variants: { size: { sm: \"text-sm p-2\" } } });\n",
                "const button = cva(\"flex items-center\", {\n  variants: { size: { sm: \"p-2 text-sm\" } },\n});\n",
                tsx(),
            ),
            None
        );
    }

    #[test]
    fn rejects_removed_object_value() {
        assert!(
            detect_code_removal(
                "const x = { size: \"text-sm p-2\" };\n",
                "const x = {};\n",
                tsx(),
            )
            .is_some()
        );
    }

    #[test]
    fn rejects_removed_jsx_text() {
        assert!(
            detect_code_removal(
                "export const X = <span>hello</span>;\n",
                "export const X = <span></span>;\n",
                tsx(),
            )
            .is_some()
        );
    }
}
