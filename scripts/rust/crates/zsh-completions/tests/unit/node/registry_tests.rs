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
        &search_key(DEFAULT_REGISTRY, "rea"),
        &vec![hit("react")],
    );
    assert_eq!(search(&ctx, DEFAULT_REGISTRY, "rea"), [hit("react")]);
}

#[test]
fn offline_a_longer_prefix_filters_what_a_shorter_one_found() {
    let root = tree(&[]);
    let ctx = Context::testing(root.path(), root.path(), &[]);
    let found = vec![hit("react"), hit("readdirp"), hit("@types/react-dom")];
    cache::store(&ctx, &search_key(DEFAULT_REGISTRY, "rea"), &found);
    assert_eq!(
        names(search(&ctx, DEFAULT_REGISTRY, "reac")),
        ["react", "@types/react-dom"]
    );
}

#[test]
fn hits_are_ranked_by_downloads_and_must_name_the_text() {
    let hits = vec![
        downloaded("claude", 79_000),
        downloaded("@anthropic-ai/claude-code", 50_000_000),
        downloaded("@steerable/agent-ui", 90_000_000),
        downloaded("Claude-Cup", 23_000_000),
    ];
    assert_eq!(
        names(rank(hits, "claude")),
        ["@anthropic-ai/claude-code", "Claude-Cup", "claude"]
    );
}

#[test]
fn download_counts_are_read_from_bulk_and_single_answers() {
    let bulk = serde_json::json!({
        "react": {"downloads": 647, "package": "react"},
        "missing": null
    });
    assert_eq!(download_count(&bulk, "react"), Some(647));
    assert_eq!(download_count(&bulk, "missing"), None);
    let single = serde_json::json!({"downloads": 5, "package": "react"});
    assert_eq!(download_count(&single, "react"), Some(5));
    assert_eq!(download_count(&single, "vue"), None);
}

#[test]
fn merged_answers_keep_one_hit_per_name_with_the_best_count() {
    let merged = merge(
        Some(vec![downloaded("react", 0), downloaded("vue", 3)]),
        Some(vec![downloaded("react", 9)]),
    )
    .expect("an answer");
    assert_eq!(merged, [downloaded("react", 9), downloaded("vue", 3)]);
    assert_eq!(merge(None, None), None);
}

#[test]
fn registries_are_cached_apart() {
    assert_ne!(
        search_key("https://registry.npmjs.org", "x"),
        search_key("https://npm.example.com", "x")
    );
    assert_eq!(host("https://npm.example.com/"), "npm.example.com");
}
