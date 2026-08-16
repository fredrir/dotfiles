//! Versioned conformance fixture records and the lex/parse/bootstrap
//! fixture runner.
//!
//! The record format is frozen by `contracts/dotfile/v1/fixtures.json`.
//! Pure syntax fixtures carry empty repository and machine state; the runner
//! decodes `input_bytes`, executes the named operation, and compares the
//! golden channels byte-for-byte or as exact JSON.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

use dotfile_format::{FormatError, format_source};
use dotfile_schema::{DomainClassifier, GroupLayout, GroupLayoutEntry, LoweringError, lower_path};
use dotfile_source::{RepoPath, SourceText, read_bootstrap, sort_diagnostics};
use dotfile_syntax::{dump_tokens, lex, parse};
use dotfile_theme::lower_theme_file;
use serde_json::{Map, Value};

use crate::contract_directory;

/// One loaded fixture record.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub id: String,
    pub status: String,
    pub normative_references: Vec<NormativeReference>,
    pub domain: String,
    pub operation: String,
    pub path: RepoPath,
    pub input: Vec<u8>,
    pub repository_state: Value,
    pub machine_state: Value,
    pub expected: Value,
}

/// One normative source attached to a fixture record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormativeReference {
    pub path: RepoPath,
    pub section: String,
    pub clause: Option<String>,
    pub rule_id: Option<String>,
}

#[derive(Debug)]
pub enum FixtureError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Record { id: String, message: String },
}

impl Display for FixtureError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Json(error) => Display::fmt(error, formatter),
            Self::Record { id, message } => write!(formatter, "fixture {id}: {message}"),
        }
    }
}

impl Error for FixtureError {}

impl From<std::io::Error> for FixtureError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for FixtureError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn record_error(id: &str, message: impl Into<String>) -> FixtureError {
    FixtureError::Record {
        id: id.to_owned(),
        message: message.into(),
    }
}

/// The directory holding the versioned fixture records.
pub fn fixture_directory() -> PathBuf {
    contract_directory().join("fixtures")
}

/// Loads every fixture record, sorted by unsigned UTF-8 bytes of id.
pub fn load_fixtures() -> Result<Vec<Fixture>, FixtureError> {
    let directory = fixture_directory();
    let paths: Vec<PathBuf> = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    let mut fixtures = paths
        .iter()
        .map(|path| load_fixture(path))
        .collect::<Result<Vec<_>, _>>()?;
    fixtures.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
    for pair in fixtures.windows(2) {
        if pair[0].id == pair[1].id {
            return Err(record_error(&pair[0].id, "duplicate fixture id"));
        }
    }
    Ok(fixtures)
}

/// Loads and validates one fixture record.
pub fn load_fixture(path: &Path) -> Result<Fixture, FixtureError> {
    let bytes = fs::read(path)?;
    let value: Value = serde_json::from_slice(&bytes)?;
    let object = value
        .as_object()
        .ok_or_else(|| record_error("<unknown>", "record is not an object"))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| record_error("<unknown>", "missing or non-string id"))?
        .to_owned();
    validate_ascii_identifier(&id, &id, "id")?;
    let expected_filename = format!("{id}.json");
    let actual_filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| record_error(&id, "fixture filename is not valid UTF-8"))?;
    if actual_filename != expected_filename {
        return Err(record_error(
            &id,
            format!("filename must be {expected_filename}, got {actual_filename}"),
        ));
    }
    const TOP_LEVEL: &[&str] = &[
        "id",
        "format_version",
        "status",
        "normative_references",
        "domain",
        "operation",
        "input_bytes",
        "repository_state",
        "machine_state",
        "expected",
    ];
    validate_fields(&id, "fixture record", object, TOP_LEVEL, &[])?;
    if value["format_version"] != "1" {
        return Err(record_error(&id, "format_version is not \"1\""));
    }
    let status = required_string(&id, "fixture record", object, "status")?.to_owned();
    if !["planned", "implemented", "passing", "failing", "blocked"].contains(&status.as_str()) {
        return Err(record_error(&id, format!("invalid status {status}")));
    }
    let normative_references = validate_normative_references(&id, &value["normative_references"])?;
    let domain = required_string(&id, "fixture record", object, "domain")?.to_owned();
    validate_ascii_identifier(&id, &domain, "domain")?;
    let fixture_path = representative_fixture_path(&domain)
        .ok_or_else(|| record_error(&id, format!("unknown fixture domain {domain}")))?;
    let operation = required_string(&id, "fixture record", object, "operation")?.to_owned();
    validate_ascii_identifier(&id, &operation, "operation")?;
    let input = decode_byte_string(&value["input_bytes"])
        .ok_or_else(|| record_error(&id, "invalid input_bytes"))?;
    validate_repository_state(&id, &value["repository_state"])?;
    validate_machine_state(&id, &value["machine_state"])?;
    let expected = value["expected"].clone();
    validate_expected(&id, &expected)?;
    if status == "passing" {
        validate_passing_obligations(&id, &expected)?;
    }
    Ok(Fixture {
        id,
        status,
        normative_references,
        domain,
        operation,
        path: fixture_path,
        input,
        repository_state: value["repository_state"].clone(),
        machine_state: value["machine_state"].clone(),
        expected,
    })
}

