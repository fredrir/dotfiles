use super::*;
use testkit::tree;

fn hit(name: &str) -> Hit {
    downloaded(name, 0)
}

fn downloaded(name: &str, downloads: u64) -> Hit {
    Hit {
        name: name.into(),
        description: String::new(),
        version: String::new(),
        downloads,
    }
}

fn names(hits: Vec<Hit>) -> Vec<String> {
    hits.into_iter().map(|hit| hit.name).collect()
}

#[test]
fn versions_split_after_the_scope() {
    assert_eq!(split_version("react@19"), Some(("react", "19")));
    assert_eq!(split_version("react@"), Some(("react", "")));
    assert_eq!(split_version("@types/node@2"), Some(("@types/node", "2")));
    assert_eq!(split_version("@types/node"), None);
    assert_eq!(split_version("@types"), None);
    assert_eq!(split_version("react"), None);
}

#[test]
fn versions_sort_newest_first_with_latest_leading_the_tags() {
    let tags = BTreeMap::from([
        ("beta".to_string(), "2.0.0-beta.1".to_string()),
        ("latest".to_string(), "1.10.0".to_string()),
    ]);
    let versions = from_packument(
        tags,
        ["1.2.0", "1.10.0", "2.0.0-beta.1", "0.9.0", "not-semver"]
            .map(String::from)
            .to_vec(),
    );
    assert_eq!(versions.tags[0].0, "latest");
    assert_eq!(
        versions.versions,
        ["2.0.0-beta.1", "1.10.0", "1.2.0", "0.9.0", "not-semver"]
    );
}

#[test]
fn an_empty_prefix_searches_nothing() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    assert!(search(&ctx, DEFAULT_REGISTRY, "").is_empty());
}

#[test]
fn a_stored_search_answers_without_the_network() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    cache::store(
        &ctx,
        &search_key("https://private.example", "rea"),
        &vec![hit("react")],
    );
    assert_eq!(
        search(&ctx, "https://private.example", "rea"),
        [hit("react")]
    );
}

#[test]
fn offline_a_longer_prefix_filters_what_a_shorter_one_found() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    let found = vec![hit("react"), hit("readdirp"), hit("@types/react-dom")];
    cache::store(&ctx, &search_key("https://private.example", "rea"), &found);
    assert_eq!(
        names(search(&ctx, "https://private.example", "reac")),
        ["react", "@types/react-dom"]
    );
}

#[test]
fn name_relevance_precedes_downloads_and_description_only_hits_are_excluded() {
    let hits = vec![
        downloaded("claude", 79_000),
        downloaded("@anthropic-ai/claude-code", 50_000_000),
        downloaded("@steerable/agent-ui", 90_000_000),
        downloaded("Claude-Cup", 23_000_000),
    ];
    assert_eq!(
        names(rank(hits, "claude")),
        ["claude", "Claude-Cup", "@anthropic-ai/claude-code"]
    );
}

#[test]
fn exact_and_scoped_names_beat_popular_prefixes_and_substrings() {
    let hits = vec![
        downloaded("unrelated", 9_000_000),
        downloaded("my-shadcn-tools", 1_000_000),
        downloaded("shadcn-ui", 100_000),
        downloaded("@acme/shadcn", 10_000),
        downloaded("shadcn", 1),
        downloaded("shadcn-extra", 200_000),
    ];
    assert_eq!(
        names(rank(hits, "SHADCN")),
        [
            "shadcn",
            "@acme/shadcn",
            "shadcn-extra",
            "shadcn-ui",
            "my-shadcn-tools",
        ]
    );
}

#[test]
fn cached_results_are_reranked_and_filtered_too() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    cache::store(
        &ctx,
        &search_key(DEFAULT_REGISTRY, "shadcn"),
        &vec![
            downloaded("shadcn-ui", 1000),
            downloaded("typescript", 1_000_000),
            hit("shadcn"),
        ],
    );
    assert_eq!(
        names(search(&ctx, DEFAULT_REGISTRY, "shadcn")),
        ["shadcn", "shadcn-ui"]
    );
}

#[test]
fn registries_are_cached_apart() {
    assert_ne!(
        search_key("https://registry.npmjs.org", "x"),
        search_key("https://npm.example.com", "x")
    );
    assert_eq!(host("https://npm.example.com/"), "npm.example.com");
}

