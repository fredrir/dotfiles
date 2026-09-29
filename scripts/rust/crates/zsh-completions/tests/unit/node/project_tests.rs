use super::*;
use testkit::tree;

const MANIFEST: &str = r#"{
  "name": "root",
  "scripts": {"build": "tsc -b", "test": "vitest", "lint": ["not", "a", "string"]},
  "dependencies": {"react": "^19.0.0"},
  "devDependencies": {"typescript": "~5.9.0", "react": "^18"},
  "peerDependencies": {"react-dom": "*"},
  "workspaces": ["packages/*", "!packages/skip"]
}"#;

#[test]
fn dependencies_keep_their_kind_and_first_range() {
    let root = tree(&[&format!("package.json={MANIFEST}")]);
    let manifest = Manifest::read(&root.path().join("package.json")).expect("manifest");
    let found: Vec<(String, String, &str)> = manifest
        .dependencies()
        .into_iter()
        .map(|dependency| (dependency.name, dependency.range, dependency.kind))
        .collect();
    assert_eq!(
        found,
        [
            ("react".into(), "^19.0.0".into(), ""),
            ("typescript".into(), "~5.9.0".into(), "dev"),
            ("react-dom".into(), "*".into(), "peer"),
        ]
    );
}

#[test]
fn scripts_keep_their_order_and_tolerate_odd_values() {
    let root = tree(&[&format!("package.json={MANIFEST}")]);
    let manifest = Manifest::read(&root.path().join("package.json")).expect("manifest");
    let names: Vec<String> = manifest
        .scripts()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["build", "test", "lint"]);
}

#[test]
fn the_nearest_manifest_is_used() {
    let root = tree(&[
        &format!("package.json={MANIFEST}"),
        "packages/web/package.json={\"name\":\"web\"}",
        "packages/web/src/",
    ]);
    let ctx = Context::testing(root.path(), &root.path().join("packages/web/src"), &[]);
    assert_eq!(
        nearest(&ctx).and_then(|manifest| manifest.name).as_deref(),
        Some("web")
    );
}

#[test]
fn workspaces_expand_globs_and_honour_exclusions() {
    let root = tree(&[
        &format!("package.json={MANIFEST}"),
        "packages/web/package.json={\"name\":\"@acme/web\"}",
        "packages/api/package.json={\"name\":\"@acme/api\"}",
        "packages/skip/package.json={\"name\":\"skipped\"}",
        "packages/empty/",
        "packages/web/node_modules/dep/package.json={\"name\":\"dep\"}",
    ]);
    let ctx = Context::testing(root.path(), &root.path().join("packages/web"), &[]);
    let mut found = workspaces(&ctx);
    found.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(
        found,
        [
            Workspace {
                name: "@acme/api".into(),
                path: "packages/api".into()
            },
            Workspace {
                name: "@acme/web".into(),
                path: "packages/web".into()
            },
        ]
    );
}

#[test]
fn pnpm_workspace_files_are_read_too() {
    let root = tree(&[
        "package.json={\"name\":\"root\"}",
        "pnpm-workspace.yaml=packages:\n  - 'apps/*'\n",
        "apps/site/package.json={\"name\":\"site\"}",
    ]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    let names: Vec<String> = workspaces(&ctx)
        .into_iter()
        .map(|workspace| workspace.name)
        .collect();
    assert_eq!(names, ["site"]);
}

#[test]
fn local_binaries_come_from_every_enclosing_node_modules() {
    let root = tree(&[
        "node_modules/.bin/tsc=",
        "node_modules/.bin/.hidden=",
        "app/node_modules/.bin/vite=",
        "app/node_modules/.bin/tsc=",
    ]);
    let ctx = Context::testing(root.path(), &root.path().join("app"), &[]);
    assert_eq!(local_binaries(&ctx), ["tsc", "vite"]);
}

#[test]
fn npm_globals_are_listed_from_the_configured_prefix() {
    let root = tree(&[
        "home/.npmrc=prefix=~/global",
        "home/global/lib/node_modules/typescript/package.json={\"version\":\"5.9.2\"}",
        "home/global/lib/node_modules/@scope/tool/package.json={\"version\":\"1.0.0\"}",
    ]);
    let home = root.path().join("home");
    let ctx = Context::testing(&home, &home, &[]);
    let found: Vec<(String, String)> = global_packages(&ctx, Manager::Npm)
        .into_iter()
        .map(|dependency| (dependency.name, dependency.range))
        .collect();
    assert_eq!(
        found,
        [
            ("@scope/tool".into(), "1.0.0".into()),
            ("typescript".into(), "5.9.2".into()),
        ]
    );
}

#[test]
fn bun_and_yarn_globals_come_from_their_manifests() {
    let root = tree(&[
        "home/.bun/install/global/package.json={\"dependencies\":{\"cowsay\":\"^1\"}}",
        "home/.config/yarn/global/package.json={\"dependencies\":{\"serve\":\"^14\"}}",
    ]);
    let home = root.path().join("home");
    let ctx = Context::testing(&home, &home, &[]);
    let name = |manager| {
        global_packages(&ctx, manager)
            .into_iter()
            .map(|dependency| dependency.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(name(Manager::Bun), ["cowsay"]);
    assert_eq!(name(Manager::Yarn), ["serve"]);
}