fn validate_passing_obligations(id: &str, expected: &Value) -> Result<(), FixtureError> {
    if expected["diagnostics"]["state"] == "golden"
        && expected["diagnostics"]["payload"]
            .as_array()
            .is_some_and(Vec::is_empty)
    {
        return Err(record_error(
            id,
            "passing negative fixture has an empty diagnostic golden",
        ));
    }
    if id == "f03-schema-unsupported-version-detail"
        && !expected["diagnostics"]["payload"]
            .as_array()
            .is_some_and(|diagnostics| {
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["detail"] == "unsupported_dotfile_version")
            })
    {
        return Err(record_error(
            id,
            "unsupported version golden lacks detail unsupported_dotfile_version",
        ));
    }
    if id == "f03-schema-binding-shadow-outer-initializer" {
        let values = expected["ir"]["payload"]["bindings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|binding| binding["evaluated"]["value"].as_str())
            .collect::<Vec<_>>();
        if values != ["outer", "outer.inner"] {
            return Err(record_error(
                id,
                "shadowing golden does not resolve the inner initializer through the outer binding",
            ));
        }
    }
    if id == "f11-format-keyless-resource-barrier" {
        let bytes = decode_byte_string(&expected["edits"]["payload"]);
        let required = b"zsh\n@font { @family = \"Hack Nerd Font Mono\" }\nalacritty\nwezterm\n";
        if bytes.as_deref() != Some(required) {
            return Err(record_error(
                id,
                "keyless resource golden does not preserve the sorting barrier",
            ));
        }
    }
    if id == "f11-format-template-variables-refusal"
        && expected["edits"]["payload"]
            != serde_json::json!({
                "error": "unsupported_domain",
                "domain": "template_variables",
            })
    {
        return Err(record_error(
            id,
            "template-variable formatter golden does not record unsupported-domain refusal",
        ));
    }
    if id == "f11-format-unknown-dotfile-preserve" {
        let bytes = decode_byte_string(&expected["edits"]["payload"]);
        let required = b"z { b = \"2\", a = \"1\" }\n# attached root assignment\na = \"root\"\n";
        if bytes.as_deref() != Some(required) {
            return Err(record_error(
                id,
                "unknown-dotfile golden does not canonicalize while preserving both source orders",
            ));
        }
    }
    if id == "f11-format-not-dotfile-refusal"
        && expected["edits"]["payload"]
            != serde_json::json!({
                "error": "unclassified_path",
                "path": "notes.txt",
            })
    {
        return Err(record_error(
            id,
            "NotDotfile formatter golden does not record the classified-path refusal",
        ));
    }
    match id {
        "f03-schema-path-validation-matrix" => {
            require_diagnostic_summary_count(
                id,
                expected,
                "decoded source path is not Unicode NFC",
                1,
            )?;
            require_diagnostic_summary_count(
                id,
                expected,
                "value is invalid for `destination_path_expression`",
                14,
            )?;
        }
        "f03-schema-resource-key-matrix" => {
            require_diagnostic_summary_count(
                id,
                expected,
                "resource demand requires exactly one direct bare `@key`",
                3,
            )?;
            require_diagnostic_summary_count(id, expected, "duplicate attribute `@key`", 1)?;
            require_diagnostic_summary_count(
                id,
                expected,
                "`bare_resource_key_reference` requires a bare typed reference",
                1,
            )?;
            require_diagnostic_summary_count(
                id,
                expected,
                "entity demands are not legal in this requirement context",
                1,
            )?;
        }
        "f03-schema-description-linebreak-requirements" => {
            require_diagnostic_summary_count(
                id,
                expected,
                "value is invalid for `one_line_string`",
                5,
            )?;
        }
        "f03-schema-description-linebreak-profiles" => {
            require_diagnostic_summary_count(
                id,
                expected,
                "value is invalid for `one_line_string`",
                2,
            )?;
        }
        "f03-schema-duplicate-prior-origins" => {
            require_diagnostic_count(id, expected, 4)?;
            require_related_span_counts(id, expected, "duplicate host field `hostnames`", &[1, 2])?;
            require_related_span_counts(
                id,
                expected,
                "duplicate hostname alias `shared.example`",
                &[1, 2],
            )?;
        }
        "f03-schema-host-own-id-alias" => {
            let aliases = expected["ir"]["payload"]["root"]["hosts"][0]["hostnames"]
                ["value"]["kind"]["values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value["kind"]["value"].as_str())
                .collect::<Vec<_>>();
            if aliases != ["ARCHIE.", "archie.local"] {
                return Err(record_error(
                    id,
                    "host own-ID golden does not retain the normalized explicit aliases",
                ));
            }
        }
        "f03-schema-host-alias-negative-matrix" => {
            require_diagnostic_summary_count(
                id,
                expected,
                "duplicate hostname alias `shared.example`",
                1,
            )?;
            require_diagnostic_summary_count(
                id,
                expected,
                "hostname alias must decode to one line",
                1,
            )?;
        }
        "f03-schema-requirement-context-positive" => {
            let ir = &expected["ir"]["payload"];
            for (kind, minimum) in [
                ("binding", 1),
                ("path", 2),
                ("entity", 2),
                ("resource", 1),
                ("extension", 2),
            ] {
                if json_string_field_count(ir, "kind", kind) < minimum {
                    return Err(record_error(
                        id,
                        format!("requirement-context golden lacks {minimum} `{kind}` HIR nodes"),
                    ));
                }
            }
        }
        "f03-schema-requirement-context-illegal" => {
            for (summary, count) in [
                ("attribute is not legal in this requirement context", 6),
                (
                    "entity demands are not legal in this requirement context",
                    1,
                ),
                (
                    "resource demands are not legal in this requirement context",
                    1,
                ),
                ("entity-only fact is not legal on a font extension", 4),
                (
                    "extension target namespace is not registered in source version 1",
                    1,
                ),
            ] {
                require_diagnostic_summary_count(id, expected, summary, count)?;
            }
        }
        "f03-schema-variant-attribute-matrix" => {
            let ir = &expected["ir"]["payload"];
            if ir["root"]["context"] != "variant"
                || json_string_field_count(ir, "kind", "path") != 2
            {
                return Err(record_error(
                    id,
                    "variant attribute golden lacks the typed variant root and two path nodes",
                ));
            }
        }
        "f03-schema-closed-bindings-profiles" => require_diagnostic_summary_count(
            id,
            expected,
            "binding declaration is not legal in this schema context",
            3,
        )?,
        "f03-schema-closed-bindings-hosts"
        | "f03-schema-closed-bindings-recipients"
        | "f03-schema-closed-bindings-benchmark" => require_diagnostic_summary_count(
            id,
            expected,
            "binding declaration is not legal in this schema context",
            2,
        )?,
        "f03-schema-closed-bindings-scan" => require_diagnostic_summary_count(
            id,
            expected,
            "binding declaration is not legal in this schema context",
            3,
        )?,
        "f03-schema-recipient-age-negative-matrix" => require_diagnostic_summary_count(
            id,
            expected,
            "value is invalid for `age_public_recipient`",
            3,
        )?,
        "f03-schema-recipient-syntax-matrix" => {
            require_diagnostic_count(id, expected, 8)?;
            require_diagnostic_summary_count(
                id,
                expected,
                "value is invalid for `age_public_recipient`",
                7,
            )?;
            require_diagnostic_summary_count(
                id,
                expected,
                "duplicate recipient label `duplicate`",
                1,
            )?;
        }
        "f03-schema-benchmark-negative-matrix" => {
            for (summary, count) in [
                (
                    "benchmark epoch must be eight lowercase hexadecimal digits",
                    1,
                ),
                (
                    "benchmark run ID is malformed or its epoch suffix does not match the key",
                    2,
                ),
                ("duplicate benchmark epoch `10db7d1f`", 1),
                ("duplicate benchmark host `archie`", 1),
            ] {
                require_diagnostic_summary_count(id, expected, summary, count)?;
            }
        }
        "f03-schema-bootstrap-sealing-matrix" => {
            validate_bootstrap_sealing_golden(id, expected)?;
        }
        "f03-schema-tolerant-recovery" => validate_schema_recovery_golden(id, expected)?,
        "f11-format-width-unicode-canonical" => {
            let output = expected_edit_text(id, expected)?;
            let lines = output.lines().collect::<Vec<_>>();
            let fits = lines
                .iter()
                .find(|line| line.starts_with("fits {"))
                .copied();
            if fits.is_none_or(|line| line.chars().count() != 100)
                || !lines.contains(&"wraps {")
                || output.contains("\\u{")
                || output.chars().filter(|scalar| *scalar == '😀').count() != 146
                || output.chars().filter(|scalar| *scalar == 'é').count() != 2
            {
                return Err(record_error(
                    id,
                    "Unicode-width golden does not preserve the exact 100-scalar boundary and canonical literals",
                ));
            }
        }
        "f11-format-published-barrier-duplicate-comments" => require_ordered_fragments(
            id,
            expected,
            &[
                "terminal = \"9\"",
                "fonts = \"wrong\"",
                "# attached to fonts",
                "general = \"First\"",
                "# first duplicate sizes",
                "terminal = \"10\"",
                "# second duplicate sizes",
                "terminal = \"11\"",
                "# attached to applications",
                "obsidian = \"enabled\"",
            ],
        )?,
        "f11-format-names-barrier-duplicate-comments" => require_ordered_fragments(
            id,
            expected,
            &[
                "zulu",
                "barrier = \"wrong\"",
                "# first alpha",
                "role = \"desktop\"",
                "# second alpha",
                "role = \"laptop\"",
                "# attached beta",
                "beta",
            ],
        )?,
        "f11-format-resource-duplicate-key-comments" => require_ordered_fragments(
            id,
            expected,
            &[
                "zsh",
                "# first identity",
                "@key = hack",
                "# duplicate identity",
                "@key = other",
                "@description = \"font\"",
                "alacritty",
                "wezterm",
            ],
        )?,
        "f11-format-recipient-invalid-identity-barrier" => require_ordered_fragments(
            id,
            expected,
            &[
                "zulu = \"z\"",
                "# invalid recipient label",
                "bad+label = \"invalid\"",
                "# lower valid label one",
                "alpha = \"a\"",
                "# lower valid label two",
                "beta = \"b\"",
            ],
        )?,
        "f11-format-benchmark-invalid-identity-barrier" => require_ordered_fragments(
            id,
            expected,
            &[
                "ffffffff = \"high\"",
                "# invalid benchmark epoch",
                "ABCDEF01 = \"invalid\"",
                "# lower valid epoch one",
                "00000001 = \"one\"",
                "# lower valid epoch two",
                "00000002 = \"two\"",
            ],
        )?,
        "f13-theme-font-family-comma-matrix" => {
            require_diagnostic_count(id, expected, 5)?;
            for name in ["leading", "middle", "trailing", "escaped"] {
                require_diagnostic_summary_count(
                    id,
                    expected,
                    &format!("font family `{name}` must not contain a comma"),
                    1,
                )?;
            }
            require_diagnostic_summary_count(
                id,
                expected,
                "theme field `empty` must not be empty",
                1,
            )?;
        }
        "f13-theme-tolerant-hir-recovery" => validate_tolerant_theme_hir(id, expected)?,
        _ => {}
    }
    Ok(())
}

fn validate_tolerant_theme_hir(id: &str, expected: &Value) -> Result<(), FixtureError> {
    require_diagnostic_count(id, expected, 6)?;
    for summary in [
        "expected a value",
        "theme field `display-name` must be an assignment",
        "missing required theme field `icons`",
        "palette `bad` must be lowercase `#[0-9a-f]{6}`",
        "duplicate palette color value `#abcdef`",
        "unknown root field `unknown` in theme profile",
    ] {
        require_diagnostic_summary_count(id, expected, summary, 1)?;
    }

    let ir = &expected["ir"]["payload"];
    if ir["kind"] != "profile"
        || ir["validated"] != false
        || !ir["document"].is_null()
        || !ir["partial_document"].is_null()
    {
        return Err(record_error(
            id,
            "tolerant theme golden does not retain an invalid profile beside the sealed projection",
        ));
    }
    let nodes = ir["hir"]["nodes"]
        .as_array()
        .ok_or_else(|| record_error(id, "tolerant theme golden has no HIR node arena"))?;
    let source_map = ir["hir"]["source_map"]
        .as_array()
        .ok_or_else(|| record_error(id, "tolerant theme golden has no source-map projection"))?;
    let root = ir["hir"]["root"]
        .as_u64()
        .ok_or_else(|| record_error(id, "tolerant theme golden has no numeric root ID"))?;

    let mut by_id = std::collections::BTreeMap::new();
    for node in nodes {
        let node_id = node["id"]
            .as_u64()
            .ok_or_else(|| record_error(id, "theme HIR node has no numeric ID"))?;
        if by_id.insert(node_id, node).is_some() {
            return Err(record_error(id, "theme HIR node IDs are not unique"));
        }
    }
    if !by_id.contains_key(&root) || nodes.len() != source_map.len() {
        return Err(record_error(
            id,
            "theme HIR root or one-to-one source-map coverage is missing",
        ));
    }

    for (&node_id, node) in &by_id {
        let parent = node["parent"].as_u64();
        if node_id == root {
            if !node["parent"].is_null() {
                return Err(record_error(id, "theme HIR root has a parent"));
            }
        } else {
            let parent = parent
                .and_then(|parent| by_id.get(&parent))
                .ok_or_else(|| record_error(id, "theme HIR child has no arena parent"))?;
            if !json_array_contains_u64(&parent["children"], node_id) {
                return Err(record_error(
                    id,
                    "theme HIR parent does not point back to its child",
                ));
            }
        }
        for child_id in node["children"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
        {
            if by_id
                .get(&child_id)
                .and_then(|child| child["parent"].as_u64())
                != Some(node_id)
            {
                return Err(record_error(
                    id,
                    "theme HIR child does not point back to its parent",
                ));
            }
        }
    }

    let mut mapped_ids = std::collections::BTreeSet::new();
    for mapping in source_map {
        let mapped_id = mapping["hir_id"]
            .as_u64()
            .ok_or_else(|| record_error(id, "theme source-map record has no HIR ID"))?;
        let node = by_id
            .get(&mapped_id)
            .ok_or_else(|| record_error(id, "theme source map points outside the HIR arena"))?;
        if !mapped_ids.insert(mapped_id)
            || mapping["range"] != node["range"]
            || mapping["range"] != node["origin"]["range"]
            || mapping["authored"] != node["origin"]["authored"]
            || !json_array_contains_u64(&mapping["theme_ids_for_range"], mapped_id)
            || (mapping["authored"] == true
                && !json_array_contains_u64(&mapping["theme_ids_for_syntax"], mapped_id))
            || (mapping["authored"] == false
                && mapping["theme_ids_for_syntax"]
                    .as_array()
                    .is_none_or(|ids| !ids.is_empty()))
        {
            return Err(record_error(
                id,
                "theme source-map golden is not bidirectional for every semantic ID",
            ));
        }
    }

    let missing_value = find_theme_node(nodes, "missing_value", "Value(NonemptyString)");
    let missing_icons = find_theme_node(nodes, "missing_field", "icons");
    let display_name = find_theme_node(nodes, "entry", "display-name");
    let appearance = find_theme_node(nodes, "entry", "appearance");
    let flavour = find_theme_node(nodes, "entry", "flavour");
    let bad_palette = find_theme_node(nodes, "decoded", "#ABCDEF");
    let unknown = find_theme_node(nodes, "entry", "unknown");
    let syntax_error = nodes.iter().find(|node| node["kind"] == "syntax_error");
    let duplicate_palette = nodes.iter().find(|node| {
        node["kind"]["decoded"] == "#abcdef" && poison_kind_count(node, "duplicate") == 1
    });
    let good_palette = nodes.iter().find(|node| {
        node["kind"]["decoded"] == "#abcdef" && node["poison"].as_array().is_some_and(Vec::is_empty)
    });

    if missing_value.is_none_or(|node| poison_kind_count(node, "missing") != 1)
        || missing_icons.is_none_or(|node| {
            node["origin"]["authored"] != false || poison_kind_count(node, "missing") == 0
        })
        || display_name.is_none_or(|node| node["kind"]["authored"] != "Incomplete")
        || appearance.is_none_or(|node| {
            node["kind"]["authored"] != "Assignment"
                || !node["poison"].as_array().is_some_and(Vec::is_empty)
        })
        || flavour.is_none_or(|node| !node["poison"].as_array().is_some_and(Vec::is_empty))
        || bad_palette.is_none_or(|node| poison_kind_count(node, "value") != 1)
        || duplicate_palette.is_none()
        || good_palette.is_none()
        || unknown.is_none_or(|node| {
            !node["kind"]["expected"].is_null() || poison_kind_count(node, "context") != 1
        })
        || syntax_error.is_none_or(|node| poison_kind_count(node, "syntax") != 1)
    {
        return Err(record_error(
            id,
            "tolerant theme golden dropped missing, valid, invalid, duplicate, unknown, or syntax-poisoned semantic data",
        ));
    }
    Ok(())
}

fn validate_bootstrap_sealing_golden(id: &str, expected: &Value) -> Result<(), FixtureError> {
    require_diagnostic_count(id, expected, 5)?;
    require_diagnostic_summary_count(
        id,
        expected,
        "`@dotfile-version` must use the exact source-version preamble shape",
        2,
    )?;
    require_diagnostic_summary_count(id, expected, "duplicate `@dotfile-version` declaration", 2)?;
    require_diagnostic_summary_count(id, expected, "expected a separator between entries", 1)?;

    let ir = &expected["ir"]["payload"];
    let source_ids = validate_schema_source_map(id, ir)?;
    let version = &ir["root"]["version"];
    if ir["root"]["kind"] != "profiles"
        || version["name"] != "@dotfile-version"
        || version["value"]["kind"]["value"] != "1"
        || !version["poison"].as_array().is_some_and(Vec::is_empty)
        || ir["poison"].as_array().is_none_or(Vec::is_empty)
    {
        return Err(record_error(
            id,
            "bootstrap sealing golden lacks its valid exact control or file-level poison",
        ));
    }
    let recovery = ir["recovery"]
        .as_array()
        .ok_or_else(|| record_error(id, "bootstrap sealing golden has no recovery arena"))?;
    if recovery.len() != 3
        || recovery[..2].iter().any(|item| {
            item["kind"] != "attribute"
                || item["value"]["name"] != "@dotfile-version"
                || item["value"]["value"]["kind"]["value"] != "1"
                || poison_kind_count(&item["value"], "value") != 1
                || poison_kind_count(&item["value"], "duplicate") != 1
                || item["value"]["id"]
                    .as_u64()
                    .is_none_or(|hir| !source_ids.contains(&hir))
        })
        || recovery[2]["kind"] != "poison"
        || poison_kind_count(&recovery[2], "syntax") != 1
        || recovery[2]["id"]
            .as_u64()
            .is_none_or(|hir| !source_ids.contains(&hir))
    {
        return Err(record_error(
            id,
            "bootstrap sealing golden does not retain both malformed duplicate preambles and the rejected same-line entry",
        ));
    }
    Ok(())
}

fn validate_schema_recovery_golden(id: &str, expected: &Value) -> Result<(), FixtureError> {
    require_diagnostic_count(id, expected, 3)?;
    for summary in [
        "duplicate host field `hostnames`",
        "duplicate host `@profile`",
        "entry is not legal in a host block",
    ] {
        require_diagnostic_summary_count(id, expected, summary, 1)?;
    }

    let ir = &expected["ir"]["payload"];
    let source_ids = validate_schema_source_map(id, ir)?;
    let host = &ir["root"]["hosts"][0];
    if ir["root"]["kind"] != "hosts"
        || host["name"] != "archie"
        || host["hostnames"]["value"]["kind"]["values"][0]["kind"]["value"] != "archie"
        || host["role"]["value"]["kind"]["value"] != "desktop"
        || host["profile"]["value"]["kind"]["name"] != "workstation"
        || !host["hostnames"]["poison"]
            .as_array()
            .is_some_and(Vec::is_empty)
        || !host["role"]["poison"].as_array().is_some_and(Vec::is_empty)
        || !host["profile"]["poison"]
            .as_array()
            .is_some_and(Vec::is_empty)
    {
        return Err(record_error(
            id,
            "schema recovery golden dropped or poisoned an independent valid host sibling",
        ));
    }

    let recovery = ir["recovery"]
        .as_array()
        .ok_or_else(|| record_error(id, "schema recovery golden has no recovery arena"))?;
    if recovery.len() != 3
        || recovery[0]["kind"] != "named_field"
        || recovery[0]["value"]["name"] != "hostnames"
        || recovery[0]["value"]["value"]["kind"]["values"][0]["kind"]["value"] != "duplicate"
        || poison_kind_count(&recovery[0]["value"], "duplicate") != 1
        || recovery[1]["kind"] != "attribute"
        || recovery[1]["value"]["name"] != "@profile"
        || recovery[1]["value"]["value"]["kind"]["name"] != "server"
        || poison_kind_count(&recovery[1]["value"], "duplicate") != 1
        || recovery[2]["kind"] != "poison"
        || poison_kind_count(&recovery[2], "context") != 1
    {
        return Err(record_error(
            id,
            "schema recovery golden lacks the named-field, attribute, and structural-poison variants",
        ));
    }
    for recovered in recovery {
        let value = if recovered["kind"] == "poison" {
            recovered
        } else {
            &recovered["value"]
        };
        if value["id"]
            .as_u64()
            .is_none_or(|hir| !source_ids.contains(&hir))
        {
            return Err(record_error(
                id,
                "schema recovery node has no owned source-map identity",
            ));
        }
    }
    Ok(())
}

fn validate_schema_source_map(
    id: &str,
    ir: &Value,
) -> Result<std::collections::BTreeSet<u64>, FixtureError> {
    let records = ir["source_map"]
        .as_array()
        .ok_or_else(|| record_error(id, "schema golden has no source map"))?;
    let mut ids = std::collections::BTreeSet::new();
    let mut previous = None;
    for record in records {
        let hir = record["hir"]
            .as_u64()
            .ok_or_else(|| record_error(id, "schema source-map record has no numeric HIR ID"))?;
        let syntax_reverse = record["hir_ids_for_syntax"]
            .as_array()
            .ok_or_else(|| record_error(id, "schema source-map record lacks syntax reverse IDs"))?;
        if !ids.insert(hir)
            || previous.is_some_and(|previous| previous >= hir)
            || !json_array_contains_u64(&record["hir_ids_for_range"], hir)
            || (record["syntax"].is_null() && !syntax_reverse.is_empty())
            || (!record["syntax"].is_null()
                && !syntax_reverse
                    .iter()
                    .any(|value| value.as_u64() == Some(hir)))
        {
            return Err(record_error(
                id,
                "schema source-map golden is not sorted, unique, and bidirectional",
            ));
        }
        previous = Some(hir);
    }
    Ok(ids)
}

fn find_theme_node<'a>(nodes: &'a [Value], field: &str, expected: &str) -> Option<&'a Value> {
    nodes.iter().find(|node| node["kind"][field] == expected)
}