#[test]
fn the_initial_picker_uses_stale_results_without_contacting_the_registry() {
    let root = tree(&[]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let registry = format!("http://{}", listener.local_addr().unwrap());
    let mut ctx = Context::testing(root.path(), root.path(), &[]);
    ctx.offline = false;
    ctx.cached_packages = true;
    cache::store(
        &ctx,
        &search_key(&registry, "sha"),
        &vec![hit("shadcn"), hit("sharp")],
    );
    ctx.now += Duration::from_secs(SEARCH_TTL + 1);
    assert_eq!(names(search(&ctx, &registry, "shad")), ["shadcn"]);
    assert!(search(&ctx, &registry, "missing").is_empty());
    assert!(versions(&ctx, &registry, "missing").is_none());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn an_outage_reuses_the_same_query_even_after_its_cache_expires() {
    let root = tree(&[]);
    let mut ctx = Context::testing(root.path(), root.path(), &[]);
    cache::store(
        &ctx,
        &search_key(DEFAULT_REGISTRY, "shadcn"),
        &vec![hit("shadcn")],
    );
    ctx.now += Duration::from_secs(SEARCH_TTL + 1);
    assert_eq!(names(search(&ctx, DEFAULT_REGISTRY, "shadcn")), ["shadcn"]);
}

#[test]
fn the_initial_picker_filters_popular_names_without_starting_bun() {
    let root = tree(&["bin/"]);
    let bin = root.path().join("bin");
    testkit::executable(&bin.join("bun"), "#!/bin/sh\n : > started\n");
    let mut ctx = Context::testing(root.path(), root.path(), &[("PATH", bin.to_str().unwrap())]);
    ctx.cached_packages = true;
    let popular = super::super::popular::Popular {
        libraries: vec![hit("shadcn"), hit("typescript")],
        tools: Vec::new(),
    };
    cache::store(&ctx, "node-popular", &popular);
    let found = names(search(&ctx, DEFAULT_REGISTRY, "sha"));
    assert!(found.iter().any(|name| name == "shadcn"));
    assert!(found.iter().all(|name| name.contains("sha")));
    assert!(search(&ctx, DEFAULT_REGISTRY, "missing").is_empty());
    assert!(!root.path().join("started").exists());
}

#[test]
fn prefix_suggestions_extend_word_search_without_losing_its_download_counts() {
    let found = merge(
        Some(vec![downloaded("react", 500), downloaded("react-dom", 200)]),
        Some(vec![hit("react"), hit("react-router")]),
    )
    .unwrap();
    assert_eq!(
        found,
        [
            downloaded("react", 500),
            downloaded("react-dom", 200),
            hit("react-router")
        ]
    );
    assert_eq!(merge(None, None), None);
}

#[test]
fn common_prefixes_work_with_an_empty_cache_and_no_network() {
    let root = tree(&[]);
    let mut ctx = Context::testing(root.path(), root.path(), &[]);
    ctx.cached_packages = true;
    for (prefix, expected) in [
        ("pret", "prettier"),
        ("prett", "prettier"),
        ("pretti", "prettier"),
        ("typesc", "typescript"),
        ("reac", "react"),
        ("shad", "shadcn"),
    ] {
        let found = names(search(&ctx, DEFAULT_REGISTRY, prefix));
        assert_eq!(
            found.first().map(String::as_str),
            Some(expected),
            "{prefix}: {found:?}"
        );
    }
}

#[test]
fn a_poor_longer_query_cannot_displace_known_common_packages() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    cache::store(
        &ctx,
        &search_key(DEFAULT_REGISTRY, "pret"),
        &vec![hit("prettier"), hit("pretty-error")],
    );
    cache::store(
        &ctx,
        &search_key(DEFAULT_REGISTRY, "prett"),
        &vec![
            downloaded("prettier-simple-logger", 221),
            downloaded("prettier-logo", 69),
            downloaded("prettier-detect", 26),
            downloaded("@yuhr/prettier", 361),
        ],
    );
    let found = names(search(&ctx, DEFAULT_REGISTRY, "prett"));
    assert_eq!(found.first().map(String::as_str), Some("prettier"));
    assert!(found.iter().any(|name| name == "pretty-error"));
}

#[test]
fn a_partial_exact_cache_does_not_hide_a_matching_shorter_query() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    let registry = "https://private.example";
    cache::store(&ctx, &search_key(registry, "foo"), &vec![hit("foobar")]);
    cache::store(
        &ctx,
        &search_key(registry, "foob"),
        &vec![hit("foob-extra")],
    );
    let found = names(search(&ctx, registry, "foob"));
    assert!(found.iter().any(|name| name == "foobar"));
    assert!(found.iter().any(|name| name == "foob-extra"));
    assert!(
        search(&ctx, registry, "prett").is_empty(),
        "public catalog must not leak into private registries"
    );
}

#[test]
fn names_from_the_catalog_pick_up_real_metadata_without_invented_counts() {
    let seed = hit("prettier");
    let mut live = downloaded("prettier", 500_000_000);
    live.description = "Code formatter".into();
    live.version = "3.0.0".into();
    let result = merge(Some(vec![seed]), Some(vec![live.clone()])).unwrap();
    assert_eq!(result, [live]);
}
