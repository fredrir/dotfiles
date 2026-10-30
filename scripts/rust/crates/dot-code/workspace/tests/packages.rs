#![forbid(unsafe_code)]

use dot_code_workspace::packages::{AGENTS, Params, Plan, plan};
use serde_json::json;
use testkit::{TempDir, tree_pairs};

fn detect(root: &TempDir, from: &str) -> Option<Plan> {
    plan(&Params {
        path: root.path().join(from),
        agent: None,
    })
}

fn install(root: &TempDir, cwd: &str, agent: &'static str) -> Option<Plan> {
    Some(Plan {
        cwd: root.path().join(cwd),
        command: Some([agent, "install"]),
        choices: &[],
    })
}

#[test]
fn workspaces_install_from_the_lockfile_root() {
    let root = tree_pairs(&[
        ("bun.lock", ""),
        ("package.json", "{}"),
        ("packages/a/package.json", "{}"),
        ("packages/a/src/", ""),
    ]);
    assert_eq!(detect(&root, "packages/a/src"), install(&root, "", "bun"));
}

#[test]
fn the_package_manager_field_overrides_the_lockfile() {
    let root = tree_pairs(&[
        ("yarn.lock", ""),
        (
            "package.json",
            r#"{ "packageManager": "^pnpm@9.1.0+sha512.abc" }"#,
        ),
    ]);
    assert_eq!(detect(&root, ""), install(&root, "", "pnpm"));
}

#[test]
fn manifest_fields_work_without_a_lockfile() {
    let root = tree_pairs(&[
        ("yarn/package.json", r#"{ "packageManager": "yarn@4.5.0" }"#),
        (
            "bun/package.json",
            r#"{ "devEngines": { "packageManager": { "name": "bun" } } }"#,
        ),
    ]);
    assert_eq!(detect(&root, "yarn"), install(&root, "yarn", "yarn"));
    assert_eq!(detect(&root, "bun"), install(&root, "bun", "bun"));
}

#[test]
fn unknown_package_managers_fall_back_to_the_lockfile() {
    let root = tree_pairs(&[
        ("package-lock.json", ""),
        ("package.json", r#"{ "packageManager": "nope@1.0.0" }"#),
    ]);
    assert_eq!(detect(&root, ""), install(&root, "", "npm"));
}

#[test]
fn undetected_projects_ask_at_the_nearest_manifest() {
    let root = tree_pairs(&[
        ("package.json", "{}"),
        ("pkg/package.json", "not json"),
        ("pkg/src/", ""),
    ]);
    let asked = detect(&root, "pkg/src").expect("a manifest exists");
    assert_eq!(asked.cwd, root.path().join("pkg"));
    assert_eq!(asked.command, None);
    assert_eq!(asked.choices, AGENTS);

    let picked = |agent: &str| {
        plan(&Params {
            path: root.path().join("pkg/src"),
            agent: Some(agent.into()),
        })
    };
    assert_eq!(picked("deno"), install(&root, "pkg", "deno"));
    assert_eq!(picked("nope"), None);
}

#[test]
fn folders_without_a_manifest_have_no_plan() {
    let root = tree_pairs(&[("src/", "")]);
    assert_eq!(detect(&root, "src"), None);
}

#[test]
fn plans_serialize_to_the_extension_contract() {
    let root = tree_pairs(&[("bun/bun.lock", ""), ("plain/package.json", "{}")]);
    let at = |dir: &str| root.path().join(dir).display().to_string();
    let serialized = |dir: &str| serde_json::to_value(detect(&root, dir)).unwrap();
    assert_eq!(
        serialized("bun"),
        json!({ "cwd": at("bun"), "command": ["bun", "install"] })
    );
    assert_eq!(
        serialized("plain"),
        json!({ "cwd": at("plain"), "choices": AGENTS })
    );
}