fn poison_kind_count(node: &Value, expected: &str) -> usize {
    node["poison"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|poison| poison["kind"] == expected)
        .count()
}

fn json_array_contains_u64(value: &Value, expected: u64) -> bool {
    value
        .as_array()
        .is_some_and(|values| values.iter().any(|value| value.as_u64() == Some(expected)))
}

fn require_diagnostic_count(
    id: &str,
    expected: &Value,
    required: usize,
) -> Result<(), FixtureError> {
    let actual = expected["diagnostics"]["payload"]
        .as_array()
        .map_or(0, Vec::len);
    if actual != required {
        return Err(record_error(
            id,
            format!("diagnostic golden has {actual} records, expected {required}"),
        ));
    }
    Ok(())
}

fn require_related_span_counts(
    id: &str,
    expected: &Value,
    summary: &str,
    required: &[usize],
) -> Result<(), FixtureError> {
    let diagnostics = expected["diagnostics"]["payload"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|diagnostic| diagnostic["summary"] == summary)
        .collect::<Vec<_>>();
    let actual = diagnostics
        .iter()
        .map(|diagnostic| diagnostic["related_spans"].as_array().map_or(0, Vec::len))
        .collect::<Vec<_>>();
    let canonically_ordered = diagnostics.iter().all(|diagnostic| {
        diagnostic["related_spans"].as_array().is_some_and(|spans| {
            spans.windows(2).all(|pair| {
                let left = &pair[0];
                let right = &pair[1];
                (
                    left["path"].as_str(),
                    left["start_byte"].as_u64(),
                    left["end_byte"].as_u64(),
                ) <= (
                    right["path"].as_str(),
                    right["start_byte"].as_u64(),
                    right["end_byte"].as_u64(),
                )
            })
        })
    });
    if actual != required || !canonically_ordered {
        return Err(record_error(
            id,
            format!(
                "diagnostic golden has related-span counts {actual:?} for `{summary}`, expected {required:?} in canonical order"
            ),
        ));
    }
    Ok(())
}

