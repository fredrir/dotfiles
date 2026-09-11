use super::*;
use serde_json::json;

fn keys(path: &[&str]) -> Vec<String> {
    path.iter().map(|key| (*key).to_owned()).collect()
}

#[test]
fn jsonc_adoption_preserves_all_reference_edit_bytes() {
    // Captured while the original Python assertions passed; no Python runtime is needed.
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../fixtures/jsonc_adoption.json")).unwrap();
    assert_eq!(cases.len(), 59);
    for case in cases {
        let text = case["input"].as_str().unwrap();
        let path: Vec<String> = case["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap().to_owned())
            .collect();
        let result = match case["operation"].as_str().unwrap() {
            "set" => apply_jsonc_set(text, &path, &case["value"]).map(Some),
            "remove" => apply_jsonc_remove(text, &path),
            _ => unreachable!(),
        };
        if case.get("error").is_some() {
            assert!(result.is_err(), "{}: {result:?}", case["case"]);
        } else {
            assert_eq!(
                result.unwrap_or_else(|error| panic!("{}: {error}", case["case"])),
                case["output"].as_str().map(str::to_owned),
                "{}",
                case["case"]
            );
        }
    }
}

#[test]
fn jsonc_adoption_rejects_empty_key_paths() {
    assert!(apply_jsonc_set("{}\n", &[], &json!(1)).is_err());
    assert!(apply_jsonc_remove("{}\n", &[]).is_err());
}

#[test]
fn real_settings_adoption_preserves_current_indent_and_unrelated_bytes() {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../shared/vscode/settings.json");
    let original = fs::read_to_string(source).unwrap();
    let changed =
        apply_jsonc_set(&original, &keys(&["editor.formatOnSave"]), &json!(false)).unwrap();
    assert_eq!(
        changed.replacen(
            "\"editor.formatOnSave\": false",
            "\"editor.formatOnSave\": true",
            1
        ),
        original
    );
    let grown = apply_jsonc_set(
        &original,
        &keys(&["[dotfile-probe]"]),
        &json!({"editor.defaultFormatter":"vendor.probe"}),
    )
    .unwrap();
    assert_eq!(
        parse_jsonc(&grown).unwrap()["[dotfile-probe]"],
        json!({"editor.defaultFormatter":"vendor.probe"})
    );
    assert_eq!(
        apply_jsonc_remove(&grown, &keys(&["[dotfile-probe]"]))
            .unwrap()
            .unwrap(),
        original
    );
    let indent = detect_indent(&original);
    assert!(grown.contains(&format!("{indent}\"[dotfile-probe]\": {{\n{indent}{indent}\"editor.defaultFormatter\": \"vendor.probe\"")));
    let nested = apply_jsonc_set(&original, &keys(&["[lua]", "dotfile-probe"]), &json!(2)).unwrap();
    assert_eq!(
        apply_jsonc_remove(&nested, &keys(&["[lua]", "dotfile-probe"]))
            .unwrap()
            .unwrap(),
        original
    );
}

#[test]
fn tagged_overlay_names_require_nonempty_stem_and_matching_group() {
    for (file, tag, expected) in [
        ("settings.json", "shared", None),
        ("settings.json", "macos", None),
        ("settings.macos.json", "macos", Some("settings.json")),
        ("settings.arch.json", "arch", Some("settings.json")),
        ("settings.linux.json", "macos", None),
        (".macos.json", "macos", None),
        (
            "nested/keybindings.arch.json",
            "arch",
            Some("nested/keybindings.json"),
        ),
    ] {
        assert_eq!(
            overlay_target(Path::new(file), tag),
            expected.map(PathBuf::from)
        );
    }
}

fn entry(root: &Path) -> MergeEntry {
    let shared = root.join("shared/vscode/settings.json");
    let macos = root.join("macos/vscode/settings.macos.json");
    for path in [&shared, &macos] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    fs::write(
        &shared,
        "{\n\t// git\n\t\"editor.formatOnSave\": true,\n\t\"[lua]\": {\"editor.tabSize\":4}\n}\n",
    )
    .unwrap();
    fs::write(
        &macos,
        "{\n\t\"editor.formatOnSave\": false,\n\t\"shellformat.path\": \"/bin/shfmt\"\n}\n",
    )
    .unwrap();
    MergeEntry {
        layers: vec![
            Layer {
                kind: LayerKind::Plain,
                path: shared.clone(),
            },
            Layer {
                kind: LayerKind::Overlay,
                path: macos.clone(),
            },
        ],
        destination: root.join("live/settings.json"),
        ignores: vec![],
        ignore_file: root.join("shared/vscode/merge.dotfile"),
        targets: vec![
            AdoptionTarget {
                label: "shared".into(),
                path: shared,
            },
            AdoptionTarget {
                label: "macos".into(),
                path: macos,
            },
        ],
    }
}

#[test]
fn adoption_ownership_searches_nested_and_literal_keys_from_last_layer() {
    let temp = tempfile::tempdir().unwrap();
    let mut entry = entry(temp.path());
    entry.targets.push(AdoptionTarget {
        label: "arch".into(),
        path: temp.path().join("linux/arch/vscode/settings.arch.json"),
    });
    assert_eq!(
        default_target(&entry, &keys(&["editor.formatOnSave"])).unwrap(),
        1
    );
    assert_eq!(
        default_target(&entry, &keys(&["shellformat.path"])).unwrap(),
        1
    );
    assert_eq!(default_target(&entry, &keys(&["[lua]"])).unwrap(), 0);
    assert_eq!(
        default_target(&entry, &keys(&["[lua]", "editor.tabSize"])).unwrap(),
        0
    );
    assert!(
        value_at(
            &entry_document(&entry).unwrap(),
            &keys(&["editor", "formatOnSave"])
        )
        .is_none()
    );
    assert!(
        value_at(
            &entry_document(&entry).unwrap(),
            &keys(&["[lua]", "editor.insertSpaces"])
        )
        .is_none()
    );
    // The interactive picker highlights the latest existing platform overlay.
    // Explicit headless live adoption separately defaults new keys to shared.
    assert_eq!(default_target(&entry, &keys(&["brand.new"])).unwrap(), 1);
}

#[test]
fn ignore_patterns_preserve_flat_dotted_keys_and_reject_ambiguous_paths() {
    for (path, expected) in [
        (vec!["[lua]", "editor.tabSize"], "[lua]/editor.tabSize"),
        (vec!["cSpell.userWords"], "cSpell.userWords"),
        (vec!["a", "b.c", "d"], "a/b.c/d"),
        (
            vec!["files.associations", "*.zsh"],
            "files.associations/*.zsh",
        ),
    ] {
        assert_eq!(render_ignore_pattern(&keys(&path)).unwrap(), expected);
    }
    for key in ["a/b", "colour#1", "", "a\nb", "a\rb", " padded", "padded\t"] {
        assert!(render_ignore_pattern(&keys(&[key])).is_err(), "{key:?}");
    }
}

#[test]
fn ignore_file_edits_preserve_columns_comments_newlines_and_are_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pkg/merge.dotfile");
    let change = Change {
        kind: ChangeKind::Add,
        path: keys(&["[lua]", "editor.tabSize"]),
        ours: None,
        theirs: Some(json!(2)),
    };
    let pattern = "[lua]/editor.tabSize";
    for (before, after) in [
        ("".to_owned(), format!("ignore  {pattern}\n")),
        (
            "# note\nignore  first.key\n".to_owned(),
            format!("# note\nignore  first.key\nignore  {pattern}\n"),
        ),
        (
            "ignore    first.key\n".to_owned(),
            format!("ignore    first.key\nignore    {pattern}\n"),
        ),
        (
            "ignore\tfirst.key\n".to_owned(),
            format!("ignore\tfirst.key\nignore\t{pattern}\n"),
        ),
        (
            "ignore  first.key".to_owned(),
            format!("ignore  first.key\nignore  {pattern}\n"),
        ),
        (
            "ignore  first.key\r\n".to_owned(),
            format!("ignore  first.key\r\nignore  {pattern}\r\n"),
        ),
        (
            format!("# ignore        {pattern}\n"),
            format!("# ignore        {pattern}\nignore  {pattern}\n"),
        ),
        (
            format!("ignore\t\t{pattern}\n"),
            format!("ignore\t\t{pattern}\n"),
        ),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &before).unwrap();
        add_ignores(&path, std::slice::from_ref(&change)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), after);
        add_ignores(&path, std::slice::from_ref(&change)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), after);
    }
    fs::remove_file(&path).unwrap();
    let bad = Change {
        path: keys(&["a/b"]),
        ..change
    };
    assert!(add_ignores(&path, &[bad]).is_err());
    assert!(!path.exists());
}

