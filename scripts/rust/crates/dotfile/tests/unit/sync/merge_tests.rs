use super::*;
use serde_json::json;

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

#[test]
fn jsonc_comments_and_trailing_commas_preserve_string_contents() {
    let text = r#"{
        // comment before a multibyte key
        "føø": {"url": "https://example.org/*path*/",},
        "items": [1, /* middle */ 2,],
    }"#;
    assert_eq!(
        parse_jsonc(text).unwrap(),
        json!({"føø": {"url": "https://example.org/*path*/"}, "items": [1, 2]})
    );
    let path = ["føø".to_owned(), "url".to_owned()];
    let (start, end) = json_value_span(text, &path).unwrap().unwrap();
    assert_eq!(&text[start..end], r#""https://example.org/*path*/""#);
    let (start, end) = json_key_span(text, &path).unwrap().unwrap();
    assert_eq!(&text[start..end], r#""url": "https://example.org/*path*/""#);
}

#[test]
fn jsonc_spans_handle_empty_paths_and_reject_unclosed_containers() {
    for lookup in [json_value_span, json_key_span, json_container_span] {
        assert_eq!(lookup("{}", &[]).unwrap(), None);
        assert!(lookup(r#"{"key": [1, 2]"#, &["key".into()]).is_err());
    }
    assert_eq!(json_value_span("{}", &["missing".into()]).unwrap(), None);
}

#[test]
fn overlays_merge_objects_and_replace_arrays_and_scalars() {
    assert_eq!(
        deep_merge(
            json!({"nested": {"keep": 1, "replace": 2}, "array": [1], "scalar": false}),
            json!({"nested": {"replace": 3}, "array": [2, 3], "scalar": {"new": true}}),
        ),
        json!({"nested": {"keep": 1, "replace": 3}, "array": [2, 3], "scalar": {"new": true}})
    );
}
