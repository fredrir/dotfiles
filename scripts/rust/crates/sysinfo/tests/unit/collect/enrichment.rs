use super::*;
use serde_json::json;

fn collected_kinds() -> Vec<&'static str> {
    let mut collected = Vec::new();
    common::collect(&mut collected);
    platform_collect(&mut collected);
    collected.into_iter().map(|(kind, _)| kind).collect()
}

fn requested_kinds() -> Vec<String> {
    enrichment_request()
        .iter()
        .map(|module| module_kind(module).unwrap_or_default().to_string())
        .collect()
}

#[test]
fn declared_native_kinds_are_always_collected() {
    let collected = collected_kinds();
    for kind in native_kinds() {
        assert!(
            collected.contains(&kind),
            "{kind} is declared native but no collector produced it, so enrichment would skip it too"
        );
    }
}

#[test]
fn enrichment_never_requests_owned_or_sampled_kinds() {
    let requested = requested_kinds();
    assert!(requested.contains(&"Host".to_string()), "{requested:?}");
    assert!(requested.contains(&"Display".to_string()), "{requested:?}");
    for kind in native_kinds() {
        assert!(
            !requested.iter().any(|requested| requested == kind),
            "{kind} is collected natively and must not be requested from enrichment"
        );
    }
    assert!(
        !requested.iter().any(|kind| kind == "CPUUsage"),
        "CPU load is sampled in process: {requested:?}"
    );
}

#[test]
fn enrichment_fills_gaps_without_overriding_native_results() {
    let mut modules = index(vec![("WM", json!({"prettyName": "native"}))]);
    merge_enrichment(
        &mut modules,
        vec![
            json!({"type": "WM", "result": {"prettyName": "enriched"}}),
            json!({"type": "Host", "result": {"name": "enriched"}}),
        ],
    );
    assert_eq!(modules["WM"]["prettyName"], json!("native"));
    assert_eq!(modules["Host"]["name"], json!("enriched"));
}

#[test]
fn scopes_only_ask_for_what_their_view_renders() {
    for (scope, enrich, identity, cpu) in [
        (Scope::Dashboard, false, false, true),
        (Scope::Summary, false, true, false),
        (Scope::Full, true, true, true),
    ] {
        assert_eq!(scope.enriches(), enrich, "{scope:?}");
        assert_eq!(scope.probes_identity(), identity, "{scope:?}");
        assert_eq!(scope.samples_cpu(), cpu, "{scope:?}");
    }
    assert_eq!(Scope::from_full(true), Scope::Full);
    assert_eq!(Scope::from_full(false), Scope::Summary);
}
