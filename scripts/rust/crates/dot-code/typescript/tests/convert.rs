#![forbid(unsafe_code)]

mod common;

use common::{actions, kinds, run};
use dot_code_core::Trigger;
use dot_code_typescript::{CONVERT_TO_INTERFACE, CONVERT_TO_TYPE};
use serde_json::json;

#[test]
fn object_aliases_become_interfaces() {
    let applied = run(
        "type Pro‸ps = {\n  /** First. */\n  a: string;\n};\n",
        CONVERT_TO_INTERFACE,
    );
    assert_eq!(
        applied.text,
        "interface Props {\n  /** First. */\n  a: string;\n}\n"
    );
    assert_eq!(applied.renamed, None);
}

#[test]
fn intersections_become_heritage_clauses() {
    let applied = run(
        "export type Props<T> = Base<T> & ns.Extra & { a‸: T };\n",
        CONVERT_TO_INTERFACE,
    );
    assert_eq!(
        applied.text,
        "export interface Props<T> extends Base<T>, ns.Extra { a: T }\n"
    );
}

#[test]
fn ambient_aliases_keep_their_modifier() {
    let applied = run("declare type ‸Props = {};\n", CONVERT_TO_INTERFACE);
    assert_eq!(applied.text, "declare interface Props {}\n");
}

#[test]
fn interfaces_become_intersection_aliases() {
    let applied = run(
        "interface ‸Props extends A, B<string> {\n  a: string;\n}\n",
        CONVERT_TO_TYPE,
    );
    assert_eq!(
        applied.text,
        "type Props = A & B<string> & {\n  a: string;\n};\n"
    );
}

#[test]
fn empty_interfaces_alias_their_parents() {
    let applied = run("interface ‸Props extends A {}\n", CONVERT_TO_TYPE);
    assert_eq!(applied.text, "type Props = A;\n");
}

#[test]
fn converting_back_and_forth_round_trips() {
    let original = "export type Props = Base & {\n  a: string;\n};\n";
    let interface = run(
        &original.replacen("Props", "P‸rops", 1),
        CONVERT_TO_INTERFACE,
    )
    .text;
    let alias = run(&interface.replacen("Props", "P‸rops", 1), CONVERT_TO_TYPE).text;
    assert_eq!(alias, original);
}

#[test]
fn unions_explain_why_they_cannot_become_interfaces() {
    let marked = "type ‸Id = string | number;\n";
    let (_, explicit) = actions(marked, Trigger::Invoked, &[CONVERT_TO_INTERFACE], json!({}));
    assert_eq!(explicit.len(), 1);
    assert!(explicit[0].disabled.is_some());
    let (_, general) = actions(marked, Trigger::Invoked, &[], json!({}));
    assert!(general.is_empty());
}

#[test]
fn merged_and_augmenting_interfaces_stay_interfaces() {
    for marked in [
        "interface ‸Props { a: string }\ninterface Props { b: string }\n",
        "declare global {\n  interface ‸Window { a: string }\n}\n",
        "export default interface ‸Props { a: string }\n",
    ] {
        let (_, found) = actions(marked, Trigger::Invoked, &[CONVERT_TO_TYPE], json!({}));
        assert_eq!(found.len(), 1, "{marked}");
        assert!(found[0].disabled.is_some(), "{marked}");
    }
}

#[test]
fn the_lightbulb_only_shows_on_the_declaration_head() {
    let (_, head) = actions(
        "interface Pr‸ops { a: string }\n",
        Trigger::Automatic,
        &[],
        json!({}),
    );
    assert_eq!(kinds(&head), [CONVERT_TO_TYPE]);
    let (_, body) = actions(
        "interface Props { a: str‸ing }\n",
        Trigger::Automatic,
        &[],
        json!({}),
    );
    assert!(body.is_empty());
}
