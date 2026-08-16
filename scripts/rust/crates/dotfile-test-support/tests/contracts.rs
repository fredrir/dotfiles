use blake2::Blake2sVar;
use blake2::digest::{Update, VariableOutput};
use dotfile_test_support::{contract_directory, load_contract, representative_fixture_path};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const CONTRACTS: &[&str] = &[
    "apply-capabilities",
    "benchmark-producer",
    "cli",
    "diagnostics",
    "fixtures",
    "performance",
    "release",
    "renderer-registry",
    "schemas",
    "traceability",
    "versions",
];

#[test]
fn every_m0_contract_is_valid_json_with_one_final_lf() {
    let expected = CONTRACTS
        .iter()
        .map(|name| format!("{name}.json"))
        .collect::<BTreeSet<_>>();
    let actual = fs::read_dir(contract_directory())
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_file())
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);

    for name in CONTRACTS {
        let path = contract_directory().join(format!("{name}.json"));
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'), "{}", path.display());
        assert!(!bytes.ends_with(b"\n\n"), "{}", path.display());
        load_contract(name).unwrap();
    }
}

#[test]
fn fixture_manifest_covers_every_required_family_and_test_class() {
    let fixtures = load_contract("fixtures").unwrap();
    let families = fixtures["conformance_families"].as_array().unwrap();
    let family_numbers = families
        .iter()
        .map(|family| family["section_28_item"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(family_numbers, (1..=14).collect::<Vec<_>>());
    assert!(families.iter().all(|family| {
        family["status"] == "planned"
            && !family["owner_packages"].as_array().unwrap().is_empty()
            && !family["planned_coverage"].as_array().unwrap().is_empty()
    }));

    let classes = fixtures["test_classes"].as_array().unwrap();
    let class_ids = classes
        .iter()
        .map(|class| class["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        class_ids,
        BTreeSet::from([
            "TC01-GOLDEN",
            "TC02-PROPERTY",
            "TC03-FUZZ",
            "TC04-DIFFERENTIAL",
            "TC05-INTEGRATION",
            "TC06-FAULT-RACE",
        ])
    );

    let case_groups = fixtures["syntax_case_inventory"]["case_groups"]
        .as_array()
        .unwrap();
    let case_ids = case_groups
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(case_ids.len(), case_groups.len());
    assert_eq!(case_groups.len(), 31);
    assert!(case_groups.iter().all(|case| {
        !case["positive_cases"].as_array().unwrap().is_empty()
            && !case["negative_neighbors"].as_array().unwrap().is_empty()
    }));

    let claims = &fixtures["implementation_claims"];
    assert_eq!(claims["conformance_claimed"], false);
    let implemented = claims["implemented_fixture_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let passing = claims["passing_fixture_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    // Claim lists are unique, sorted by unsigned bytes, and name fixture
    // records that exist on disk; passing is a subset of implemented.
    let implemented_set = implemented.iter().collect::<BTreeSet<_>>();
    let passing_set = passing.iter().collect::<BTreeSet<_>>();
    assert_eq!(implemented_set.len(), implemented.len());
    assert_eq!(passing_set.len(), passing.len());
    assert!(implemented.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(passing.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(passing_set.is_subset(&implemented_set));
    let fixture_directory = contract_directory().join("fixtures");
    for id in &implemented {
        assert!(
            fixture_directory.join(format!("{id}.json")).is_file(),
            "claimed fixture {id} has no record on disk"
        );
    }
}

#[test]
fn fixture_domain_paths_match_the_frozen_manifest() {
    let contract = load_contract("fixtures").unwrap();
    let paths = contract["fixture_record_contract"]["representative_repository_paths"]
        .as_object()
        .unwrap();
    for (domain, expected) in paths {
        assert_eq!(
            representative_fixture_path(domain).unwrap().as_str(),
            expected.as_str().unwrap(),
            "{domain}",
        );
    }
    assert_eq!(paths.len(), 26);
}

#[test]
fn accepted_adrs_are_complete() {
    let directory = contract_directory()
        .join("../../../docs/dotfile-m0/adrs")
        .canonicalize()
        .unwrap();
    let mut paths = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths.len(), 8);
    for (index, path) in paths.iter().enumerate() {
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(&format!("{:04}-", index + 1)));
        let contents = fs::read_to_string(path).unwrap();
        assert!(contents.contains("Status: Accepted"));
        assert!(contents.contains("## Decision"));
        assert!(contents.contains("## Verification"));
    }
}

fn repository_root() -> PathBuf {
    contract_directory()
        .join("../../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn version_tuple_adapters_and_provenance_are_frozen() {
    let versions = load_contract("versions").unwrap();
    assert_eq!(versions["tuple"]["source"], "1");
    assert_eq!(versions["tuple"]["generated_lock"], "1");
    assert_eq!(versions["tuple"]["builtins"], "1");
    assert_eq!(versions["compatibility"]["policy"], "exact_only");

    let managers = versions["adapter_registry"]["managers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|manager| manager["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(managers, ["brew", "pacman", "apt"]);

    let installers = versions["adapter_registry"]["installers"]
        .as_array()
        .unwrap();
    let installer_names = installers
        .iter()
        .map(|installer| installer["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    let installer_orders = installers
        .iter()
        .map(|installer| installer["order"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        installer_names,
        [
            "brew-formula",
            "brew-cask",
            "pacman",
            "aur",
            "apt",
            "cargo",
            "uv"
        ]
    );
    assert_eq!(installer_orders, [10, 20, 30, 40, 50, 60, 70]);

    let checks = versions["adapter_registry"]["check_adapters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|adapter| adapter["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        checks,
        ["command", "package", "font", "service", "path", "none"]
    );

    let provenance = versions["provenance_rule_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(provenance.len(), 26);
    assert!(provenance.contains("manager/brew/default-installer"));
    assert!(provenance.contains("deploy/mapping/longest-prefix"));
    assert!(provenance.contains("theme/repository-default"));
}

#[test]
fn diagnostics_have_exact_stages_codes_and_warning_details() {
    let diagnostics = load_contract("diagnostics").unwrap();
    assert_eq!(
        diagnostics["severities"],
        serde_json::json!(["Error", "Warning"])
    );
    let expected = [
        ("lex", ["lex/encoding", "lex/token"].as_slice()),
        ("parse", ["parse/syntax"].as_slice()),
        (
            "schema",
            ["schema/context", "schema/duplicate", "schema/binding"].as_slice(),
        ),
        (
            "theme",
            [
                "theme/discovery",
                "theme/reference",
                "theme/merge",
                "theme/map",
                "theme/output",
            ]
            .as_slice(),
        ),
        (
            "resolve",
            [
                "resolve/reference",
                "resolve/identity",
                "resolve/resource-key",
                "resolve/fact-conflict",
                "resolve/adapter",
                "resolve/theme",
            ]
            .as_slice(),
        ),
        ("graph", ["graph/cycle"].as_slice()),
        (
            "discovery",
            ["discovery/group", "discovery/inventory", "discovery/source"].as_slice(),
        ),
        (
            "deploy",
            [
                "deploy/mapping",
                "deploy/permission",
                "deploy/collision",
                "deploy/variant",
            ]
            .as_slice(),
        ),
        (
            "lock",
            ["lock/stale", "lock/noncanonical", "lock/tampered"].as_slice(),
        ),
        (
            "bind",
            [
                "bind/profile",
                "bind/host",
                "bind/variant",
                "bind/destination",
            ]
            .as_slice(),
        ),
        (
            "observe",
            [
                "observe/absent",
                "observe/adapter",
                "observe/vault",
                "observe/destination",
            ]
            .as_slice(),
        ),
        (
            "apply",
            ["apply/approval", "apply/race", "apply/rollback"].as_slice(),
        ),
    ];
    let stages = diagnostics["stages"].as_array().unwrap();
    assert_eq!(stages.len(), expected.len());
    for (index, (stage, (name, codes))) in stages.iter().zip(expected).enumerate() {
        assert_eq!(stage["order"], index as u64 + 1);
        assert_eq!(stage["name"], name);
        let actual = stage["codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|code| code.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actual, codes);
    }

    let warning_details = diagnostics["details"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|detail| detail["severity"] == "Warning")
        .map(|detail| detail["detail"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        warning_details,
        BTreeSet::from([
            "package_without_metadata",
            "near_entity_name",
            "unused_entity_facts",
            "empty_deployable_facet",
            "unused_binding",
            "departed_identity",
        ])
    );
    assert!(
        diagnostics["record"]["required_fields"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("scope"))
    );
    let registered_codes = stages
        .iter()
        .flat_map(|stage| array_strings(&stage["codes"]))
        .collect::<BTreeSet<_>>();
    assert_eq!(registered_codes.len(), 39);
    assert!(
        diagnostics["details"]
            .as_array()
            .unwrap()
            .iter()
            .all(|detail| { registered_codes.contains(detail["code"].as_str().unwrap()) })
    );
    assert_eq!(
        diagnostics["code_policy"]["new_detail_requires_owner_version_revision"],
        true
    );
    assert_eq!(
        diagnostics["code_policy"]["same_owner_version_detail_addition"],
        "forbidden"
    );
    let required_details = diagnostics["code_policy"]["detail_required_for"]
        .as_array()
        .unwrap()
        .iter()
        .map(|detail| {
            format!(
                "{}#{}",
                detail["code"].as_str().unwrap(),
                detail["detail"].as_str().unwrap()
            )
        })
        .collect::<BTreeSet<_>>();
    let registered_details = diagnostics["details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|detail| {
            format!(
                "{}#{}",
                detail["code"].as_str().unwrap(),
                detail["detail"].as_str().unwrap()
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(required_details, registered_details);

    let record = &diagnostics["record"];
    let declared_fields = array_strings(&record["required_fields"])
        .into_iter()
        .chain(array_strings(&record["optional_fields"]))
        .collect::<BTreeSet<_>>();
    assert_eq!(declared_fields, object_keys(&record["field_types"]));
    assert_eq!(record["unknown_fields"], "forbidden");
    assert_eq!(record["null_values"], "forbidden");
    let span = &record["primary_span"];
    assert_eq!(
        array_strings(&span["required_fields"])
            .into_iter()
            .collect::<BTreeSet<_>>(),
        object_keys(&span["field_types"])
    );
    assert_eq!(span["unknown_fields"], "forbidden");
    assert_eq!(record["structured_data"]["json_type"], "object");
    assert_eq!(
        record["structured_data"]["null_values"],
        "forbidden_recursively"
    );
    assert_eq!(
        array_strings(&record["fix"]["required_fields"]),
        ["title", "applicability", "edits"]
    );
}

#[test]
fn cli_transport_matches_the_accepted_contract() {
    let cli = load_contract("cli").unwrap();
    assert_eq!(
        cli["json_envelope"]["required_fields"],
        serde_json::json!([
            "protocol_version",
            "command",
            "outcome",
            "changed",
            "diagnostics",
            "data"
        ])
    );
    assert_eq!(cli["json_envelope"]["fields"]["data"], "object");
    assert_eq!(cli["json_envelope"]["null_data"], "forbidden");
    let exits = cli["exit_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|exit| exit["code"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(exits, [0, 1, 2, 3, 70, 130]);
    assert_eq!(cli["repository_writes"]["atomic"], true);
    assert_eq!(cli["repository_writes"]["check_mode_writes"], false);
    assert_eq!(cli["repository_writes"]["writes_on_error"], false);
}

fn valid_base_facet(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("facet:") else {
        return false;
    };
    let mut parts = rest.split('/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(group), Some(package), None) if !group.is_empty() && !package.is_empty() && !package.contains('@'))
}

#[test]
fn renderer_outputs_are_explicit_unique_sorted_and_present() {
    let registry = load_contract("renderer-registry").unwrap();
    let artifacts = registry["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 22);
    let paths = artifacts
        .iter()
        .map(|artifact| artifact["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    let unique = paths.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), paths.len());
    assert_eq!(paths, unique.into_iter().collect::<Vec<_>>());
    assert!(artifacts.iter().all(|artifact| {
        valid_base_facet(artifact["facet"].as_str().unwrap())
            && Path::new(artifact["path"].as_str().unwrap()).is_relative()
    }));
    for path in paths {
        assert!(repository_root().join(path).is_file(), "{path}");
    }
    assert_eq!(registry["api"]["check"]["mutating"], false);
    assert_eq!(registry["api"]["render"]["mutating"], true);
    assert_eq!(
        registry["freshness"]["status_diagnostics"]["unregistered"],
        "theme/output"
    );
}

fn array_strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect()
}

fn object_keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn schema_references_and_generated_lock_records_are_total() {
    let schemas = load_contract("schemas").unwrap();
    let shapes = schemas["domain_shapes"].as_object().unwrap();
    for domain in schemas["domains"].as_array().unwrap() {
        let schema = domain["schema"].as_str().unwrap();
        assert!(shapes.contains_key(schema), "{schema}");
    }

    let value_types = schemas["value_types"].as_object().unwrap();
    let attributes = schemas["attribute_registry"].as_array().unwrap();
    assert_eq!(attributes.len(), 27);
    let attribute_names = attributes
        .iter()
        .map(|attribute| attribute["name"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(attribute_names.len(), attributes.len());
    assert_eq!(
        attribute_names,
        object_keys(&schemas["completion_registry"]["attribute_text"])
    );
    assert_eq!(
        shapes.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        object_keys(&schemas["completion_registry"]["domain_file_text"])
    );
    assert_eq!(
        object_keys(&schemas["namespace_registry"]),
        BTreeSet::from([
            "binding",
            "entity",
            "facet",
            "facet_path",
            "group",
            "host",
            "profile",
            "resource",
            "theme",
        ])
    );
    assert_eq!(
        schemas["namespace_registry"]["facet"]["variant_identity"],
        "facet:<group>/<package>@<variant>"
    );
    assert_eq!(
        schemas["namespace_registry"]["resource"]["canonical_identity"],
        "resource:<kind>/<key>"
    );
    let resource_kinds = &schemas["demand_shape_registry"]["resource_kinds"];
    assert_eq!(object_keys(resource_kinds), BTreeSet::from(["font"]));
    assert_eq!(
        resource_kinds["font"]["key_cardinality"],
        "exactly_one_direct"
    );
    assert_eq!(
        resource_kinds["font"]["key_value_type"],
        "bare_resource_key_reference"
    );
    assert_eq!(
        schemas["demand_shape_registry"]["extension"]["creates_occurrence"],
        false
    );
    for attribute in attributes {
        let value_type = attribute["value_type"].as_str().unwrap();
        assert!(value_types.contains_key(value_type), "{value_type}");
        assert!(!attribute["legal_contexts"].as_array().unwrap().is_empty());
    }

    let lock = &schemas["domain_shapes"]["generated_lock"];
    let records = lock["records"].as_object().unwrap();
    let layouts = lock["record_layouts"].as_object().unwrap();
    let field_types = lock["record_field_types"].as_object().unwrap();
    let serialized = lock["serialized_record_names"].as_object().unwrap();
    let mut typed_record_names = object_keys(&lock["record_field_types"]);
    assert!(typed_record_names.remove("header"));
    assert_eq!(
        object_keys(&lock["records"]),
        object_keys(&lock["record_layouts"])
    );
    assert_eq!(object_keys(&lock["records"]), typed_record_names);
    assert_eq!(
        object_keys(&lock["records"]),
        object_keys(&lock["serialized_record_names"])
    );

    let known_types = lock["field_types"].as_object().unwrap();
    for (name, record) in records {
        let required = array_strings(&record["required"]);
        let optional = array_strings(&record["optional"]);
        let declared = required
            .iter()
            .chain(optional.iter())
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(declared.len(), required.len() + optional.len(), "{name}");

        let order = array_strings(&layouts[name]["field_order"]);
        assert_eq!(order.len(), declared.len(), "{name}");
        assert_eq!(
            order.iter().copied().collect::<BTreeSet<_>>(),
            declared,
            "{name}"
        );
        assert_eq!(object_keys(&field_types[name]), declared, "{name}");
        assert_eq!(
            array_strings(&layouts[name]["conditional_fields"])
                .into_iter()
                .collect::<BTreeSet<_>>(),
            optional.into_iter().collect::<BTreeSet<_>>(),
            "{name}"
        );
        for type_name in field_types[name].as_object().unwrap().values() {
            let type_name = type_name.as_str().unwrap();
            assert!(known_types.contains_key(type_name), "{name}.{type_name}");
        }
        assert!(!serialized[name].as_str().unwrap().is_empty());
    }

    let header_order = array_strings(&lock["header"]["field_order"]);
    assert_eq!(
        object_keys(&lock["record_field_types"]["header"]),
        header_order.iter().copied().collect::<BTreeSet<_>>()
    );
    for type_name in lock["record_field_types"]["header"]
        .as_object()
        .unwrap()
        .values()
    {
        assert!(known_types.contains_key(type_name.as_str().unwrap()));
    }

    assert_eq!(serialized["semantic_fact"], "fact");
    assert_eq!(serialized["theme_contribution"], "contribution");
    assert_eq!(serialized["host_fact"], "fact");
    assert_eq!(field_types["occurrence"]["id"], "occurrence_id");
    assert_eq!(field_types["assertion"]["id"], "assertion_id");
    assert_eq!(field_types["node"]["resource_key"], "resource_key");
    assert_eq!(field_types["resolution"]["family"], "family_list");
    assert_eq!(field_types["candidate"]["source_type"], "source_type");

    let conditional = &lock["conditional_field_rules"];
    assert!(
        array_strings(&conditional["resolution"])
            .contains(&"installer_and_package_are_both_present_or_both_absent")
    );
    assert!(
        array_strings(&conditional["candidate"])
            .contains(&"mode_is_present_exactly_when_action_is_copy")
    );
    assert!(
        array_strings(&conditional["candidate"])
            .contains(&"vault_source_and_vault_digest_are_present_exactly_for_template_candidates")
    );
    let source_span_records = field_types
        .iter()
        .filter(|(_, fields)| fields.as_object().unwrap().contains_key("source_span"))
        .map(|(record, _)| format!("{record}.source_span"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        source_span_records,
        lock["canonical_json_type_overrides"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, value)| *value == "source_span_json")
            .map(|(field, _)| field.to_owned())
            .collect::<BTreeSet<_>>()
    );

    let structure = &lock["structure_preimage"];
    assert_eq!(
        array_strings(&structure["required_fields"]),
        [
            "files",
            "groups",
            "facets",
            "themes",
            "ignores",
            "vault_inputs"
        ]
    );
    assert_eq!(structure["unknown_fields"], "forbidden");
    assert_eq!(structure["null_values"], "forbidden");
    assert_eq!(
        object_keys(&structure["records"]),
        BTreeSet::from(["facet", "file", "group", "ignore", "vault_input"])
    );
    for (name, record) in structure["records"].as_object().unwrap() {
        let declared = array_strings(&record["required_fields"])
            .into_iter()
            .chain(array_strings(&record["optional_fields"]))
            .collect::<BTreeSet<_>>();
        assert_eq!(declared, object_keys(&record["field_types"]), "{name}");
        assert_eq!(
            declared,
            array_strings(&record["field_order"])
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "{name}"
        );
    }
    assert!(
        array_strings(&structure["records"]["file"]["conditional_rules"])
            .contains(&"raw_target_and_content_digest_never_coexist")
    );

    let override_order =
        array_strings(&schemas["domain_shapes"]["override_variant"]["canonical_root_order"]);
    assert!(override_order.contains(&"@description"));
    assert!(override_order.contains(&"@theme"));
    let baseline = schemas["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|domain| domain["id"] == "benchmark_baselines")
        .unwrap();
    assert_eq!(baseline["required"], false);
    assert_eq!(baseline["missing_behavior"], "empty_baseline_set");
}

#[test]
fn fixture_record_format_has_typed_raw_inputs_and_expected_channels() {
    let fixtures = load_contract("fixtures").unwrap();
    let contract = &fixtures["fixture_record_contract"];
    assert_eq!(
        contract["unknown_fields"],
        "forbidden_at_every_record_level"
    );
    assert_eq!(
        array_strings(&contract["input_bytes"]["required_fields"]),
        ["encoding", "value"]
    );
    assert_eq!(
        contract["input_bytes"]["field_types"]["value"],
        "json_string"
    );

    for state_name in ["repository_state", "machine_state"] {
        let state = &contract[state_name];
        assert_eq!(
            array_strings(&state["required_fields"])
                .into_iter()
                .collect::<BTreeSet<_>>(),
            object_keys(&state["field_types"]),
            "{state_name}"
        );
    }
    assert_eq!(
        contract["repository_state"]["ordering"],
        "each_array_sorted_by_decoded_unsigned_path_bytes"
    );
    assert_eq!(
        contract["record_types"]["git_index_entry"]["field_types"]["path"],
        "byte_string"
    );
    assert_eq!(
        contract["machine_state"]["field_types"]["os"],
        "json_string"
    );
    assert!(array_strings(&contract["machine_state"]["required_fields"]).contains(&"state_files"));
    assert_eq!(
        contract["record_types"]["machine_state_file"]["field_types"]["bytes"],
        "byte_string"
    );

    let expected = &contract["expected"];
    assert_eq!(
        array_strings(&expected["channel_required_fields"]),
        ["state", "comparison", "payload"]
    );
    assert_eq!(array_strings(&expected["required_fields"]).len(), 11);
    assert_eq!(
        expected["channel_field_types"]["payload"],
        "json_value_with_byte_strings_used_for_exact_bytes"
    );
}

fn capacity_gib(value: &Value) -> String {
    value
        .as_f64()
        .map(|bytes| (bytes / 1_073_741_824.0).floor() as u64)
        .map(|value| value.to_string())
        .unwrap_or_default()
}

fn evidence_epoch_fields(run: &Value) -> Vec<String> {
    let snapshot = &run["snapshot"];
    let mut fields = vec![
        snapshot["cpu"]["model"].as_str().unwrap().to_owned(),
        snapshot["cpu"]["cores_physical"]
            .as_u64()
            .unwrap()
            .to_string(),
        snapshot["cpu"]["cores_logical"]
            .as_u64()
            .unwrap()
            .to_string(),
        capacity_gib(&snapshot["memory"]["total"]),
    ];
    let mut gpus = snapshot["gpu"]
        .as_array()
        .unwrap()
        .iter()
        .map(|gpu| {
            format!(
                "{}:{}",
                gpu["name"].as_str().unwrap(),
                capacity_gib(&gpu["memory_total"])
            )
        })
        .collect::<Vec<_>>();
    gpus.sort();
    let mut disks = snapshot["disks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|disk| {
            format!(
                "{}:{}",
                disk["name"].as_str().unwrap(),
                capacity_gib(&disk["size"])
            )
        })
        .collect::<Vec<_>>();
    disks.sort();
    fields.extend(gpus);
    fields.extend(disks);
    fields
}

fn blake2s_hex(bytes: &[u8], digest_bytes: usize) -> String {
    let mut hasher = Blake2sVar::new(digest_bytes).unwrap();
    hasher.update(bytes);
    let mut output = vec![0; digest_bytes];
    hasher.finalize_variable(&mut output).unwrap();
    output
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn benchmark_interface_and_reviewed_epoch_vectors_match_evidence() {
    let benchmark = load_contract("benchmark-producer").unwrap();
    let performance = load_contract("performance").unwrap();
    assert_eq!(benchmark["epoch_identity"]["hash"], "blake2s");
    assert_eq!(
        benchmark["epoch_identity"]["input_types"]["snapshot.memory.total"],
        "finite_nonnegative_json_number_or_missing"
    );
    assert_eq!(
        benchmark["epoch_identity"]["invalid_value_behavior"],
        "reject_run_before_epoch_or_baseline_mutation"
    );
    let digest_bytes = benchmark["epoch_identity"]["digest_bytes"]
        .as_u64()
        .unwrap() as usize;
    for vector in benchmark["reviewed_epoch_vectors"].as_array().unwrap() {
        let host = vector["host"].as_str().unwrap();
        let fields = array_strings(&vector["fields"])
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let epoch = blake2s_hex(fields.join("\n").as_bytes(), digest_bytes);
        assert_eq!(epoch, vector["epoch"].as_str().unwrap());

        let host_contract = performance["reference_hosts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["name"] == host)
            .unwrap();
        let evidence_path = repository_root().join(host_contract["evidence"].as_str().unwrap());
        let evidence: Value = serde_json::from_slice(&fs::read(evidence_path).unwrap()).unwrap();
        assert_eq!(evidence["schema"], benchmark["run_schema"]["value"]);
        assert_eq!(evidence["host"], host);
        assert_eq!(evidence["epoch"], vector["epoch"]);
        assert!(
            evidence["run_id"]
                .as_str()
                .unwrap()
                .ends_with(vector["epoch"].as_str().unwrap())
        );
        assert_eq!(evidence_epoch_fields(&evidence), fields);
    }

    let validation = array_strings(&benchmark["storage"]["baseline_validation"]);
    for rule in [
        "referenced_run_host_equals_outer_host",
        "referenced_run_epoch_equals_inner_epoch",
        "recomputed_run_epoch_equals_inner_epoch",
        "run_id_epoch_suffix_equals_inner_epoch",
    ] {
        assert!(validation.contains(&rule), "{rule}");
    }
    assert_eq!(
        benchmark["storage"]["validation_failure"],
        "reject_without_mutating_baseline_document"
    );
    assert_eq!(
        benchmark["storage"]["baseline_write"],
        "same_directory_temporary_file_fsync_atomic_replace"
    );
    let clear = benchmark["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["role"] == "remove_baseline")
        .unwrap();
    assert_eq!(clear["syntax"], "sysinfo bench baseline clear <selector>");
}

#[test]
fn cli_command_matrix_preserves_mutation_and_freshness_boundaries() {
    let cli = load_contract("cli").unwrap();
    let commands = cli["commands"].as_array().unwrap();
    assert!(
        commands
            .iter()
            .all(|command| command["destination_mutation"].is_boolean())
    );
    let command = |name: &str| {
        commands
            .iter()
            .find(|command| command["name"] == name)
            .unwrap()
    };
    assert_eq!(command("sync")["destination_mutation"], true);
    assert_eq!(
        command("sync")["destination_mutation_condition"],
        "only_when_sync_invokes_link"
    );
    assert_eq!(
        command("migrate")["syntax"],
        "dotfile migrate --from legacy [--check|--write]"
    );
    assert!(array_strings(&command("lock")["reads"]).contains(&"generated_lock"));

    for name in ["link", "system install"] {
        let reads = array_strings(&command(name)["reads"]);
        for required in [
            "source",
            "repository_snapshot",
            "generated_lock",
            "machine_state",
            "state_ledger",
        ] {
            assert!(reads.contains(&required), "{name}.{required}");
        }
        assert_eq!(
            command(name)["lock_freshness_precondition"],
            "independent_recompile_and_complete_canonical_byte_compare"
        );
        let writes = command(name)["writes"].as_str().unwrap();
        assert!(writes.contains("state_ledger"));
        assert!(writes.contains("journal"));
    }
    assert_eq!(
        cli["destination_apply_preconditions"]["dry_run_writes"],
        false
    );

    for name in [
        "system status",
        "system diff",
        "status",
        "why",
        "graph",
        "packages",
    ] {
        let reads = array_strings(&command(name)["reads"]);
        assert!(reads.contains(&"source"), "{name}");
        assert!(reads.contains(&"repository_snapshot"), "{name}");
        assert!(reads.contains(&"generated_lock"), "{name}");
    }
    assert_eq!(
        cli["lock_backed_read_freshness"]["result"],
        "report_current_source_staleness_before_returning_lock_backed_data"
    );
    for name in ["theme apply", "theme switch"] {
        assert_eq!(
            command(name)["postcondition"],
            "regenerate_package_lock_after_materialized_artifact_change_before_clean_result"
        );
    }
    let io_exit = cli["exit_codes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|exit| exit["code"] == 3)
        .unwrap();
    assert_eq!(
        io_exit["meaning"],
        "repository I/O or a required external transport failed"
    );
}

#[test]
fn release_performance_and_apply_matrices_are_explicit_and_fail_closed() {
    let release = load_contract("release").unwrap();
    assert_eq!(release["lsp"]["protocol_floor"], "3.17");
    assert_eq!(release["lsp"]["transport"], "stdio");
    assert_eq!(
        release["lsp"]["position_encoding_preference"],
        serde_json::json!(["utf-8", "utf-16"])
    );
    let required_targets = BTreeSet::from([
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-gnu",
    ]);
    let artifacts = release["artifacts"].as_array().unwrap();
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact["target"].as_str().unwrap())
            .collect::<BTreeSet<_>>(),
        required_targets
    );
    assert!(artifacts.iter().all(|artifact| {
        artifact["status"] == "required"
            && artifact["blocking"] == true
            && artifact["format"] == "tar.gz"
            && artifact["binaries"] == serde_json::json!(["dotfile", "dotfile-lsp"])
    }));
    let conformance = release["conformance_matrix"].as_array().unwrap();
    for target in &required_targets {
        let row = conformance
            .iter()
            .find(|row| row["target"] == *target)
            .unwrap();
        assert_eq!(row["blocking"], true);
        assert!(array_strings(&row["scope"]).contains(&"lock"));
    }
    let windows = conformance
        .iter()
        .find(|row| row["target"] == "x86_64-pc-windows-msvc")
        .unwrap();
    assert_eq!(windows["status"], "optional");
    assert_eq!(windows["blocking"], false);
    assert_eq!(
        windows["scope"],
        serde_json::json!(["syntax", "formatter", "lsp"])
    );
    let install = array_strings(&release["distribution"]["install_procedure"]);
    let version_check = install
        .iter()
        .position(|step| step.contains("version_checks"))
        .unwrap();
    let installation = install
        .iter()
        .position(|step| step.contains("atomically_install"))
        .unwrap();
    assert!(version_check < installation);

    let required_tree_sitter_files = BTreeSet::from([
        "node-types.json",
        "queries/highlights.scm",
        "queries/locals.scm",
        "queries/folds.scm",
        "queries/indents.scm",
    ]);
    let editor = release["editor_artifacts"].as_array().unwrap();
    let native = editor
        .iter()
        .find(|artifact| artifact["name"] == "tree-sitter-dotfile-native")
        .unwrap();
    assert_eq!(native["checksum_manifest"], "SHA256SUMS");
    assert_eq!(native["sigstore_attestation_required"], true);
    assert_eq!(native["apple_native_library_signature_required"], true);
    assert!(
        native["installation"]
            .as_str()
            .unwrap()
            .contains("after_verification")
    );
    for asset in native["assets"].as_array().unwrap() {
        let contents = array_strings(&asset["contents"])
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert!(required_tree_sitter_files.is_subset(&contents));
    }
    let wasm = editor
        .iter()
        .find(|artifact| artifact["name"] == "tree-sitter-dotfile-wasm")
        .unwrap();
    assert_eq!(wasm["checksum_manifest"], "SHA256SUMS");
    assert_eq!(wasm["sigstore_attestation_required"], true);
    assert!(
        wasm["installation"]
            .as_str()
            .unwrap()
            .contains("after_verification")
    );
    let wasm_contents = array_strings(&wasm["contents"])
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert!(required_tree_sitter_files.is_subset(&wasm_contents));
    assert!(wasm_contents.contains("tree-sitter-dotfile.wasm"));
    let vscode = editor
        .iter()
        .find(|artifact| artifact["name"] == "dotfile-vscode")
        .unwrap();
    assert_eq!(vscode["checksum_manifest"], "SHA256SUMS");
    assert_eq!(vscode["sigstore_attestation_required"], true);
    assert_eq!(
        vscode["package_signature"],
        "sigstore_attestation_bound_to_release_identity"
    );

    let performance = load_contract("performance").unwrap();
    let active = performance["gate_policy"]["active"].as_bool().unwrap();
    for corpus in performance["corpora"].as_object().unwrap().values() {
        let artifact = repository_root().join(corpus["artifact"].as_str().unwrap());
        if active {
            assert_eq!(corpus["status"], "frozen");
            assert!(artifact.exists());
            let digest = corpus["tree_digest"].as_str().unwrap();
            assert_eq!(digest.len(), 64);
            assert!(
                digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            );
        } else {
            assert_eq!(corpus["status"], "planned_unfrozen");
            assert!(!artifact.exists());
            assert_eq!(corpus["tree_digest"], "required_before_gate_activation");
        }
    }
    assert_eq!(
        performance["gate_activation"]["state"],
        "inactive_unfrozen_corpora"
    );
    assert_eq!(performance["gates"].as_array().unwrap().len(), 13);
    assert!(performance["gates"].as_array().unwrap().iter().all(|gate| {
        gate["statistic"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    }));
    let hosts = performance["reference_hosts"].as_array().unwrap();
    assert_eq!(
        hosts
            .iter()
            .map(|host| host["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["archie", "macie"])
    );
    for host in hosts {
        let evidence: Value = serde_json::from_slice(
            &fs::read(repository_root().join(host["evidence"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        assert_eq!(
            host["measured_memory_bytes"].as_u64().unwrap(),
            evidence["snapshot"]["memory"]["total"].as_u64().unwrap()
        );
    }

    let apply = load_contract("apply-capabilities").unwrap();
    assert_eq!(apply["os_names"]["mapping"]["darwin"], "macos");
    assert_eq!(apply["filesystems"].as_array().unwrap().len(), 3);
    for row in apply["filesystems"].as_array().unwrap() {
        let primitives = &row["create_no_replace"]["primitives"];
        assert_eq!(primitives["directory"], "mkdirat");
        assert_eq!(primitives["symlink"], "symlinkat");
        assert_eq!(row["guarded_replace"]["static_status"], "unsupported");
        assert_eq!(row["guarded_replace"]["automatic_operation"], false);
        assert_eq!(row["guarded_prune"]["static_status"], "unsupported");
        assert_eq!(row["guarded_prune"]["automatic_operation"], false);
    }
    assert_eq!(apply["path_traversal"]["descriptor_relative"], true);
    assert_eq!(apply["path_traversal"]["no_follow"], true);
    assert_eq!(apply["filesystem_test_matrix"].as_array().unwrap().len(), 3);
}
