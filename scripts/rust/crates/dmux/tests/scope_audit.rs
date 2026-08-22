//! Audit of the one hatch in the scope boundary (ADR 012 WS-A.3).
//!
//! `InventoryScope::unmanaged_endpoint` builds a scope nothing in the registry
//! vouches for. Reads under it are unverified, so every call site in `src/`
//! must say why it is legitimate, and the set of such sites is held here the
//! way `the_wrapper_verb_allowlist_matches_the_cli` holds the wrapper verbs:
//! evaluated against the tree, not maintained by habit. There is no CI; the
//! suite is the gate.
//!
//! A site declares itself with a marker comment on the call line or the line
//! above it:
//!
//! ```text
//! // audit(unmanaged_endpoint): <reason>
//! ```
//!
//! The allowlist below pairs each file with the exact reason text. Adding a
//! call without a marker, with an unlisted reason, or removing a listed one
//! fails this test and names the line. Entries marked as WS-A.5/WS-A.7
//! burn-down are the review's laundering sites, migrated one commit at a
//! time to `backend::scope::resolve_managed`; each migration deletes its
//! entry here, so the burn-down is visible in the diff and the list ends at
//! the one legitimate production site (`ls_cli` first-contact tmux) plus the two test helpers.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MARKER: &str = "audit(unmanaged_endpoint):";
const CALL: &str = "InventoryScope::unmanaged_endpoint(";

/// (file relative to the crate, reason). Multiplicity matters.
const ALLOWLIST: &[(&str, &str)] = &[
    (
        "src/ls_cli.rs",
        "first-contact tmux namespace; nothing is registered for this backend",
    ),
    ("src/backend/tmux.rs", "test-only scope(Option) helper"),
    ("src/backend/wez.rs", "test-only scope(Option) helper"),
    (
        "src/gui_cli.rs",
        "WS-A.5 burn-down: opposite-backend create target launders a NULL epoch (finding #15)",
    ),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn reason_on(line: &str) -> Option<String> {
    let at = line.find(MARKER)?;
    Some(line[at + MARKER.len()..].trim().to_string())
}

#[test]
fn every_unmanaged_endpoint_site_is_named_in_the_allowlist() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&crate_root.join("src"), &mut files);
    files.sort();

    let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut unmarked = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(crate_root)
            .expect("under crate root")
            .to_string_lossy()
            .into_owned();
        if rel == "src/backend/scope.rs" {
            continue; // the definition
        }
        let text = fs::read_to_string(path).expect("read source");
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.contains(CALL) {
                continue;
            }
            let reason = reason_on(line).or_else(|| {
                lines[..i]
                    .iter()
                    .rev()
                    .take_while(|l| l.trim_start().starts_with("//"))
                    .find_map(|l| reason_on(l))
            });
            match reason {
                Some(reason) => *found.entry((rel.clone(), reason)).or_default() += 1,
                None => unmarked.push(format!("{rel}:{}", i + 1)),
            }
        }
    }

    let mut expected: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (file, reason) in ALLOWLIST {
        *expected
            .entry((file.to_string(), reason.to_string()))
            .or_default() += 1;
    }

    let missing: Vec<String> = expected
        .iter()
        .filter(|(k, n)| found.get(*k).copied().unwrap_or(0) < **n)
        .map(|((f, r), n)| format!("{f} x{n}: {r}"))
        .collect();
    let extra: Vec<String> = found
        .iter()
        .filter(|(k, n)| expected.get(*k).copied().unwrap_or(0) < **n)
        .map(|((f, r), n)| format!("{f} x{n}: {r}"))
        .collect();

    assert!(
        unmarked.is_empty() && missing.is_empty() && extra.is_empty(),
        "the unmanaged_endpoint hatch drifted from its allowlist\n  \
         calls with no `// {MARKER} <reason>` marker: {unmarked:?}\n  \
         listed but not found (stale entry — delete it): {missing:?}\n  \
         found but not listed (new hatch — justify it or use resolve_managed): {extra:?}"
    );
}
