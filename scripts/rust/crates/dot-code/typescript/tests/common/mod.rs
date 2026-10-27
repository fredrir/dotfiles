#![allow(dead_code)]

use std::ops::Range;

use dot_code_core::{Action, Feature, Request, Trigger, apply};
use dot_code_typescript::TypeScript;
use serde_json::{Value, json};

const CURSOR: char = '‸';
const OPEN: char = '«';
const CLOSE: char = '»';

/// Strips a `‸` cursor or a `«…»` selection from `marked`.
pub fn unmark(marked: &str) -> (String, Range<usize>) {
    if let Some(cursor) = marked.find(CURSOR) {
        return (marked.replacen(CURSOR, "", 1), cursor..cursor);
    }
    let start = marked.find(OPEN).expect("cursor or selection marker");
    let end = marked.find(CLOSE).expect("selection end") - OPEN.len_utf8();
    (
        marked.replacen(OPEN, "", 1).replacen(CLOSE, "", 1),
        start..end,
    )
}

pub fn actions(
    marked: &str,
    trigger: Trigger,
    only: &[&str],
    settings: Value,
) -> (String, Vec<Action>) {
    let (source, selection) = unmark(marked);
    let only: Vec<String> = only.iter().map(|kind| kind.to_string()).collect();
    let mut feature = TypeScript::default();
    feature.configure(settings).expect("valid settings");
    let actions = feature.code_actions(&Request {
        source: &source,
        language: "typescriptreact",
        selection,
        trigger,
        only: &only,
    });
    (source, actions)
}

pub struct Applied {
    pub text: String,
    pub renamed: Option<String>,
}

/// Runs the single enabled action of `kind`, as a keybinding would.
pub fn run_with(marked: &str, kind: &str, settings: Value) -> Applied {
    let (source, actions) = actions(marked, Trigger::Invoked, &[kind], settings);
    let action = single(&actions, kind);
    let text = apply(&source, &action.edits);
    let renamed = action.rename.map(|offset| identifier_at(&text, offset));
    Applied { text, renamed }
}

pub fn run(marked: &str, kind: &str) -> Applied {
    run_with(marked, kind, json!({}))
}

pub fn single<'a>(actions: &'a [Action], kind: &str) -> &'a Action {
    let matching: Vec<&Action> = actions
        .iter()
        .filter(|action| action.kind == kind)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected one {kind} action in {actions:#?}"
    );
    let action = matching[0];
    assert_eq!(action.disabled, None, "{kind} is disabled");
    action
}

pub fn kinds(actions: &[Action]) -> Vec<&'static str> {
    actions.iter().map(|action| action.kind).collect()
}

fn identifier_at(text: &str, offset: usize) -> String {
    text[offset..]
        .chars()
        .take_while(|character| {
            character.is_alphanumeric() || *character == '_' || *character == '$'
        })
        .collect()
}