fn require_diagnostic_summary_count(
    id: &str,
    expected: &Value,
    summary: &str,
    required: usize,
) -> Result<(), FixtureError> {
    let actual = expected["diagnostics"]["payload"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|diagnostic| diagnostic["summary"] == summary)
        .count();
    if actual != required {
        return Err(record_error(
            id,
            format!("diagnostic golden has {actual} `{summary}` summaries, expected {required}"),
        ));
    }
    Ok(())
}

fn json_string_field_count(value: &Value, field: &str, expected: &str) -> usize {
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| json_string_field_count(value, field, expected))
            .sum(),
        Value::Object(object) => {
            usize::from(object.get(field).and_then(Value::as_str) == Some(expected))
                + object
                    .values()
                    .map(|value| json_string_field_count(value, field, expected))
                    .sum::<usize>()
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => 0,
    }
}

fn expected_edit_text(id: &str, expected: &Value) -> Result<String, FixtureError> {
    let bytes = decode_byte_string(&expected["edits"]["payload"])
        .ok_or_else(|| record_error(id, "formatter golden is not an exact byte string"))?;
    String::from_utf8(bytes)
        .map_err(|_| record_error(id, "formatter golden is not valid UTF-8 source text"))
}

fn require_ordered_fragments(
    id: &str,
    expected: &Value,
    fragments: &[&str],
) -> Result<(), FixtureError> {
    let output = expected_edit_text(id, expected)?;
    let mut offset = 0;
    for fragment in fragments {
        let Some(relative) = output[offset..].find(fragment) else {
            return Err(record_error(
                id,
                format!("formatter golden lacks ordered fragment {fragment:?}"),
            ));
        };
        offset += relative + fragment.len();
    }
    Ok(())
}

fn validate_fields(
    id: &str,
    context: &str,
    object: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
) -> Result<(), FixtureError> {
    for field in required {
        if !object.contains_key(*field) {
            return Err(record_error(
                id,
                format!("{context} is missing field {field}"),
            ));
        }
    }
    for field in object.keys() {
        if !required.contains(&field.as_str()) && !optional.contains(&field.as_str()) {
            return Err(record_error(
                id,
                format!("{context} has unknown field {field}"),
            ));
        }
    }
    Ok(())
}

fn required_string<'a>(
    id: &str,
    context: &str,
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, FixtureError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| record_error(id, format!("{context}.{field} is not a string")))
}

fn validate_ascii_identifier(id: &str, value: &str, field: &str) -> Result<(), FixtureError> {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(record_error(id, format!("{field} is empty")));
    };
    if !first.is_ascii_alphanumeric()
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(record_error(
            id,
            format!("{field} is not a nonempty ASCII identifier"),
        ));
    }
    Ok(())
}

fn validate_normative_references(
    id: &str,
    value: &Value,
) -> Result<Vec<NormativeReference>, FixtureError> {
    let records = value
        .as_array()
        .ok_or_else(|| record_error(id, "normative_references is not an array"))?;
    if records.is_empty() {
        return Err(record_error(id, "normative_references is empty"));
    }
    records
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("normative_references[{index}]");
            let object = value
                .as_object()
                .ok_or_else(|| record_error(id, format!("{context} is not an object")))?;
            validate_fields(
                id,
                &context,
                object,
                &["path", "section"],
                &["clause", "rule_id"],
            )?;
            let path_text = required_string(id, &context, object, "path")?;
            let path = RepoPath::new(path_text).map_err(|error| {
                record_error(id, format!("{context}.path is not canonical: {error}"))
            })?;
            let section = required_string(id, &context, object, "section")?;
            if section.is_empty() {
                return Err(record_error(id, format!("{context}.section is empty")));
            }
            let optional_nonempty = |field: &str| -> Result<Option<String>, FixtureError> {
                let Some(value) = object.get(field) else {
                    return Ok(None);
                };
                let value = value.as_str().ok_or_else(|| {
                    record_error(id, format!("{context}.{field} is not a string"))
                })?;
                if value.is_empty() {
                    return Err(record_error(id, format!("{context}.{field} is empty")));
                }
                Ok(Some(value.to_owned()))
            };
            let rule_id = optional_nonempty("rule_id")?;
            if rule_id.as_deref().is_some_and(|rule| {
                rule.strip_prefix("DFV1-MUST-").is_none_or(|suffix| {
                    suffix.len() != 3 || !suffix.bytes().all(|byte| byte.is_ascii_digit())
                })
            }) {
                return Err(record_error(
                    id,
                    format!("{context}.rule_id is not a traceability rule id"),
                ));
            }
            if let Some(rule_id) = rule_id.as_deref()
                && !traceability_rule_exists(rule_id)?
            {
                return Err(record_error(
                    id,
                    format!("{context}.rule_id is not present in traceability.json"),
                ));
            }
            Ok(NormativeReference {
                path,
                section: section.to_owned(),
                clause: optional_nonempty("clause")?,
                rule_id,
            })
        })
        .collect()
}

