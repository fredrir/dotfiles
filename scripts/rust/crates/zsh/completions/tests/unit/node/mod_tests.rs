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
    assert!(rendered.starts_with("search\nskip\treact@\n"), "{rendered}");
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
    assert!(
        rendered
            .starts_with("search\ngroup\tregistry\tregistry package\tlines\tunsorted\treplace\n")
    );
    let row = rendered
        .lines()
        .find(|line| line.starts_with("item\t@anthropic-ai/claude-code\t"))
        .unwrap();
    assert!(row.contains("50M/mo"));
    assert!(row.ends_with("Claude in the terminal"));
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
fn a_bare_word_can_search_before_the_popular_list_is_ready() {
    let root = project();
    let rendered = completed(&root, "npm", "npm i -g ").plain();
    assert_eq!(rendered, "search\nmessage\ttype a package name to search\n");
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

fn bun_reply(ctx: &Context, text: &str) -> Reply {
    let mut spec = Spec::fallback(Manager::Bun);
    spec.top = Help::parse(include_str!("../../fixtures/bun.txt"), &["bun"]);
    spec.roles[0].help = Help::parse(include_str!("../../fixtures/bun-add.txt"), &["bun", "add"]);
    let words: Vec<String> = text.split(' ').map(String::from).collect();
    let line = Line::new(words.clone(), words.len(), None);
    complete_with_spec(ctx, &line, Manager::Bun, false, &spec)
}

#[test]
fn bun_install_dev_flags_complete_packages_instead_of_define_values() {
    let root = project();
    let ctx = Context::testing(&root.path().join("home"), root.path(), &[]);
    cache::store(
        &ctx,
        "search-registry.npmjs.org-shadcn",
        &vec![registry::Hit {
            name: "shadcn".into(),
            description: String::new(),
            version: String::new(),
            downloads: 1,
        }],
    );
    for command in ["i", "install", "add", "a"] {
        for flag in ["-d", "--dev", "-g"] {
            let text = format!("bun {command} {flag} shadcn");
            let reply = bun_reply(&ctx, &text);
            assert_eq!(reply.values(), ["shadcn"], "{text}: {}", reply.plain());
        }
        let reply = bun_reply(&ctx, &format!("bun {command} -d "));
        assert!(reply.plain().starts_with("search\n"), "{}", reply.plain());
        assert!(!reply.plain().contains("define"));
    }
}

#[test]
fn bun_flags_before_and_after_the_command_use_the_correct_help() {
    let root = project();
    let ctx = Context::testing(&root.path().join("home"), root.path(), &[]);
    assert!(bun_reply(&ctx, "bun -d ").plain().contains("--define"));
    assert!(
        bun_reply(&ctx, "bun i --backend ")
            .values()
            .contains(&"hardlink")
    );
    assert!(
        bun_reply(&ctx, "bun --cwd i i -d ")
            .plain()
            .starts_with("search\n")
    );
}

#[test]
fn every_manager_offers_the_same_ranked_package_names() {
    let root = project();
    let ctx = Context::testing(&root.path().join("home"), root.path(), &[]);
    let hits: Vec<registry::Hit> = [("shadcn-ui", 100), ("shadcn", 1), ("typescript", 1000)]
        .into_iter()
        .map(|(name, downloads)| registry::Hit {
            name: name.into(),
            downloads,
            description: String::new(),
            version: String::new(),
        })
        .collect();
    cache::store(&ctx, "search-registry.npmjs.org-shadcn", &hits);
    for (command, action) in [("bun", "i"), ("npm", "i"), ("pnpm", "add"), ("yarn", "add")] {
        assert_eq!(
            completed(&root, command, &format!("{command} {action} shadcn")).values(),
            ["shadcn", "shadcn-ui"]
        );
    }
}
