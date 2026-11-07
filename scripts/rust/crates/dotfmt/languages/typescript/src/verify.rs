use std::collections::HashMap;

use dotfmt_tailwind::Sorter;
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    CallExpression, Expression, JSXAttribute, JSXAttributeName, JSXExpression,
    JSXExpressionContainer, JSXText, ObjectProperty, PropertyKey, StringLiteral,
};
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};

pub fn detect_code_removal(
    before: &str,
    after: &str,
    source_type: SourceType,
    tailwind: Option<&Sorter>,
) -> Option<String> {
    let removal = oxc_formatter::detect_code_removal(before, after, source_type)?;

    if let Some(sorter) = tailwind
        && let (Some(before), Some(after)) = (
            normalize_class_strings(before, source_type, sorter),
            normalize_class_strings(after, source_type, sorter),
        )
    {
        match oxc_formatter::detect_code_removal(&before, &after, source_type) {
            None => return None,
            Some(normalized) if !normalized.contains("Parse error") => {
                if !is_known_false_positive(&normalized) {
                    return Some(removal);
                }
                let before_signature = Signature::collect(&before, source_type)?;
                let after_signature = Signature::collect(&after, source_type)?;
                return (before_signature != after_signature).then_some(removal);
            }
            Some(_) => {}
        }
    }

    if !is_known_false_positive(&removal) {
        return Some(removal);
    }
    let before_signature = Signature::collect(before, source_type)?;
    let after_signature = Signature::collect(after, source_type)?;
    (before_signature != after_signature).then_some(removal)
}

/// Rewrites string literals through the Tailwind sorter so the removal guard
/// compares class lists after the same reordering, deduplication and merging
/// that formatting applies. Non-class strings are left untouched.
fn normalize_class_strings(
    source: &str,
    source_type: SourceType,
    sorter: &Sorter,
) -> Option<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let mut visitor = NormalizeVisitor {
        source,
        sorter,
        depth: 0,
        cache: HashMap::new(),
        replacements: Vec::new(),
    };
    visitor.visit_program(&parsed.program);
    if visitor.replacements.is_empty() {
        return Some(source.to_owned());
    }
    visitor
        .replacements
        .sort_unstable_by_key(|(start, _, _)| *start);
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    for (start, end, replacement) in visitor.replacements {
        if start < cursor {
            continue;
        }
        output.push_str(&source[cursor..start]);
        output.push_str(&replacement);
        cursor = end;
    }
    output.push_str(&source[cursor..]);
    Some(output)
}

struct NormalizeVisitor<'s> {
    source: &'s str,
    sorter: &'s Sorter,
    depth: usize,
    cache: HashMap<String, String>,
    replacements: Vec<(usize, usize, String)>,
}

impl<'a> Visit<'a> for NormalizeVisitor<'_> {
    fn visit_jsx_attribute(&mut self, attribute: &JSXAttribute<'a>) {
        let tailwind = is_class_attribute(&attribute.name, self.sorter.config());
        if tailwind {
            self.depth += 1;
        }
        oxc_ast_visit::walk::walk_jsx_attribute(self, attribute);
        if tailwind {
            self.depth -= 1;
        }
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        let tailwind = is_tailwind_function(&call.callee, self.sorter.config());
        if tailwind {
            self.depth += 1;
        }
        oxc_ast_visit::walk::walk_call_expression(self, call);
        if tailwind {
            self.depth -= 1;
        }
    }

    fn visit_string_literal(&mut self, literal: &StringLiteral<'a>) {
        if self.depth == 0 {
            return;
        }
        let Some(value) = literal.value.as_str() else {
            return;
        };
        if !value
            .chars()
            .any(|character| character.is_ascii_whitespace())
        {
            return;
        }
        let normalized = if let Some(normalized) = self.cache.get(value) {
            normalized.clone()
        } else {
            let normalized = self.sorter.sort_list(value);
            self.cache.insert(value.to_owned(), normalized.clone());
            normalized
        };
        if normalized == value {
            return;
        }
        let start = literal.span.start as usize;
        let end = literal.span.end as usize;
        let Some(original) = self.source.get(start..end) else {
            return;
        };
        if let Some(replacement) = requote(original, &normalized) {
            self.replacements.push((start, end, replacement));
        }
    }
}