fn traceability_rule_exists(rule_id: &str) -> Result<bool, FixtureError> {
    let bytes = fs::read(contract_directory().join("traceability.json"))?;
    let manifest: Value = serde_json::from_slice(&bytes)?;
    Ok(manifest["rules"]
        .as_array()
        .is_some_and(|rules| rules.iter().any(|rule| rule["id"] == rule_id)))
}

fn expect_object<'a>(
    id: &str,
    context: &str,
    value: &'a Value,
) -> Result<&'a Map<String, Value>, FixtureError> {
    value
        .as_object()
        .ok_or_else(|| record_error(id, format!("{context} is not an object")))
}

fn expect_array<'a>(
    id: &str,
    context: &str,
    value: &'a Value,
) -> Result<&'a [Value], FixtureError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| record_error(id, format!("{context} is not an array")))
}

fn validate_byte_field(
    id: &str,
    context: &str,
    object: &Map<String, Value>,
    field: &str,
) -> Result<Vec<u8>, FixtureError> {
    let value = object
        .get(field)
        .ok_or_else(|| record_error(id, format!("{context} is missing field {field}")))?;
    validate_byte_string(value).ok_or_else(|| {
        record_error(
            id,
            format!("{context}.{field} is not a canonical byte string"),
        )
    })
}

fn validate_string_fields(
    id: &str,
    context: &str,
    object: &Map<String, Value>,
    fields: &[&str],
) -> Result<(), FixtureError> {
    for field in fields {
        required_string(id, context, object, field)?;
    }
    Ok(())
}

fn validate_repository_state(id: &str, value: &Value) -> Result<(), FixtureError> {
    const FIELDS: &[&str] = &[
        "git_index",
        "tracked_ignores",
        "worktree",
        "no_follow_metadata",
        "symlink_targets",
    ];
    let object = expect_object(id, "repository_state", value)?;
    validate_fields(id, "repository_state", object, FIELDS, &[])?;
    for field in FIELDS {
        let records = expect_array(id, &format!("repository_state.{field}"), &object[*field])?;
        let mut previous_path: Option<Vec<u8>> = None;
        for (index, record) in records.iter().enumerate() {
            let context = format!("repository_state.{field}[{index}]");
            let record = expect_object(id, &context, record)?;
            let path = match *field {
                "git_index" | "worktree" => {
                    validate_fields(
                        id,
                        &context,
                        record,
                        &["path", "object_type", "mode", "bytes"],
                        &[],
                    )?;
                    validate_string_fields(id, &context, record, &["object_type", "mode"])?;
                    validate_byte_field(id, &context, record, "bytes")?;
                    validate_byte_field(id, &context, record, "path")?
                }
                "tracked_ignores" => {
                    validate_fields(id, &context, record, &["path", "bytes"], &[])?;
                    validate_byte_field(id, &context, record, "bytes")?;
                    validate_byte_field(id, &context, record, "path")?
                }
                "no_follow_metadata" => {
                    validate_fields(
                        id,
                        &context,
                        record,
                        &[
                            "path",
                            "object_type",
                            "device",
                            "inode",
                            "generation",
                            "mode",
                        ],
                        &[],
                    )?;
                    validate_string_fields(
                        id,
                        &context,
                        record,
                        &["object_type", "device", "inode", "generation", "mode"],
                    )?;
                    validate_byte_field(id, &context, record, "path")?
                }
                "symlink_targets" => {
                    validate_fields(id, &context, record, &["path", "target_bytes"], &[])?;
                    validate_byte_field(id, &context, record, "target_bytes")?;
                    validate_byte_field(id, &context, record, "path")?
                }
                _ => unreachable!(),
            };
            if previous_path
                .as_ref()
                .is_some_and(|previous| previous >= &path)
            {
                return Err(record_error(
                    id,
                    format!(
                        "repository_state.{field} is not strictly sorted by decoded path bytes"
                    ),
                ));
            }
            previous_path = Some(path);
        }
    }
    Ok(())
}

fn validate_machine_state(id: &str, value: &Value) -> Result<(), FixtureError> {
    const FIELDS: &[&str] = &[
        "profile",
        "host",
        "variants",
        "os",
        "architecture",
        "filesystem",
        "state_files",
        "destinations",
        "accounts",
        "capabilities",
    ];
    let object = expect_object(id, "machine_state", value)?;
    validate_fields(id, "machine_state", object, FIELDS, &[])?;
    validate_string_fields(
        id,
        "machine_state",
        object,
        &["profile", "host", "os", "architecture"],
    )?;

    let variants = expect_object(id, "machine_state.variants", &object["variants"])?;
    if variants.values().any(|value| !value.is_string()) {
        return Err(record_error(
            id,
            "machine_state.variants values must be strings",
        ));
    }

    let filesystem = expect_object(id, "machine_state.filesystem", &object["filesystem"])?;
    validate_fields(
        id,
        "machine_state.filesystem",
        filesystem,
        &["kind", "case_sensitivity", "volume_identity"],
        &[],
    )?;
    validate_string_fields(
        id,
        "machine_state.filesystem",
        filesystem,
        &["kind", "case_sensitivity", "volume_identity"],
    )?;

    for (index, record) in expect_array(id, "machine_state.state_files", &object["state_files"])?
        .iter()
        .enumerate()
    {
        let context = format!("machine_state.state_files[{index}]");
        let record = expect_object(id, &context, record)?;
        validate_fields(id, &context, record, &["path", "bytes"], &[])?;
        required_string(id, &context, record, "path")?;
        validate_byte_field(id, &context, record, "bytes")?;
    }

    for (index, record) in expect_array(id, "machine_state.destinations", &object["destinations"])?
        .iter()
        .enumerate()
    {
        let context = format!("machine_state.destinations[{index}]");
        let record = expect_object(id, &context, record)?;
        validate_fields(
            id,
            &context,
            record,
            &[
                "path",
                "object_type",
                "bytes",
                "mode",
                "owner",
                "group",
                "token",
            ],
            &[],
        )?;
        validate_byte_field(id, &context, record, "path")?;
        let contents = validate_byte_field(id, &context, record, "bytes")?;
        validate_string_fields(
            id,
            &context,
            record,
            &["object_type", "mode", "owner", "group"],
        )?;
        let object_type = required_string(id, &context, record, "object_type")?;
        let token = expect_object(id, &format!("{context}.token"), &record["token"])?;
        if object_type == "absent" {
            if !contents.is_empty()
                || ["mode", "owner", "group"].iter().any(|field| {
                    !required_string(id, &context, record, field)
                        .unwrap_or_default()
                        .is_empty()
                })
                || !token.is_empty()
            {
                return Err(record_error(
                    id,
                    format!("{context}: absent destination fields must be empty"),
                ));
            }
        } else {
            validate_fields(
                id,
                &format!("{context}.token"),
                token,
                &["volume", "file", "generation"],
                &[],
            )?;
            validate_string_fields(
                id,
                &format!("{context}.token"),
                token,
                &["volume", "file", "generation"],
            )?;
        }
    }

    for (index, record) in expect_array(id, "machine_state.accounts", &object["accounts"])?
        .iter()
        .enumerate()
    {
        let context = format!("machine_state.accounts[{index}]");
        let record = expect_object(id, &context, record)?;
        validate_fields(
            id,
            &context,
            record,
            &["name", "uid", "primary_group", "groups"],
            &[],
        )?;
        validate_string_fields(id, &context, record, &["name", "uid", "primary_group"])?;
        if expect_array(id, &format!("{context}.groups"), &record["groups"])?
            .iter()
            .any(|value| !value.is_string())
        {
            return Err(record_error(
                id,
                format!("{context}.groups contains a non-string"),
            ));
        }
    }

    let capabilities = expect_object(id, "machine_state.capabilities", &object["capabilities"])?;
    validate_fields(
        id,
        "machine_state.capabilities",
        capabilities,
        &["create_no_replace", "guarded_replace", "guarded_prune"],
        &[],
    )?;
    if ["create_no_replace", "guarded_replace", "guarded_prune"]
        .iter()
        .any(|field| !capabilities[*field].is_boolean())
    {
        return Err(record_error(
            id,
            "machine_state.capabilities fields must be booleans",
        ));
    }
    Ok(())
}