#[test]
fn baseline_records_are_hashed_distinct_plain_json_and_preserve_empty_objects() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("repo/config")).unwrap();
    let context = Context::new(
        temp.path().join("repo"),
        temp.path().join("home"),
        temp.path().join("state"),
    )
    .unwrap();
    let first = Path::new("/x/settings.json");
    let second = Path::new("/y/settings.json");
    let digest = format!("{:x}", Sha256::digest(first.as_os_str().as_encoded_bytes()));
    assert_eq!(
        baseline_path(&context, first),
        context.state.join("merge").join(format!("{digest}.json"))
    );
    assert_eq!(load_baseline(&context, first).unwrap(), None);
    let document = json!({"git.autofetch":true,"[lua]":{"editor.tabSize":2}});
    save_baseline(&context, first, &document).unwrap();
    save_baseline(&context, second, &json!({})).unwrap();
    assert_eq!(
        load_baseline(&context, first).unwrap(),
        Some(document.clone())
    );
    assert_eq!(load_baseline(&context, second).unwrap(), Some(json!({})));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(baseline_path(&context, first)).unwrap())
            .unwrap(),
        document
    );
    fs::write(baseline_path(&context, first), "{ truncated").unwrap();
    assert!(
        load_baseline(&context, first).is_err(),
        "corrupt state must be reported before reconciliation writes"
    );
}