fn is_class_attribute(name: &JSXAttributeName<'_>, config: &dotfmt_tailwind::Config) -> bool {
    let JSXAttributeName::Identifier(identifier) = name else {
        return false;
    };
    let name = identifier.name.as_str();
    name == "class"
        || name == "className"
        || config.attributes.iter().any(|attribute| attribute == name)
}

fn is_tailwind_function(callee: &Expression<'_>, config: &dotfmt_tailwind::Config) -> bool {
    if config.functions.is_empty() {
        return false;
    }
    let mut node = callee;
    loop {
        match node {
            Expression::CallExpression(call) => node = &call.callee,
            Expression::StaticMemberExpression(member) => node = &member.object,
            Expression::ComputedMemberExpression(member) => node = &member.object,
            Expression::Identifier(identifier) => {
                return config
                    .functions
                    .iter()
                    .any(|function| function == identifier.name.as_str());
            }
            _ => return false,
        }
    }
}

fn requote(original: &str, value: &str) -> Option<String> {
    let quote = original.chars().next()?;
    if !matches!(quote, '\'' | '"') || original.len() < 2 || !original.ends_with(quote) {
        return None;
    }
    let mut output = String::with_capacity(value.len() + 2);
    output.push(quote);
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character == quote => {
                output.push('\\');
                output.push(character);
            }
            character if character.is_control() => {
                use std::fmt::Write as _;
                write!(output, "\\u{{{:x}}}", character as u32).ok()?;
            }
            character => output.push(character),
        }
    }
    output.push(quote);
    Some(output)
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
    use std::sync::Arc;

    use dotfmt_tailwind::{Config as TailwindConfig, OrderTable, Sorter};
    use oxc_span::SourceType;

    use super::detect_code_removal;

    fn tsx() -> SourceType {
        SourceType::tsx()
    }

    fn merge_sorter() -> Sorter {
        let table = OrderTable::from_pairs([
            ("m-2".to_owned(), Some(0)),
            ("p-4".to_owned(), Some(1)),
            ("p-2".to_owned(), Some(2)),
            ("text-sm".to_owned(), Some(3)),
            ("flex".to_owned(), Some(4)),
        ]);
        Sorter::new(
            Arc::new(table),
            TailwindConfig {
                functions: vec!["cn".to_owned(), "clsx".to_owned(), "cva".to_owned()],
                merge: true,
                dedupe: true,
                ..TailwindConfig::default()
            },
        )
    }

    #[test]
    fn accepts_folded_jsx_space() {
        assert_eq!(
            detect_code_removal(
                "export const X = <span>a{\" \"}b</span>;\n",
                "export const X = <span>a b</span>;\n",
                tsx(),
                None,
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
                None,
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
                None,
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
                None,
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
                None,
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
                None,
            )
            .is_some()
        );
    }

    #[test]
    fn accepts_tailwind_merge_of_jsx_class_lists() {
        let sorter = merge_sorter();
        assert_eq!(
            detect_code_removal(
                "export const A=()=> <div className=\"p-2 p-4\"/>\n",
                "export const A = () => <div className=\"p-4\" />;\n",
                tsx(),
                Some(&sorter),
            ),
            None
        );
    }

    #[test]
    fn accepts_tailwind_merge_of_string_and_object_values() {
        let sorter = merge_sorter();
        assert_eq!(
            detect_code_removal(
                "const x = cn({ \"size\": \"p-2 p-4\" });\n",
                "const x = cn({ size: \"p-4\" });\n",
                tsx(),
                Some(&sorter),
            ),
            None
        );
        assert_eq!(
            detect_code_removal(
                "const button = cva(\"p-2 p-4\", { variants: { size: { sm: \"p-2 p-4\" } } });\n",
                "const button = cva(\"p-4\", { variants: { size: { sm: \"p-4\" } } });\n",
                tsx(),
                Some(&sorter),
            ),
            None
        );
    }

    #[test]
    fn rejects_real_string_loss_even_with_tailwind_merge() {
        let sorter = merge_sorter();
        assert!(
            detect_code_removal(
                "const x = \"hello world\";\n",
                "const x = \"hello\";\n",
                tsx(),
                Some(&sorter),
            )
            .is_some()
        );
        assert!(
            detect_code_removal(
                "export const A = <div className=\"p-2 p-4\" />;\n",
                "export const A = <div />;\n",
                tsx(),
                Some(&sorter),
            )
            .is_some()
        );
    }
}