const CHANNELS: &[&str] = &[
    "tokens",
    "cst",
    "diagnostics",
    "ir",
    "lock_text",
    "lock_jcs",
    "edits",
    "cli",
    "lsp",
    "tree_sitter",
    "filesystem_mutations",
];

fn validate_expected(id: &str, value: &Value) -> Result<(), FixtureError> {
    let object = expect_object(id, "expected", value)?;
    validate_fields(id, "expected", object, CHANNELS, &[])?;
    for channel in CHANNELS {
        let context = format!("expected.{channel}");
        let record = expect_object(id, &context, &object[*channel])?;
        validate_fields(
            id,
            &context,
            record,
            &["state", "comparison", "payload"],
            &[],
        )?;
        let state = required_string(id, &context, record, "state")?;
        let comparison = required_string(id, &context, record, "comparison")?;
        let payload = &record["payload"];
        let valid = match state {
            "not_applicable" => {
                comparison == "none" && payload.as_object().is_some_and(Map::is_empty)
            }
            "empty" => comparison == "empty" && is_natural_empty(payload),
            "golden" => {
                matches!(comparison, "exact_bytes" | "exact_json" | "semantic_json")
                    && (comparison != "exact_bytes" || validate_byte_string(payload).is_some())
            }
            "property" => comparison == "property",
            "oracle" => comparison == "oracle",
            _ => false,
        };
        if !valid {
            return Err(record_error(
                id,
                format!("{context} has invalid state/comparison/payload combination"),
            ));
        }
    }
    Ok(())
}

fn is_natural_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        _ => false,
    }
}

fn validate_byte_string(record: &Value) -> Option<Vec<u8>> {
    let object = record.as_object()?;
    if object.len() != 2 || !object.contains_key("encoding") || !object.contains_key("value") {
        return None;
    }
    let encoding = object.get("encoding")?.as_str()?;
    let value = object.get("value")?.as_str()?;
    match encoding {
        "utf8" => Some(value.as_bytes().to_vec()),
        "hex"
            if value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            decode_hex(value)
        }
        "base64" => decode_base64(value),
        _ => None,
    }
}

/// Decodes a contract byte string: `{ "encoding": "utf8" | "base64" | "hex",
/// "value": ... }` decoded exactly, without newline or Unicode
/// normalization.
pub fn decode_byte_string(record: &Value) -> Option<Vec<u8>> {
    validate_byte_string(record)
}

/// Encodes a byte string using the readable-when-possible representation:
/// valid UTF-8 text uses `utf8`, anything else uses `base64`.
pub fn encode_byte_string(bytes: &[u8]) -> Value {
    if let Ok(text) = std::str::from_utf8(bytes) {
        serde_json::json!({ "encoding": "utf8", "value": text })
    } else {
        serde_json::json!({ "encoding": "base64", "value": encode_base64(bytes) })
    }
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(value.get(index..index + 2)?, 16).ok())
        .collect()
}

fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut output = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = index + 1 == bytes.len() / 4;
        let mut digits = [0u8; 4];
        let mut padding = 0usize;
        for (position, byte) in chunk.iter().copied().enumerate() {
            digits[position] = match byte {
                b'A'..=b'Z' => u32::from(byte - b'A'),
                b'a'..=b'z' => u32::from(byte - b'a') + 26,
                b'0'..=b'9' => u32::from(byte - b'0') + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if last && position >= 2 => {
                    padding += 1;
                    0
                }
                _ => return None,
            } as u8;
        }
        if (padding == 1 && chunk[3] != b'=')
            || (padding == 2 && &chunk[2..] != b"==")
            || padding > 2
            || (padding == 2 && digits[1] & 0x0f != 0)
            || (padding == 1 && digits[2] & 0x03 != 0)
        {
            return None;
        }
        let accumulator = (u32::from(digits[0]) << 18)
            | (u32::from(digits[1]) << 12)
            | (u32::from(digits[2]) << 6)
            | u32::from(digits[3]);
        output.push((accumulator >> 16) as u8);
        if padding < 2 {
            output.push((accumulator >> 8) as u8);
        }
        if padding < 1 {
            output.push(accumulator as u8);
        }
    }
    Some(output)
}

fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let mut accumulator = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            accumulator |= u32::from(*byte) << (16 - index * 8);
        }
        for position in 0..4 {
            let beyond = position > chunk.len();
            if beyond {
                output.push('=');
            } else {
                output.push(ALPHABET[(accumulator >> (18 - position * 6)) as usize & 0x3f] as char);
            }
        }
    }
    output
}

/// One channel mismatch or fixture-level failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureFailure {
    pub id: String,
    pub channel: String,
    pub message: String,
}

impl Display for FixtureFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "fixture {} channel {}: {}",
            self.id, self.channel, self.message
        )
    }
}

/// Runs one fixture and returns every channel mismatch.
pub fn run_fixture(fixture: &Fixture) -> Vec<FixtureFailure> {
    let path = &fixture.path;
    let source = SourceText::from_bytes(fixture.input.clone());
    let mut failures = Vec::new();
    match fixture.operation.as_str() {
        "lex" => {
            let lexed = lex(path, &source);
            compare_bytes(
                fixture,
                &mut failures,
                "tokens",
                lexed.dump(source.as_bytes()).as_bytes(),
            );
            compare_diagnostics(fixture, &mut failures, &lexed.diagnostics);
        }
        "parse" => {
            let result = parse(path, &source);
            let cst = result.cst();
            compare_bytes(
                fixture,
                &mut failures,
                "tokens",
                dump_tokens(cst.tokens(), cst.gaps(), cst.strings(), source.as_bytes()).as_bytes(),
            );
            compare_bytes(
                fixture,
                &mut failures,
                "cst",
                cst.dump(source.as_bytes()).as_bytes(),
            );
            compare_diagnostics(fixture, &mut failures, result.diagnostics());
        }
        "bootstrap" => {
            let diagnostics = match read_bootstrap(path, &source) {
                Ok(_) => Vec::new(),
                Err(errors) => errors,
            };
            compare_diagnostics(fixture, &mut failures, &diagnostics);
        }
        "lower" | "schema" if is_theme_domain(&fixture.domain) => {
            let result = lower_theme_file(path, &source);
            compare_diagnostics(fixture, &mut failures, result.diagnostics());
            if fixture.operation == "lower" {
                compare_json(fixture, &mut failures, "ir", &result.dump_json());
            }
        }
        "lower" | "schema" => {
            let parsed = parse(path, &source);
            let result = match lower_path(path, &source, &parsed, &fixture_classifier()) {
                Ok(result) => result,
                Err(LoweringError::MismatchedParse) => {
                    failures.push(FixtureFailure {
                        id: fixture.id.clone(),
                        channel: "operation".to_owned(),
                        message: "schema lowering rejected its matching parse".to_owned(),
                    });
                    return failures;
                }
            };
            if fixture_requires_invalid_schema_seal(&fixture.id)
                && result.clone().into_validated(&parsed).is_ok()
            {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: "operation".to_owned(),
                    message: "poisoned schema HIR incorrectly sealed as validated".to_owned(),
                });
            }
            let mut diagnostics = parsed.diagnostics().to_vec();
            diagnostics.extend(result.diagnostics().iter().cloned());
            sort_diagnostics(&mut diagnostics);
            compare_diagnostics(fixture, &mut failures, &diagnostics);
            if fixture.operation == "lower" {
                compare_json(fixture, &mut failures, "ir", &result.dump_json());
            }
        }
        "format" => run_format_fixture(fixture, path, &source, &mut failures),
        other => failures.push(FixtureFailure {
            id: fixture.id.clone(),
            channel: "operation".to_owned(),
            message: format!("unknown operation {other}"),
        }),
    }
    failures
}