#[test]
fn offered_targets_derive_nested_group_names_and_exclude_override_directories() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("config")).unwrap();
    let context = Context::new(
        temp.path().to_owned(),
        temp.path().join("home"),
        temp.path().join("state"),
    )
    .unwrap();
    let configuration = Configuration {
        targets: BTreeMap::new(),
        groups: keys(&[
            "shared",
            "linux/common",
            "linux/arch",
            "macos",
            "macos/overrides/laptop",
        ]),
        active_override_dirs: vec![],
        overrides: BTreeMap::new(),
        packages: vec![],
    };
    let targets = adoption_targets(
        &context,
        &configuration,
        "vscode",
        Path::new("nested/keybindings.json"),
        temp.path().join("shared/vscode/nested/keybindings.json"),
        &[],
    );
    assert_eq!(
        targets
            .iter()
            .map(|target| target.label.as_str())
            .collect::<Vec<_>>(),
        ["shared", "common", "arch", "macos"]
    );
    assert_eq!(
        targets[2].path,
        temp.path()
            .join("linux/arch/vscode/nested/keybindings.arch.json")
    );
    assert_eq!(
        targets[3].path,
        temp.path()
            .join("macos/vscode/nested/keybindings.macos.json")
    );
}

#[test]
fn merge_dry_runs_leave_materialization_adoption_and_baseline_bytes_untouched() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("config")).unwrap();
    let context = Context::new(
        temp.path().to_owned(),
        temp.path().join("home"),
        temp.path().join("state"),
    )
    .unwrap();
    let entry = entry(temp.path());
    let choices = EntryDecisions::default();
    assert!(
        settle(&context, &entry, true, false, Resolution::Skip, &choices)
            .unwrap()
            .changed
    );
    assert!(!entry.destination.exists());
    assert!(!context.state.exists());
    settle(&context, &entry, false, false, Resolution::Skip, &choices).unwrap();
    let baseline = baseline_path(&context, &entry.destination);
    let before = fs::read(&baseline).unwrap();
    let modified = fs::metadata(&baseline).unwrap().modified().unwrap();
    let mut document = entry_document(&entry).unwrap();
    document["new.local"] = json!(14);
    fs::write(&entry.destination, serde_json::to_vec(&document).unwrap()).unwrap();
    let live = fs::read(&entry.destination).unwrap();
    let shared = fs::read(&entry.targets[0].path).unwrap();
    assert!(
        settle(&context, &entry, true, false, Resolution::Live, &choices)
            .unwrap()
            .changed
    );
    assert_eq!(fs::read(&entry.destination).unwrap(), live);
    assert_eq!(fs::read(&entry.targets[0].path).unwrap(), shared);
    assert_eq!(fs::read(&baseline).unwrap(), before);
    assert_eq!(
        fs::metadata(&baseline).unwrap().modified().unwrap(),
        modified
    );
    assert!(
        settle(&context, &entry, true, false, Resolution::Skip, &choices)
            .unwrap()
            .blocked
    );
    settle(&context, &entry, false, false, Resolution::Live, &choices).unwrap();
    let shared = fs::read_to_string(&entry.targets[0].path).unwrap();
    assert!(shared.contains("// git"));
    assert_eq!(parse_jsonc(&shared).unwrap()["new.local"], json!(14));
    assert!(
        parse_jsonc(&fs::read_to_string(&entry.targets[1].path).unwrap())
            .unwrap()
            .get("new.local")
            .is_none()
    );
    let settled = settle(&context, &entry, false, false, Resolution::Skip, &choices).unwrap();
    assert!(!settled.changed);
    assert!(!settled.blocked);
}

#[test]
fn explicit_adoption_creates_missing_overlays_and_missing_removals_are_noops() {
    let temp = tempfile::tempdir().unwrap();
    let mut entry = entry(temp.path());
    let target = temp.path().join("linux/arch/vscode/settings.arch.json");
    entry.targets.push(AdoptionTarget {
        label: "arch".into(),
        path: target.clone(),
    });
    let deletion = Change {
        kind: ChangeKind::Delete,
        path: keys(&["missing"]),
        ours: Some(json!(1)),
        theirs: None,
    };
    adopt_changes(&entry, &[(deletion, Some(2))]).unwrap();
    assert!(!target.exists());
    let addition = Change {
        kind: ChangeKind::Add,
        path: keys(&["editor.fontSize"]),
        ours: None,
        theirs: Some(json!(14)),
    };
    adopt_changes(&entry, &[(addition, Some(2))]).unwrap();
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "{\n\t\"editor.fontSize\": 14\n}\n"
    );
    assert!(
        !fs::read_to_string(&entry.targets[0].path)
            .unwrap()
            .contains("editor.fontSize")
    );
}
