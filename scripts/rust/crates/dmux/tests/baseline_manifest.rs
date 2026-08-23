//! Acceptance case 46 has an executable half (plan §20.1; ADR 012 WS-G.1):
//! every baseline test recorded in `docs/adr/dmux/baseline-tests.json`
//! still exists by name in this crate, or is named by a retirement entry
//! whose replacement tests exist. Deleting or renaming a baseline test
//! without a manifest entry fails here, naming the test. Whether the
//! surviving tests are green is the suite's own business; this guards the
//! accounting, not the outcome.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../docs/adr/dmux/baseline-tests.json"
);

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every `fn <name>(` declared anywhere in src/ and tests/.
fn declared_fns() -> BTreeSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("tests"), &mut files);
    let mut names = BTreeSet::new();
    for file in files {
        let text = fs::read_to_string(&file).expect("read source");
        for line in text.lines() {
            let trimmed = line.trim_start();
            let rest = trimmed
                .strip_prefix("pub fn ")
                .or_else(|| trimmed.strip_prefix("fn "))
                .or_else(|| trimmed.strip_prefix("pub(crate) fn "))
                .or_else(|| trimmed.strip_prefix("async fn "));
            if let Some(rest) = rest
                && let Some(end) = rest.find(['(', '<'])
            {
                names.insert(rest[..end].to_string());
            }
        }
    }
    names
}

/// `suite::module::tests::fn_name (assertion: …)` → `fn_name`.
fn fn_name(id: &str) -> &str {
    let id = id.split(" (").next().unwrap_or(id);
    id.rsplit("::").next().unwrap_or(id)
}

#[test]
fn every_baseline_test_exists_or_is_retired_with_existing_replacements() {
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(MANIFEST).expect("manifest readable"))
            .expect("manifest is JSON");
    let declared = declared_fns();

    let retirements = manifest["retirements"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let retired: BTreeSet<&str> = retirements
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();

    let mut missing = Vec::new();
    let mut total = 0usize;
    for (suite, body) in manifest["suites"].as_object().expect("suites") {
        for test in body["tests"].as_array().expect("tests") {
            let id = test["id"].as_str().expect("id");
            total += 1;
            let name = fn_name(id);
            let exists = declared.contains(name);
            let retired_here = retired.iter().any(|r| fn_name(r) == name);
            if !exists && !retired_here {
                missing.push(format!("{suite}: {id}"));
            }
        }
    }
    assert!(
        total >= 100,
        "the manifest lists {total} baseline tests; it recorded 116 at 039e2ee"
    );

    let mut bad_retirements = Vec::new();
    for entry in &retirements {
        let id = entry["id"].as_str().unwrap_or("<no id>");
        let rationale = entry["rationale"].as_str().unwrap_or("");
        if rationale.trim().is_empty() {
            bad_retirements.push(format!("{id}: no rationale"));
        }
        let replacements = entry["replacement_tests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for replacement in replacements {
            let rid = replacement.as_str().unwrap_or("<not a string>");
            if !declared.contains(fn_name(rid)) {
                bad_retirements.push(format!("{id}: replacement {rid} does not exist"));
            }
        }
    }

    assert!(
        missing.is_empty() && bad_retirements.is_empty(),
        "baseline accountability drifted (plan §20.1, case 46)\n  \
         baseline tests neither present nor retired: {missing:?}\n  \
         retirement entries with a missing rationale or replacement: {bad_retirements:?}"
    );
}