/// Replaces the golden payloads of one M2 fixture with the implementation's
/// current exact results. Empty expectations remain assertions, formatter
/// idempotence is checked, and the record is promoted to `passing` only when
/// those assertions hold. This function returns JSON and never writes files.
pub fn materialize_m2_fixture(path: &Path) -> Result<Value, FixtureError> {
    let fixture = load_fixture(path)?;
    if !matches!(fixture.operation.as_str(), "lower" | "schema" | "format") {
        return Err(record_error(
            &fixture.id,
            "materialization is limited to M2 lower/schema/format fixtures",
        ));
    }
    let mut record: Value = serde_json::from_slice(&fs::read(path)?)?;
    let source = SourceText::from_bytes(fixture.input.clone());

    match fixture.operation.as_str() {
        "lower" | "schema" if is_theme_domain(&fixture.domain) => {
            let result = lower_theme_file(&fixture.path, &source);
            materialize_diagnostics(&fixture.id, &mut record, result.diagnostics())?;
            if fixture.operation == "lower" {
                materialize_json_channel(&fixture.id, &mut record, "ir", result.dump_json())?;
            }
        }
        "lower" | "schema" => {
            let parsed = parse(&fixture.path, &source);
            let result = lower_path(&fixture.path, &source, &parsed, &fixture_classifier())
                .map_err(|LoweringError::MismatchedParse| {
                    record_error(&fixture.id, "schema lowering rejected its matching parse")
                })?;
            if fixture_requires_invalid_schema_seal(&fixture.id)
                && result.clone().into_validated(&parsed).is_ok()
            {
                return Err(record_error(
                    &fixture.id,
                    "poisoned schema HIR incorrectly sealed as validated",
                ));
            }
            let mut diagnostics = parsed.diagnostics().to_vec();
            diagnostics.extend(result.diagnostics().iter().cloned());
            sort_diagnostics(&mut diagnostics);
            materialize_diagnostics(&fixture.id, &mut record, &diagnostics)?;
            if fixture.operation == "lower" {
                materialize_json_channel(&fixture.id, &mut record, "ir", result.dump_json())?;
            }
        }
        "format" => materialize_format_fixture(&fixture, &source, &mut record)?,
        _ => unreachable!(),
    }
    record["status"] = Value::String("passing".to_owned());
    validate_passing_obligations(&fixture.id, &record["expected"])?;
    Ok(record)
}

fn materialize_format_fixture(
    fixture: &Fixture,
    source: &SourceText,
    record: &mut Value,
) -> Result<(), FixtureError> {
    let classifier = fixture_classifier();
    match format_source(&fixture.path, source, &classifier) {
        Ok(output) => {
            materialize_diagnostics(&fixture.id, record, &[])?;
            materialize_bytes_channel(&fixture.id, record, "edits", &output.bytes)?;
            let formatted = SourceText::from_bytes(output.bytes.clone());
            let second =
                format_source(&fixture.path, &formatted, &classifier).map_err(|error| {
                    record_error(
                        &fixture.id,
                        format!("formatted output was refused on its second pass: {error}"),
                    )
                })?;
            if second.changed || second.bytes != output.bytes {
                return Err(record_error(&fixture.id, "formatter is not idempotent"));
            }
        }
        Err(FormatError::InvalidSyntax { diagnostics }) => {
            materialize_diagnostics(&fixture.id, record, &diagnostics)?;
            materialize_json_channel(
                &fixture.id,
                record,
                "edits",
                serde_json::json!({ "error": "invalid_syntax" }),
            )?;
        }
        Err(FormatError::GeneratedLockReadOnly) => {
            materialize_diagnostics(&fixture.id, record, &[])?;
            materialize_json_channel(
                &fixture.id,
                record,
                "edits",
                serde_json::json!({ "error": "generated_lock_read_only" }),
            )?;
        }
        Err(FormatError::UnsupportedDomain { domain }) => {
            materialize_diagnostics(&fixture.id, record, &[])?;
            materialize_json_channel(
                &fixture.id,
                record,
                "edits",
                serde_json::json!({ "error": "unsupported_domain", "domain": domain.as_str() }),
            )?;
        }
        Err(FormatError::MismatchedParse) => {
            return Err(record_error(
                &fixture.id,
                "formatter reported a mismatched parse for its own source",
            ));
        }
        Err(FormatError::UnclassifiedPath { path }) => {
            materialize_diagnostics(&fixture.id, record, &[])?;
            materialize_json_channel(
                &fixture.id,
                record,
                "edits",
                serde_json::json!({
                    "error": "unclassified_path",
                    "path": path.as_str(),
                }),
            )?;
        }
        Err(error @ FormatError::SchemaMismatch { .. }) => {
            return Err(record_error(
                &fixture.id,
                format!("formatter path classification failed: {error}"),
            ));
        }
    }
    Ok(())
}

fn materialize_diagnostics(
    id: &str,
    record: &mut Value,
    diagnostics: &[dotfile_source::Diagnostic],
) -> Result<(), FixtureError> {
    let state = record["expected"]["diagnostics"]["state"].as_str();
    if state == Some("golden") && diagnostics.is_empty() {
        return Err(record_error(id, "negative fixture emitted no diagnostics"));
    }
    let actual = serde_json::to_value(diagnostics).map_err(FixtureError::Json)?;
    materialize_json_channel(id, record, "diagnostics", actual)
}

fn materialize_json_channel(
    id: &str,
    record: &mut Value,
    channel: &str,
    actual: Value,
) -> Result<(), FixtureError> {
    let expected = record
        .get_mut("expected")
        .and_then(Value::as_object_mut)
        .and_then(|expected| expected.get_mut(channel))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| record_error(id, format!("missing expected channel {channel}")))?;
    match expected.get("state").and_then(Value::as_str) {
        Some("golden") => {
            expected.insert("payload".to_owned(), actual);
            Ok(())
        }
        Some("empty") if is_natural_empty(&actual) => Ok(()),
        Some("not_applicable") if is_natural_empty(&actual) => Ok(()),
        Some(state) => Err(record_error(
            id,
            format!(
                "channel {channel} state {state} rejected actual {}",
                pretty(&actual)
            ),
        )),
        None => Err(record_error(id, format!("channel {channel} has no state"))),
    }
}

fn materialize_bytes_channel(
    id: &str,
    record: &mut Value,
    channel: &str,
    actual: &[u8],
) -> Result<(), FixtureError> {
    let value = encode_byte_string(actual);
    materialize_json_channel(id, record, channel, value)
}

fn fixture_classifier() -> DomainClassifier {
    let layout = GroupLayout::try_new([GroupLayoutEntry {
        group: "shared".to_owned(),
        directory: RepoPath::new("shared").expect("static fixture group directory"),
    }])
    .expect("static fixture group layout");
    DomainClassifier::new(layout)
}

fn run_format_fixture(
    fixture: &Fixture,
    path: &RepoPath,
    source: &SourceText,
    failures: &mut Vec<FixtureFailure>,
) {
    let classifier = fixture_classifier();
    match format_source(path, source, &classifier) {
        Ok(output) => {
            compare_diagnostics(fixture, failures, &[]);
            compare_bytes(fixture, failures, "edits", &output.bytes);
            let formatted_source = SourceText::from_bytes(output.bytes.clone());
            match format_source(path, &formatted_source, &classifier) {
                Ok(second) if second.bytes == output.bytes && !second.changed => {}
                Ok(second) => failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: "edits".to_owned(),
                    message: format!(
                        "formatter is not idempotent: second pass changed={} and produced different_bytes={}",
                        second.changed,
                        second.bytes != output.bytes,
                    ),
                }),
                Err(error) => failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: "edits".to_owned(),
                    message: format!("formatted output was refused on its second pass: {error}"),
                }),
            }
        }
        Err(FormatError::InvalidSyntax { diagnostics }) => {
            compare_diagnostics(fixture, failures, &diagnostics);
            compare_json(
                fixture,
                failures,
                "edits",
                &serde_json::json!({ "error": "invalid_syntax" }),
            );
        }
        Err(FormatError::GeneratedLockReadOnly) => {
            compare_diagnostics(fixture, failures, &[]);
            compare_json(
                fixture,
                failures,
                "edits",
                &serde_json::json!({ "error": "generated_lock_read_only" }),
            );
        }
        Err(FormatError::UnsupportedDomain { domain }) => {
            compare_diagnostics(fixture, failures, &[]);
            compare_json(
                fixture,
                failures,
                "edits",
                &serde_json::json!({ "error": "unsupported_domain", "domain": domain.as_str() }),
            );
        }
        Err(FormatError::MismatchedParse) => failures.push(FixtureFailure {
            id: fixture.id.clone(),
            channel: "edits".to_owned(),
            message: "formatter reported a mismatched parse for its own source".to_owned(),
        }),
        Err(FormatError::UnclassifiedPath { path }) => {
            compare_diagnostics(fixture, failures, &[]);
            compare_json(
                fixture,
                failures,
                "edits",
                &serde_json::json!({
                    "error": "unclassified_path",
                    "path": path.as_str(),
                }),
            );
        }
        Err(error @ FormatError::SchemaMismatch { .. }) => {
            failures.push(FixtureFailure {
                id: fixture.id.clone(),
                channel: "operation".to_owned(),
                message: format!("formatter path classification failed: {error}"),
            });
        }
    }
}

