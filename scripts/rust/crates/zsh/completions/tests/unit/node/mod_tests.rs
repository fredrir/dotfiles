use super::*;
use crate::cache;
use testkit::tree;

const MANIFEST: &str = r#"{
  "name": "root",
  "scripts": {"build": "tsc -b", "test": "vitest"},
  "dependencies": {"react": "^19.0.0"},
  "devDependencies": {"typescript": "~5.9.0"},
  "workspaces": ["packages/*"]
}"#;

fn project() -> testkit::TempDir {
    tree(&[
        &format!("package.json={MANIFEST}"),
        "packages/web/package.json={\"name\":\"@acme/web\"}",
        "node_modules/.bin/vitest=",
        "home/.config/yarn/global/package.json={\"dependencies\":{\"serve\":\"^14\"}}",
    ])
}

fn completed(root: &testkit::TempDir, command: &str, line: &str) -> Reply {
    let ctx = Context::testing(&root.path().join("home"), root.path(), &[]);
    let words: Vec<String> = line.split(' ').map(String::from).collect();
    let current = words.len();
    let line = Line::new(words, current, None);
    let (manager, runner) = Manager::from_command(command).expect("a node command");
    complete(&ctx, &line, manager, runner)
}

#[test]
fn run_offers_the_scripts_with_their_commands() {
    let root = project();
    let reply = completed(&root, "npm", "npm run ");
    assert_eq!(reply.values(), ["build", "test"]);
    assert!(reply.plain().contains("item\tbuild\tbuild  tsc -b"));
}

#[test]
fn removing_offers_the_declared_dependencies() {
    let root = project();
    let reply = completed(&root, "pnpm", "pnpm rm ");
    assert_eq!(reply.values(), ["react", "typescript"]);
    assert!(
        reply
            .plain()
            .contains("item\ttypescript\ttypescript  ~5.9.0   dev")
    );
}

#[test]
fn yarn_global_commands_work_on_the_global_packages() {
    let root = project();
    assert_eq!(
        completed(&root, "yarn", "yarn global remove ").values(),
        ["serve"]
    );
    assert!(
        completed(&root, "yarn", "yarn global ")
            .values()
            .contains(&"add")
    );
}

#[test]
fn a_version_after_the_name_comes_from_the_registry_answer() {
    let root = project();
    let ctx = Context::testing(&root.path().join("home"), root.path(), &[]);
    let versions = registry::from_packument(
        [("latest".to_string(), "19.1.0".to_string())].into(),
        vec!["18.3.1".into(), "19.1.0".into()],
    );
    cache::store(&ctx, "versions-registry.npmjs.org-react", &versions);
    let reply = completed(&root, "bun", "bun add react@");
    let rendered = reply.plain();
    assert!(rendered.starts_with("skip\treact@\n"), "{rendered}");
    assert_eq!(reply.values(), ["latest", "19.1.0", "18.3.1"]);
    assert!(
        rendered.contains("item\t19.1.0\t19.1.0  latest"),
        "{rendered}"
    );
}

#[test]
fn adding_offers_registry_hits_that_replace_the_word() {
    let root = project();
    let ctx = Context::testing(&root.path().join("home"), root.path(), &[]);
    let hits = vec![registry::Hit {
        name: "@anthropic-ai/claude-code".into(),
        description: "Claude in the terminal".into(),
        version: String::new(),
        downloads: 50_195_456,
    }];
    cache::store(&ctx, "search-registry.npmjs.org-claude", &hits);
    let rendered = completed(&root, "npm", "npm i -g claude").plain();
    assert_eq!(
        rendered,
        "group\tregistry\tregistry package\tlines\tunsorted\treplace\n\
         item\t@anthropic-ai/claude-code\t@anthropic-ai/claude-code  50M/mo  Claude in the terminal\n"
    );
}

#[test]
fn a_bare_word_lists_popular_commands_globally_and_libraries_locally() {
    let root = project();
    let ctx = Context::testing(&root.path().join("home"), root.path(), &[]);
    let hit = |name: &str| registry::Hit {
        name: name.into(),
        description: String::new(),
        version: String::new(),
        downloads: 0,
    };
    let popular = popular::Popular {
        libraries: vec![hit("chalk")],
        tools: vec![hit("typescript")],
    };
    cache::store(&ctx, "node-popular", &popular);
    assert_eq!(
        completed(&root, "npm", "npm i -g ").values(),
        ["typescript"]
    );
    assert_eq!(completed(&root, "npm", "npm i ").values(), ["chalk"]);
    assert_eq!(
        completed(&root, "npx", "npx ").values(),
        ["vitest", "typescript"]
    );
}

#[test]
fn a_bare_word_says_so_while_the_popular_list_is_fetched() {
    let root = project();
    let rendered = completed(&root, "npm", "npm i -g ").plain();
    assert!(rendered.starts_with("message\tfetching"), "{rendered}");
}

#[test]
fn download_counts_read_compactly() {
    assert_eq!(compact(390), "390");
    assert_eq!(compact(79_400), "79K");
    assert_eq!(compact(50_195_456), "50M");
    assert_eq!(compact(1_200_000_000), "1B");
}

#[test]
fn paths_complete_as_files() {
    let root = project();
    assert_eq!(completed(&root, "npm", "npm i ./").plain(), "files\n");
}

#[test]
fn workspace_flags_offer_the_workspaces() {
    let root = project();
    let reply = completed(&root, "pnpm", "pnpm add --filter=");
    assert_eq!(reply.values(), ["@acme/web"]);
    assert!(reply.plain().starts_with("skip\t--filter=\n"));
}

#[test]
fn runners_offer_local_binaries_first() {
    let root = project();
    let reply = completed(&root, "npx", "npx ");
    assert_eq!(reply.values().first(), Some(&"vitest"));
    assert_eq!(completed(&root, "npx", "npx vitest ").plain(), "files\n");
}

#[test]
fn the_first_word_is_left_to_the_tool_s_own_completion() {
    let root = project();
    let rendered = completed(&root, "npm", "npm ").plain();
    assert!(rendered.starts_with("delegate\n"), "{rendered}");
    assert!(
        rendered.contains("item\tbuild\t"),
        "scripts back up a missing completer"
    );
}

#[test]
fn unknown_subcommands_are_delegated_with_files_as_the_fallback() {
    let root = project();
    assert_eq!(
        completed(&root, "npm", "npm publish ").plain(),
        "delegate\nfiles\n"
    );
}
