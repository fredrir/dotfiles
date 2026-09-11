use super::*;
use serde_json::json;

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn legacy_jsonc_spans_and_three_way_merge_oracle() {
    // Captured from the original Python edge-case tests before deleting that backend.
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../fixtures/merge-legacy.json")).unwrap();
    assert_eq!(cases.len(), 123);
    for (index, case) in cases.iter().enumerate() {
        let args = case["args"].as_array().unwrap();
        let function = case["function"].as_str().unwrap();
        let result: Result<Value, String> = match function {
            "jsonc.loads" => parse_jsonc(args[0].as_str().unwrap()),
            "jsonspan.detect_indent" => Ok(json!(detect_indent(args[0].as_str().unwrap()))),
            "jsonspan.key_span" | "jsonspan.value_span" | "jsonspan.container_span" => {
                let text = args[0].as_str().unwrap();
                let path = strings(&args[1]);
                let span = match function {
                    "jsonspan.key_span" => json_key_span(text, &path),
                    "jsonspan.value_span" => json_value_span(text, &path),
                    _ => json_container_span(text, &path),
                };
                span.map(|span| {
                    json!(span.map(|(start, end)| (
                        text[..start].chars().count(),
                        text[..end].chars().count()
                    )))
                })
            }
            "jsonspan.members" => {
                let text = args[0].as_str().unwrap();
                let offset = |v: &Value| {
                    text.char_indices()
                        .map(|(i, _)| i)
                        .chain(std::iter::once(text.len()))
                        .nth(v.as_u64().unwrap() as usize)
                        .unwrap()
                };
                json_members(text, (offset(&args[1][0]), offset(&args[1][1]))).map(|members| {
                    json!(
                        members
                            .into_iter()
                            .map(|m| (
                                m.key,
                                text[..m.key_start].chars().count(),
                                text[..m.value_start].chars().count(),
                                text[..m.value_end].chars().count()
                            ))
                            .collect::<Vec<_>>()
                    )
                })
            }
            "merge.deep_merge" => Ok(deep_merge(args[0].clone(), args[1].clone())),
            "mergeconf.matches" => Ok(json!(matches_ignore(
                &strings(&args[0]),
                &strings(&args[1])
            ))),
            "merge.resolve" => {
                let ignores = strings(&args[3]);
                let choices = args[4]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(strings)
                    .collect::<HashSet<_>>();
                let mut changes = Vec::new();
                let document = Resolver {
                    tracked: !args[2].is_null(),
                    ignores: &ignores,
                    repo_choices: &choices,
                }
                .walk(
                    Some(&args[0]),
                    Some(&args[1]),
                    (!args[2].is_null()).then_some(&args[2]),
                    &[],
                    &mut changes,
                );
                let kinds = changes
                    .iter()
                    .map(|change| {
                        (
                            change.path.join("/"),
                            match change.kind {
                                ChangeKind::Add => "add",
                                ChangeKind::Modify => "modify",
                                ChangeKind::Delete => "delete",
                                ChangeKind::Conflict => "conflict",
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                Ok(json!([document, kinds]))
            }
            other => panic!("unknown oracle function {other}"),
        };
        if case.get("error").is_some() {
            assert!(result.is_err(), "case {index}: {case}\n{result:?}");
        } else {
            assert_eq!(
                result.unwrap_or_else(|e| panic!("case {index}: {case}\n{e}")),
                case["result"],
                "case {index}: {case}"
            );
        }
    }
}

#[test]
fn merge_directives_union_layers_and_reject_invalid_lines() {
    let temp = tempfile::tempdir().unwrap();
    let directories = [temp.path().join("shared"), temp.path().join("macos")];
    for path in &directories {
        fs::create_dir(path).unwrap();
    }
    assert!(load_ignores(&directories).unwrap().is_empty());
    fs::write(
        directories[0].join("merge.dotfile"),
        "# note\nignore cSpell.*\nignore [lua]/*\n",
    )
    .unwrap();
    fs::write(
        directories[1].join("merge.dotfile"),
        "ignore cSpell.*\nignore shellformat.path\n",
    )
    .unwrap();
    assert_eq!(
        load_ignores(&directories).unwrap(),
        ["cSpell.*", "[lua]/*", "shellformat.path"]
    );
    for invalid in ["keep workbench.colorTheme\n", "ignore\n"] {
        fs::write(directories[0].join("merge.dotfile"), invalid).unwrap();
        assert!(load_ignores(&directories).is_err());
    }
}

#[test]
fn vscode_and_taplo_formatting_settings_match() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let taplo: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("shared/tools/.taplo.toml")).unwrap())
            .unwrap();
    let settings =
        parse_jsonc(&fs::read_to_string(root.join("shared/vscode/settings.json")).unwrap())
            .unwrap();
    let mut mirrored = HashSet::new();
    for (key, value) in taplo["formatting"].as_table().unwrap() {
        let mut words = key.split('_');
        let mut camel = words.next().unwrap().to_string();
        for word in words {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                camel.extend(first.to_uppercase());
                camel.extend(chars);
            }
        }
        let name = format!("evenBetterToml.formatter.{camel}");
        assert_eq!(
            settings[&name],
            serde_json::to_value(value).unwrap(),
            "{name}"
        );
        mirrored.insert(name);
    }
    for name in settings
        .as_object()
        .unwrap()
        .keys()
        .filter(|key| key.starts_with("evenBetterToml.formatter."))
    {
        assert!(
            mirrored.contains(name) || name == "evenBetterToml.formatter.indentString",
            "unmirrored {name}"
        );
    }
}
