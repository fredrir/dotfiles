use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
        .canonicalize()
        .unwrap()
}

fn load_json(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'), "{}", path.display());
    serde_json::from_slice(&bytes).unwrap()
}

fn must_tokens(source: &str) -> Vec<(u64, u64, String)> {
    let mut found = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        let line_number = line_index as u64 + 1;
        if line_number == 16 {
            continue;
        }
        let mut ordinal = 0;
        for (start, _) in line.match_indices("MUST") {
            let before = line[..start].chars().next_back();
            let after = line[start + 4..].chars().next();
            if before.is_some_and(|value| value.is_ascii_alphanumeric() || value == '_') {
                continue;
            }
            if after.is_some_and(|value| value.is_ascii_alphanumeric() || value == '_') {
                continue;
            }
            ordinal += 1;
            let keyword = if line[start..].starts_with("MUST NOT") {
                "MUST NOT"
            } else {
                "MUST"
            };
            found.push((line_number, ordinal, keyword.to_owned()));
        }
    }
    found
}

#[test]
fn every_must_has_an_owner_and_test_plan() {
    let root = repository_root();
    let source = fs::read_to_string(root.join("docs/dotfile-language.md")).unwrap();
    let manifest = load_json(&root.join("contracts/dotfile/v1/traceability.json"));
    let versions = load_json(&root.join("contracts/dotfile/v1/versions.json"));
    let diagnostics = load_json(&root.join("contracts/dotfile/v1/diagnostics.json"));
    let rules = manifest["rules"].as_array().unwrap();
    let actual = must_tokens(&source);
    let version_owners = versions["traceability_version_owners"].as_object().unwrap();
    let diagnostic_stages = diagnostics["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|stage| stage["name"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let stage_groups = versions["traceability_stage_groups"].as_object().unwrap();
    for stage in diagnostics["stages"].as_array().unwrap() {
        let name = stage["name"].as_str().unwrap();
        let owner = stage["version_owner"].as_str().unwrap();
        assert!(versions["tuple"].get(owner).is_some(), "{owner}");
        assert!(
            versions["diagnostic_stage_owners"][owner]
                .as_array()
                .unwrap()
                .iter()
                .any(|candidate| candidate == name)
        );
    }
    for group in stage_groups.values() {
        assert!(
            group["members"]
                .as_array()
                .unwrap()
                .iter()
                .all(|member| { diagnostic_stages.contains(member.as_str().unwrap()) })
        );
    }
    let owner_packages = BTreeSet::from([
        "dotfile-analysis",
        "dotfile-apply",
        "dotfile-bind",
        "dotfile-cli",
        "dotfile-deploy",
        "dotfile-lock",
        "dotfile-repo",
        "dotfile-schema",
        "dotfile-semantics",
        "dotfile-source",
        "dotfile-syntax",
        "dotfile-test-support",
        "dotfile-theme",
    ]);
    for rule in rules {
        let version_owner = rule["version_owner"].as_str().unwrap();
        let owner = &version_owners[version_owner];
        assert!(owner.is_object(), "{version_owner}");
        for (component, version) in owner["components"].as_object().unwrap() {
            assert_eq!(versions["tuple"][component], *version, "{version_owner}");
        }
        let stage = rule["stage"].as_str().unwrap();
        assert!(
            diagnostic_stages.contains(stage) || stage_groups.contains_key(stage),
            "{stage}"
        );
        assert!(owner_packages.contains(rule["owner_package"].as_str().unwrap()));
        for milestone in rule["milestone"].as_str().unwrap().split('/') {
            let number = milestone.strip_prefix('M').unwrap().parse::<u8>().unwrap();
            assert!(number <= 14);
        }
    }
    let declared = rules
        .iter()
        .map(|rule| {
            assert!(!rule["id"].as_str().unwrap().is_empty());
            assert!(!rule["owner_package"].as_str().unwrap().is_empty());
            assert!(!rule["milestone"].as_str().unwrap().is_empty());
            assert!(!rule["test_plans"].as_array().unwrap().is_empty());
            (
                rule["source"]["line"].as_u64().unwrap(),
                rule["source"]["token_ordinal"].as_u64().unwrap(),
                rule["source"]["keyword"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(actual.len(), 77);
    assert_eq!(declared, actual);
}