fn is_theme_domain(domain: &str) -> bool {
    matches!(
        domain,
        "theme_roles"
            | "theme_fonts"
            | "theme_map_catppuccin"
            | "theme_map_eza"
            | "theme_map_gtk"
            | "theme_map_kde"
            | "theme_map_obsidian"
            | "theme_profiles"
            | "theme_profile_poison_path"
    )
}

fn fixture_requires_invalid_schema_seal(id: &str) -> bool {
    matches!(
        id,
        "f03-schema-bootstrap-sealing-matrix" | "f03-schema-tolerant-recovery"
    )
}

/// Returns the exact or representative repository path for a frozen fixture
/// domain. Dynamic group-layout domains use the `shared` group, `wezterm`
/// facet, and `laptop` variant consistently across the corpus.
pub fn representative_fixture_path(domain: &str) -> Option<RepoPath> {
    let path = match domain {
        "generic" => "fixture.dotfile",
        "unknown_repository" => "unknown.dotfile",
        "unknown_dotfile_format_path" => "config/extra.dotfile",
        "not_dotfile" => "notes.txt",
        "profiles" => dotfile_source::PROFILES_PATH,
        "hosts" => "config/hosts.dotfile",
        "group_root_requirements" => "shared/package.dotfile",
        "facet_requirements" => "shared/wezterm/package.dotfile",
        "facet_ident_path" => "shared/7-zip.v2/package.dotfile",
        "override_variant" => "shared/overrides/laptop/wezterm/package.dotfile",
        "override_variant_ident_path" => "shared/overrides/wayland.v2/7-zip/package.dotfile",
        "recipient_keys" => "config/keys.dotfile",
        "secret_scan_rules" => "config/scan.dotfile",
        "benchmark_baselines" => "benchmarks/baselines.dotfile",
        "theme_roles" => "theme/roles.dotfile",
        "theme_fonts" => "theme/fonts.dotfile",
        "theme_map_catppuccin" => "theme/maps/catppuccin.dotfile",
        "theme_map_eza" => "theme/maps/eza.dotfile",
        "theme_map_gtk" => "theme/maps/gtk.dotfile",
        "theme_map_kde" => "theme/maps/kde.dotfile",
        "theme_map_obsidian" => "theme/maps/obsidian.dotfile",
        "theme_profiles" => "theme/profiles/mocha.dotfile",
        "theme_profile_ident_path" => "theme/profiles/7-dark.dotfile",
        "theme_profile_poison_path" => "theme/profiles/poison.dotfile",
        "template_variables" => "vars.enc.yaml",
        "generated_lock" => "package.lock.dotfile",
        _ => return None,
    };
    Some(RepoPath::new(path).expect("fixture paths are statically valid"))
}

/// Concise alias for [`representative_fixture_path`].
pub fn fixture_path(domain: &str) -> Option<RepoPath> {
    representative_fixture_path(domain)
}

fn channel<'a>(fixture: &'a Fixture, name: &str) -> &'a Value {
    &fixture.expected[name]
}

fn compare_bytes(fixture: &Fixture, failures: &mut Vec<FixtureFailure>, name: &str, actual: &[u8]) {
    let channel = channel(fixture, name);
    match channel["state"].as_str() {
        Some("golden") => {
            let Some(expected) = decode_byte_string(&channel["payload"]) else {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: name.to_owned(),
                    message: "golden payload is not a valid byte string".to_owned(),
                });
                return;
            };
            if expected != actual {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: name.to_owned(),
                    message: format!(
                        "golden mismatch\n--- expected ---\n{}\n--- actual ---\n{}",
                        String::from_utf8_lossy(&expected),
                        String::from_utf8_lossy(actual),
                    ),
                });
            }
        }
        Some("empty") => {
            if !actual.is_empty() {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: name.to_owned(),
                    message: "expected an empty result".to_owned(),
                });
            }
        }
        Some("not_applicable") => {}
        _ => {}
    }
}

fn compare_diagnostics(
    fixture: &Fixture,
    failures: &mut Vec<FixtureFailure>,
    actual: &[dotfile_source::Diagnostic],
) {
    let channel = channel(fixture, "diagnostics");
    let actual_json = serde_json::to_value(actual).unwrap_or(Value::Null);
    match channel["state"].as_str() {
        Some("golden") => {
            if channel["payload"] != actual_json {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: "diagnostics".to_owned(),
                    message: format!(
                        "golden mismatch\n--- expected ---\n{}\n--- actual ---\n{}",
                        pretty(&channel["payload"]),
                        pretty(&actual_json),
                    ),
                });
            }
        }
        Some("empty") => {
            if !actual.is_empty() {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: "diagnostics".to_owned(),
                    message: format!("expected no diagnostics, got {}", pretty(&actual_json)),
                });
            }
        }
        Some("not_applicable") => {}
        _ => {}
    }
}

fn compare_json(fixture: &Fixture, failures: &mut Vec<FixtureFailure>, name: &str, actual: &Value) {
    let expected = channel(fixture, name);
    match expected["state"].as_str() {
        Some("golden") => {
            if expected["payload"] != *actual {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: name.to_owned(),
                    message: format!(
                        "golden mismatch\n--- expected ---\n{}\n--- actual ---\n{}",
                        pretty(&expected["payload"]),
                        pretty(actual),
                    ),
                });
            }
        }
        Some("empty") => {
            if !is_natural_empty(actual) {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: name.to_owned(),
                    message: format!("expected an empty result, got {}", pretty(actual)),
                });
            }
        }
        Some("not_applicable") => {}
        _ => {}
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_string_codecs() {
        assert_eq!(decode_hex("00ff10"), Some(vec![0x00, 0xff, 0x10]));
        assert_eq!(decode_hex("0"), None);
        assert_eq!(decode_hex("zz"), None);

        assert_eq!(decode_base64("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(decode_base64("aGVsbG8"), None);
        assert_eq!(decode_base64("AAAA"), Some(vec![0, 0, 0]));
        assert_eq!(decode_base64("A==="), None);
        assert_eq!(decode_base64("AB=="), None, "non-zero unused bits");
        assert_eq!(decode_base64("AAF="), None, "non-zero unused bits");
        assert_eq!(decode_base64("AA=A"), None, "padding must be terminal");
        assert!(
            decode_byte_string(&serde_json::json!({
                "encoding": "hex",
                "value": "FF"
            }))
            .is_none()
        );
        for case in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            &[0xff, 0xfe, 0xfd],
        ] {
            assert_eq!(decode_base64(&encode_base64(case)), Some(case.to_vec()));
        }

        let encoded = encode_byte_string(b"plain text");
        assert_eq!(encoded["encoding"], "utf8");
        let encoded = encode_byte_string(&[0xff, 0x00]);
        assert_eq!(encoded["encoding"], "base64");
        assert_eq!(decode_byte_string(&encoded), Some(vec![0xff, 0x00]));
    }

    #[test]
    fn representative_paths_cover_every_frozen_domain() {
        let expected = [
            ("profiles", "config/profiles.dotfile"),
            ("hosts", "config/hosts.dotfile"),
            ("group_root_requirements", "shared/package.dotfile"),
            ("facet_requirements", "shared/wezterm/package.dotfile"),
            (
                "override_variant",
                "shared/overrides/laptop/wezterm/package.dotfile",
            ),
            ("recipient_keys", "config/keys.dotfile"),
            ("secret_scan_rules", "config/scan.dotfile"),
            ("benchmark_baselines", "benchmarks/baselines.dotfile"),
            ("theme_roles", "theme/roles.dotfile"),
            ("theme_fonts", "theme/fonts.dotfile"),
            ("theme_map_catppuccin", "theme/maps/catppuccin.dotfile"),
            ("theme_map_eza", "theme/maps/eza.dotfile"),
            ("theme_map_gtk", "theme/maps/gtk.dotfile"),
            ("theme_map_kde", "theme/maps/kde.dotfile"),
            ("theme_map_obsidian", "theme/maps/obsidian.dotfile"),
            ("theme_profiles", "theme/profiles/mocha.dotfile"),
            ("template_variables", "vars.enc.yaml"),
            ("generated_lock", "package.lock.dotfile"),
        ];
        for (domain, path) in expected {
            assert_eq!(representative_fixture_path(domain).unwrap().as_str(), path);
        }
        assert!(representative_fixture_path("not_a_domain").is_none());
    }
}
