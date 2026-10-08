use super::*;
use testkit::tree;

#[test]
fn the_shim_registers_every_supported_command() {
    let shim = shim();
    assert!(!shim.contains("{{"), "every placeholder is filled");
    assert!(
        shim.contains("for command in npm npx pnpm pn pnpx pnx yarn bun bunx pi shadcn; do"),
        "{shim}"
    );
}

#[test]
fn unknown_commands_get_no_candidates() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    let line = Line::new(vec!["deno".into(), String::new()], 2, None);
    assert_eq!(complete(&ctx, "deno", &line).plain(), "");
}

#[test]
fn a_command_given_as_a_path_is_recognised() {
    let root = tree(&["package.json={\"scripts\":{\"dev\":\"vite\"}}"]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    let line = Line::new(vec!["npm".into(), "run".into(), String::new()], 3, None);
    assert!(
        complete(&ctx, "/usr/bin/npm", &line)
            .plain()
            .contains("item\tdev\tdev  vite")
    );
}

#[test]
fn refreshing_an_unknown_source_fails() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    assert!(!refresh(&ctx, "nothing", &[]));
    assert!(!refresh(&ctx, "node-spec", &["deno".into()]));
}
