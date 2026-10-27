#![forbid(unsafe_code)]

mod common;

use common::{actions, kinds, run, run_with};
use dot_code_core::Trigger;
use dot_code_typescript::{EXTRACT_INTERFACE, EXTRACT_TYPE};
use serde_json::json;

#[test]
fn inline_props_become_an_interface_and_rename_starts_on_its_name() {
    let applied = run(
        "export function Test({a,b}:{a:str‸ing,b:string}){\n  return a + b;\n}\n",
        EXTRACT_INTERFACE,
    );
    assert_eq!(
        applied.text,
        "interface Props {\n  a:string;\n  b:string;\n}\n\nexport function Test({a,b}:Props){\n  return a + b;\n}\n"
    );
    assert_eq!(applied.renamed.as_deref(), Some("Props"));
}

#[test]
fn the_cursor_on_a_destructured_parameter_extracts_its_type() {
    let applied = run(
        "export function Test({a‸,b}:{a:string,b:string}){}\n",
        EXTRACT_INTERFACE,
    );
    assert_eq!(
        applied.text,
        "interface Props {\n  a:string;\n  b:string;\n}\n\nexport function Test({a,b}:Props){}\n"
    );
}

#[test]
fn multiline_types_keep_their_comments_and_land_above_the_doc_comment() {
    let applied = run(
        "import { x } from \"x\";\n\n/** Renders. */\nexport function Test({\n  a,\n}: {\n  /** First. */\n  a‸: string;\n}) {}\n",
        EXTRACT_INTERFACE,
    );
    assert_eq!(
        applied.text,
        "import { x } from \"x\";\n\ninterface Props {\n  /** First. */\n  a: string;\n}\n\n/** Renders. */\nexport function Test({\n  a,\n}: Props) {}\n"
    );
}

#[test]
fn used_type_parameters_move_along_with_their_constraints() {
    let applied = run(
        "function pick<T extends string, U>(items: { list: T[‸] }, other: U) {}\n",
        EXTRACT_INTERFACE,
    );
    assert_eq!(
        applied.text,
        "interface Props<T extends string> {\n  list: T[];\n}\n\nfunction pick<T extends string, U>(items: Props<T>, other: U) {}\n"
    );
}

#[test]
fn types_using_local_declarations_stay_in_that_scope() {
    let applied = run(
        "function outer() {\n  type Local = string;\n  const inner = (value: { a: Lo‸cal }) => value;\n}\n",
        EXTRACT_INTERFACE,
    );
    assert_eq!(
        applied.text,
        "function outer() {\n  type Local = string;\n  interface Props {\n    a: Local;\n  }\n\n  const inner = (value: Props) => value;\n}\n"
    );
}

#[test]
fn taken_names_get_a_numeric_suffix() {
    let applied = run(
        "type Props = number;\nconst f = (p: { a‸: Props }) => p;\n",
        EXTRACT_INTERFACE,
    );
    assert!(
        applied
            .text
            .starts_with("type Props = number;\ninterface Props2 {")
    );
    assert!(applied.text.contains("(p: Props2)"));
}

#[test]
fn name_templates_and_export_inheritance_are_configurable() {
    let applied = run_with(
        "export const Card = ({ a }: { a‸: string }) => a;\n",
        EXTRACT_INTERFACE,
        json!({ "extract": { "name": "${name}Props", "export": "inherit", "rename": false } }),
    );
    assert_eq!(
        applied.text,
        "export interface CardProps {\n  a: string;\n}\n\nexport const Card = ({ a }: CardProps) => a;\n"
    );
    assert_eq!(applied.renamed, None);
}

#[test]
fn selected_types_become_type_aliases() {
    let applied = run("function f(value: «string | number») {}\n", EXTRACT_TYPE);
    assert_eq!(
        applied.text,
        "type Props = string | number;\n\nfunction f(value: Props) {}\n"
    );
    assert_eq!(applied.renamed.as_deref(), Some("Props"));
}

#[test]
fn semicolon_free_files_get_semicolon_free_declarations() {
    let applied = run(
        "const x = 1\nfunction f(value: { a: str‸ing }) {}\n",
        EXTRACT_TYPE,
    );
    assert_eq!(
        applied.text,
        "const x = 1\ntype Props = { a: string }\n\nfunction f(value: Props) {}\n"
    );
}

#[test]
fn the_lightbulb_needs_a_selection() {
    let cursor = "function f(value: { a: str‸ing }) {}\n";
    let (_, automatic) = actions(cursor, Trigger::Automatic, &[], json!({}));
    assert!(automatic.is_empty());
    let selection = "function f(value: «{ a: string }») {}\n";
    let (_, automatic) = actions(selection, Trigger::Automatic, &[], json!({}));
    assert_eq!(kinds(&automatic), [EXTRACT_INTERFACE, EXTRACT_TYPE]);
}

#[test]
fn an_alias_body_is_not_extracted_from_its_own_alias() {
    let (_, found) = actions(
        "type Props = { a: str‸ing };\n",
        Trigger::Invoked,
        &[EXTRACT_INTERFACE, EXTRACT_TYPE],
        json!({}),
    );
    assert!(found.is_empty());
}
