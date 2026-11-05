#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use dotfmt_core::config::{CONFIG_NAME, ConfigOverlay, Effective, Resolver, Selection};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_core::language::Language;
use serde_json::json;

fn config(directory: &Path, source: &str) {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join(CONFIG_NAME), source).unwrap();
}

fn resolver(directory: &Path) -> Resolver {
    Resolver::with_paths(directory.to_path_buf(), None)
}

fn overlay(root: &Path, value: serde_json::Value) -> ConfigOverlay {
    ConfigOverlay::from_json(&value, root).unwrap()
}

fn overlaid_resolver(root: &Path, overlay: ConfigOverlay) -> Resolver {
    Resolver::with_paths_overlay(root.to_path_buf(), None, Some(overlay))
}

#[test]
fn an_unconfigured_workspace_uses_the_shipped_default_language_set() {
    let root = tempfile::tempdir().unwrap();
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    for language in [
        Language::Json,
        Language::Lua,
        Language::TypeScript,
        Language::Css,
        Language::Html,
    ] {
        assert!(
            effective.languages[&language].enabled,
            "{language:?} should be enabled by default"
        );
    }
    assert!(
        !effective.languages[&Language::Javascript].enabled,
        "JavaScript must stay opt-in"
    );
}

#[test]
fn only_global_or_filter_settings_also_use_the_default_languages() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        "{\n width = 100\n}\nexcluded_files { vendor/ }\n",
    );
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    assert!(effective.languages[&Language::Json].enabled);
    assert!(effective.languages[&Language::TypeScript].enabled);
    assert!(!effective.languages[&Language::Javascript].enabled);
}

#[test]
fn an_explicit_language_block_keeps_legacy_subset_semantics() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "json {}\n");
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    assert!(effective.languages[&Language::Json].enabled);
    assert!(!effective.languages[&Language::Lua].enabled);
    assert!(!effective.languages[&Language::TypeScript].enabled);
}

#[test]
fn all_null_native_language_blocks_do_not_suppress_defaults() {
    let root = tempfile::tempdir().unwrap();
    let overlay = overlay(
        root.path(),
        json!({
            "json": {"enabled": null, "indent": null},
            "typescript": {"enabled": null, "quote_style": ""}
        }),
    );
    let effective = overlaid_resolver(root.path(), overlay)
        .for_directory(root.path())
        .unwrap();
    assert!(effective.languages[&Language::Json].enabled);
    assert!(effective.languages[&Language::TypeScript].enabled);
    assert!(!effective.languages[&Language::Javascript].enabled);
}

#[test]
fn native_settings_only_do_not_suppress_default_languages() {
    let root = tempfile::tempdir().unwrap();
    let overlay = overlay(
        root.path(),
        json!({
            "global": {"width": 40},
            "typescript": {"quote_style": "single"}
        }),
    );
    let effective = overlaid_resolver(root.path(), overlay)
        .for_directory(root.path())
        .unwrap();
    assert!(effective.languages[&Language::Json].enabled);
    assert!(effective.languages[&Language::TypeScript].enabled);
    assert_eq!(
        effective.settings(Language::TypeScript)["quote_style"].value,
        "single"
    );
    assert!(!effective.languages[&Language::Javascript].enabled);
}

#[test]
fn nearer_global_overrides_outer_language_and_nearest_language_wins() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let nested = project.join("nested");
    config(
        root.path(),
        "{ width = 80\n quote-style = \"double\" }\nlua { width = 100 }\njson {}\n",
    );
    config(&project, "{ width = 90 }\n");
    config(&nested, "lua { width = 120 }\n");
    let resolver = resolver(root.path());
    let project_config = resolver.for_directory(&project).unwrap();
    assert_eq!(project_config.settings(Language::Lua)["width"].value, "90");
    assert!(project_config.settings(Language::Lua)["width"].global);
    assert_eq!(project_config.settings(Language::Json)["width"].value, "90");
    let nested_config = resolver.for_file(&nested.join("init.lua")).unwrap();
    assert_eq!(nested_config.settings(Language::Lua)["width"].value, "120");
    assert!(!nested_config.settings(Language::Lua)["width"].global);
    assert_eq!(nested_config.settings(Language::Json)["width"].value, "90");
    assert_eq!(
        nested_config.settings(Language::Lua)["quote_style"].value,
        "double"
    );
    assert_eq!(
        nested_config.settings(Language::Lua)["width"].source,
        nested.join(CONFIG_NAME)
    );
}

#[test]
fn installed_settings_and_filters_use_invocation_directory() {
    let root = tempfile::tempdir().unwrap();
    let installed = root.path().join("installed");
    let project = root.path().join("project");
    config(
        &installed,
        "{ width = 80 }\njson {}\nexcluded_files { /skip.json }\n",
    );
    fs::create_dir(&project).unwrap();
    let resolver = Resolver::with_paths(project.clone(), Some(installed.join(CONFIG_NAME)));
    let effective = resolver.for_directory(&project).unwrap();
    assert_eq!(effective.settings(Language::Json)["width"].value, "80");
    assert_eq!(
        effective.select(&project.join("skip.json"), None).unwrap(),
        None
    );
    assert_eq!(
        effective
            .select(&project.join("nested/skip.json"), None)
            .unwrap(),
        Some(Language::Json)
    );
}

#[test]
fn blocks_enable_languages_and_disable_can_be_overridden() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    let deeper = nested.join("deeper");
    config(root.path(), "{ final_newline = false }\nlua {}\nmd {}\n");
    config(&nested, "lua { enabled = false }\n");
    config(&deeper, "lua {}\n");
    let resolver = resolver(root.path());
    let effective = resolver.for_directory(root.path()).unwrap();
    assert!(effective.languages[&Language::Markdown].enabled);
    assert!(!effective.languages[&Language::Conf].enabled);
    assert_eq!(
        effective
            .select(&root.path().join("a.json"), Some(Language::Json))
            .unwrap(),
        None
    );
    let effective = resolver.for_directory(&nested).unwrap();
    assert_eq!(
        effective
            .select(&nested.join("init.lua"), Some(Language::Lua))
            .unwrap(),
        None
    );
    let effective = resolver.for_directory(&deeper).unwrap();
    assert_eq!(
        effective.select(&deeper.join("init.lua"), None).unwrap(),
        Some(Language::Lua)
    );
}

#[test]
fn global_includes_restrict_and_language_includes_add_custom_extensions() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        "conf { include { *.ssh } }\njson {}\nincluded_files {\n settings/**\n !settings/private.*\n}\n",
    );
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    for (file, expected) in [
        ("settings/config.ssh", Some(Language::Conf)),
        ("settings/app.json", Some(Language::Json)),
        ("settings/private.ssh", None),
        ("other.json", None),
        ("settings/a.unknown", None),
    ] {
        assert_eq!(
            effective.select(&root.path().join(file), None).unwrap(),
            expected,
            "{file}"
        );
    }
}

#[test]
fn local_patterns_override_earlier_patterns_and_empty_blocks_reset() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    let reset = nested.join("reset");
    config(
        root.path(),
        "json {}\nexcluded_files { *.json }\nincluded_files { *.json }\n",
    );
    config(&nested, "excluded_files { !keep.json }\n");
    config(&reset, "excluded_files {}\nincluded_files {}\nconf {}\n");
    let resolver = resolver(root.path());
    let effective = resolver.for_directory(&nested).unwrap();
    assert_eq!(
        effective.select(&nested.join("keep.json"), None).unwrap(),
        Some(Language::Json)
    );
    assert_eq!(
        effective.select(&nested.join("other.json"), None).unwrap(),
        None
    );
    let effective = resolver.for_directory(&reset).unwrap();
    assert_eq!(
        effective.select(&reset.join("other.json"), None).unwrap(),
        Some(Language::Json)
    );
    assert_eq!(
        effective.select(&reset.join("app.conf"), None).unwrap(),
        Some(Language::Conf)
    );
}

#[test]
fn excluded_parent_cannot_be_reopened_by_a_negated_file_pattern() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        "json {}\nexcluded_files {\n vendor/\n !vendor/keep.json\n}\n",
    );
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    assert_eq!(
        effective
            .select(&root.path().join("vendor/keep.json"), None)
            .unwrap(),
        None
    );
    assert_eq!(
        effective
            .select(&root.path().join("keep.json"), None)
            .unwrap(),
        Some(Language::Json)
    );
}

#[test]
fn custom_mapping_conflicts_are_reported_and_forced_language_resolves_them() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        "conf { include { *.custom } }\nlua { include { *.custom } }\n",
    );
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    let path = root.path().join("file.custom");
    assert!(
        effective
            .select(&path, None)
            .unwrap_err()
            .to_string()
            .contains("ambiguous language mapping")
    );
    assert_eq!(
        effective.select(&path, Some(Language::Lua)).unwrap(),
        Some(Language::Lua)
    );
}

#[test]
fn gitignore_escaped_characters_and_quoted_settings_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        "conf { # header\n quote_style = 'double' # inline comment\n include {\n \\#settings\n \\!settings\n file{a,b}\n file#name\n file=name\n file\\}\n trailing\\ \n \"literal\"\n } # close filter\n} # close language\n",
    );
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    for file in [
        "#settings",
        "!settings",
        "file{a,b}",
        "file#name",
        "file=name",
        "file}",
        "trailing ",
        "\"literal\"",
    ] {
        assert_eq!(
            effective.select(&root.path().join(file), None).unwrap(),
            Some(Language::Conf),
            "{file}"
        );
    }
    assert_eq!(
        effective.settings(Language::Conf)["quote_style"].value,
        "double"
    );
}

#[test]
fn malformed_configuration_reports_source_and_line() {
    let root = tempfile::tempdir().unwrap();
    for (source, diagnostic) in [
        ("{\n typo = 1\n}", "unknown global setting"),
        ("{ width = 0 }", "positive integer"),
        ("{ final_newline = maybe }", "true or false"),
        ("{ repair = maybe }", "repair must be true or false"),
        ("json {}\njson {}", "duplicate json block"),
        ("bib {}\nbibtex {}", "duplicate bib block"),
        ("env {}\ndotenv {}", "duplicate env block"),
        ("ignore {}\ngitignore {}", "duplicate ignore block"),
        ("ini {}\nini {}", "duplicate ini block"),
        ("plist {}\nplist {}", "duplicate plist block"),
        ("latex {}\ntex {}", "duplicate latex block"),
        ("markdown {}\nmd {}", "duplicate markdown block"),
        ("yaml {}\nyml {}", "duplicate yaml block"),
        ("toml {}\ntoml {}", "duplicate toml block"),
        ("lua { width = 80\nwidth = 90 }", "duplicate setting"),
        ("lua { enabled = perhaps }", "true or false"),
        (
            "lua { enabled = true\nenabled = false }",
            "duplicate setting",
        ),
        ("lua { width = \"80 }", "unterminated quoted value"),
        ("lua {", "unterminated"),
        ("python {}", "unknown language"),
        ("lua { unknown {} }", "unknown nested block"),
    ] {
        config(root.path(), source);
        let error = resolver(root.path())
            .for_directory(root.path())
            .unwrap_err();
        assert!(error.to_string().contains(diagnostic), "{source}: {error}");
        assert!(
            error
                .to_string()
                .contains(&format!("{}:", root.path().join(CONFIG_NAME).display())),
            "{error}"
        );
    }
}

#[test]
fn concurrent_resolution_reuses_an_immutable_snapshot() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "json {}\n");
    let resolver = resolver(root.path());
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    resolver
                        .for_file(&root.path().join("nested/a.json"))
                        .unwrap()
                })
            })
            .collect();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(
            results
                .iter()
                .all(|result| Arc::ptr_eq(result, &results[0]))
        );
        fs::write(root.path().join(CONFIG_NAME), "invalid").unwrap();
        assert!(Arc::ptr_eq(
            &resolver.for_directory(root.path()).unwrap(),
            &results[0]
        ));
    });
}

#[test]
fn inline_pattern_blocks_terminate_after_the_pattern() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "javascript {}\nexcluded_files {*.min.js}\n");
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    assert_eq!(
        effective
            .select(&root.path().join("bundle.min.js"), None)
            .unwrap(),
        None
    );
    assert_eq!(
        effective
            .select(&root.path().join("bundle.js"), None)
            .unwrap(),
        Some(Language::Javascript)
    );
}

#[test]
fn inline_included_patterns_restrict_selection_instead_of_excluding_everything() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "markdown {}\njson {}\nincluded_files {*.md}\n");
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    assert_eq!(
        effective
            .select(&root.path().join("README.md"), None)
            .unwrap(),
        Some(Language::Markdown)
    );
    assert_eq!(
        effective
            .select(&root.path().join("data.json"), None)
            .unwrap(),
        None
    );
}

#[test]
fn an_escaped_brace_inside_an_inline_pattern_stays_literal() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "json {}\nexcluded_files {file\\}.json}\n");
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    assert_eq!(
        effective
            .select(&root.path().join("file}.json"), None)
            .unwrap(),
        None
    );
    assert_eq!(
        effective
            .select(&root.path().join("file.json"), None)
            .unwrap(),
        Some(Language::Json)
    );
}

#[test]
fn an_escaped_space_before_a_brace_remains_part_of_the_pattern() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "conf {\n include {\n name\\ }\n }\n}\n");
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    assert_eq!(
        effective.select(&root.path().join("name }"), None).unwrap(),
        Some(Language::Conf)
    );
    assert_eq!(
        effective.select(&root.path().join("name "), None).unwrap(),
        None
    );
}

#[test]
fn relative_targets_use_the_same_anchored_filters_as_absolute_targets() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    config(&nested, "json {}\nexcluded_files { /a.json }\n");
    let resolver = resolver(root.path());
    let effective = resolver.for_file(Path::new("nested/a.json")).unwrap();
    assert_eq!(
        effective.select(Path::new("nested/a.json"), None).unwrap(),
        None
    );
    assert_eq!(
        effective.select(&nested.join("a.json"), None).unwrap(),
        None
    );
    assert_eq!(
        effective
            .select(Path::new("nested/child/a.json"), None)
            .unwrap(),
        Some(Language::Json)
    );
}

#[test]
fn canceled_path_components_do_not_contribute_configuration_layers() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "lua {}\n");
    config(
        &root.path().join("child"),
        "lua { width = 120 }\nexcluded_files { *.lua }\n",
    );
    let resolver = resolver(root.path());
    let direct = resolver.for_file(Path::new("file.lua")).unwrap();
    let indirect = resolver.for_file(Path::new("child/../file.lua")).unwrap();
    assert!(Arc::ptr_eq(&direct, &indirect));
    assert!(!indirect.settings(Language::Lua).contains_key("width"));
    assert_eq!(
        indirect
            .select(Path::new("child/../file.lua"), None)
            .unwrap(),
        Some(Language::Lua)
    );
}

#[cfg(unix)]
#[test]
fn parent_component_after_a_symlink_uses_the_actual_parent_configuration() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let elsewhere = root.path().join("elsewhere");
    config(&project, "lua { width = 80 }\n");
    config(
        &elsewhere,
        "lua { width = 120 }\nexcluded_files { /skip.lua }\n",
    );
    fs::create_dir(elsewhere.join("child")).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("child"), project.join("link")).unwrap();
    let resolver = resolver(&project);
    let config = resolver.for_file(Path::new("link/../file.lua")).unwrap();
    assert_eq!(config.settings(Language::Lua)["width"].value, "120");
    assert_eq!(
        config.select(Path::new("link/../skip.lua"), None).unwrap(),
        None
    );
}

#[test]
fn requested_languages_disambiguate_without_claiming_unfamiliar_files() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        "conf { include { *.custom } }\nlua { include { *.custom } }\n",
    );
    let effective = resolver(root.path()).for_directory(root.path()).unwrap();
    assert_eq!(
        effective
            .select_languages(Path::new("a.custom"), &[Language::Conf], None)
            .unwrap(),
        Some(Language::Conf)
    );
    assert_eq!(
        effective
            .select_languages(Path::new("a.lua"), &[Language::Conf], None)
            .unwrap(),
        None
    );
    assert_eq!(
        effective
            .select_languages(Path::new("a.unknown"), &[Language::Conf], None)
            .unwrap(),
        None
    );
    assert_eq!(
        effective
            .select_languages(
                Path::new("a.unknown"),
                &[Language::Conf],
                Some(Language::Conf)
            )
            .unwrap(),
        Some(Language::Conf)
    );
}

#[test]
fn selection_explains_skipped_files_and_sources_list_applied_layers() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    let global = root.path().join("global.dotfile");
    fs::write(&global, "json {}\n").unwrap();
    config(
        root.path(),
        "lua { include { !keep.lua } }\nmarkdown { exclude { skip.md } }\nexclude { vendor/ }\n",
    );
    config(&nested, "toml { enabled = false }\n");
    let effective = Resolver::with_paths(root.path().to_path_buf(), Some(global.clone()))
        .for_directory(&nested)
        .unwrap();
    assert_eq!(
        effective.sources(),
        [
            global,
            root.path().join(CONFIG_NAME),
            nested.join(CONFIG_NAME)
        ]
    );
    for (name, expected) in [
        ("a.json", Selection::Language(Language::Json)),
        ("vendor/a.json", Selection::Excluded),
        ("keep.lua", Selection::Excluded),
        ("skip.md", Selection::Excluded),
        ("a.toml", Selection::Disabled(Language::Toml)),
        ("a.yaml", Selection::Disabled(Language::Yaml)),
        ("a.unknown", Selection::Unmapped),
    ] {
        assert_eq!(
            effective
                .selection(&root.path().join(name), &[], None)
                .unwrap(),
            expected,
            "{name}"
        );
    }
}

#[cfg(unix)]
#[test]
fn discovery_preserves_the_supplied_symlink_path() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    let project = root.path().join("project");
    config(&real, "lua { width = 120 }\n");
    config(&project, "lua { width = 80 }\n");
    fs::write(real.join("init.lua"), "return 1").unwrap();
    std::os::unix::fs::symlink(real.join("init.lua"), project.join("init.lua")).unwrap();
    let effective = resolver(root.path())
        .for_file(&project.join("init.lua"))
        .unwrap();
    assert_eq!(effective.settings(Language::Lua)["width"].value, "80");
}

#[test]
fn overlay_global_settings_override_file_layers() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "{ width = 80 }\njson {}\n");
    let source = root.path().join(".vscode/settings.json");
    let value = json!({"global": {"width": 120, "final_newline": false}});
    let mut direct = Effective::default();
    direct.apply_overlay(overlay(root.path(), value.clone()));
    assert_eq!(direct.settings(Language::Lua)["width"].value, "120");
    let effective = overlaid_resolver(root.path(), overlay(root.path(), value))
        .for_directory(root.path())
        .unwrap();
    let width = &effective.settings(Language::Json)["width"];
    assert_eq!(width.value, "120");
    assert!(width.global);
    assert_eq!(width.source, source);
    assert_eq!(width.line, 0);
    assert_eq!(
        effective.settings(Language::Json)["final_newline"].value,
        "false"
    );
    assert_eq!(effective.sources().last(), Some(&source));
}

#[test]
fn overlay_language_settings_override_file_layers() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "{ width = 80 }\nlua {}\n");
    let effective = overlaid_resolver(
        root.path(),
        overlay(
            root.path(),
            json!({"lua": {"width": 100, "quote_style": "single"}}),
        ),
    )
    .for_directory(root.path())
    .unwrap();
    let width = &effective.settings(Language::Lua)["width"];
    assert_eq!(width.value, "100");
    assert!(!width.global);
    assert_eq!(
        effective.settings(Language::Lua)["quote_style"].value,
        "single"
    );
    assert_eq!(effective.settings(Language::Json)["width"].value, "80");
}

#[test]
fn overlay_overrides_nearest_dotfile_once() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    config(root.path(), "{ width = 80 }\nlua {}\n");
    config(&nested, "lua { width = 100 }\n");
    let source = root.path().join(".vscode/settings.json");
    let effective = overlaid_resolver(
        root.path(),
        overlay(root.path(), json!({"lua": {"width": 140}})),
    )
    .for_file(&nested.join("init.lua"))
    .unwrap();
    assert_eq!(effective.settings(Language::Lua)["width"].value, "140");
    assert_eq!(effective.settings(Language::Lua)["width"].source, source);
    assert_eq!(
        effective.sources(),
        [
            root.path().join(CONFIG_NAME),
            nested.join(CONFIG_NAME),
            source,
        ]
    );
}

#[test]
fn overlay_includes_and_excludes_restrict_selection() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "json {}\nconf {}\n");
    let effective = overlaid_resolver(
        root.path(),
        overlay(
            root.path(),
            json!({
                "included_files": ["src/**"],
                "excluded_files": ["vendor/**"],
            }),
        ),
    )
    .for_directory(root.path())
    .unwrap();
    for (file, expected) in [
        ("src/a.json", Some(Language::Json)),
        ("src/vendor/a.json", Some(Language::Json)),
        ("other.json", None),
        ("vendor/b.json", None),
        ("src/a.conf", Some(Language::Conf)),
    ] {
        assert_eq!(
            effective.select(&root.path().join(file), None).unwrap(),
            expected,
            "{file}"
        );
    }
}

#[test]
fn overlay_language_filters_add_extensions_and_empty_arrays_clear() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "lua {}\njson {}\nexcluded_files { *.json }\n");
    let filtered = overlaid_resolver(
        root.path(),
        overlay(root.path(), json!({"lua": {"include": ["*.custom"]}})),
    )
    .for_directory(root.path())
    .unwrap();
    assert_eq!(
        filtered
            .select(&root.path().join("a.custom"), None)
            .unwrap(),
        Some(Language::Lua)
    );
    let cleared = overlaid_resolver(
        root.path(),
        overlay(root.path(), json!({"excluded_files": []})),
    )
    .for_directory(root.path())
    .unwrap();
    assert_eq!(
        cleared.select(&root.path().join("a.json"), None).unwrap(),
        Some(Language::Json)
    );
}

#[test]
fn overlay_enabled_only_changes_when_explicit() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "json {}\nlua {}\n");
    let disabled = overlaid_resolver(
        root.path(),
        overlay(root.path(), json!({"json": {"enabled": false}})),
    )
    .for_directory(root.path())
    .unwrap();
    assert_eq!(
        disabled
            .selection(&root.path().join("a.json"), &[], None)
            .unwrap(),
        Selection::Disabled(Language::Json)
    );

    let disabled_root = tempfile::tempdir().unwrap();
    config(disabled_root.path(), "json { enabled = false }\nlua {}\n");
    let enabled = overlaid_resolver(
        disabled_root.path(),
        overlay(
            disabled_root.path(),
            json!({"json": {"enabled": true, "indent": 3}}),
        ),
    )
    .for_directory(disabled_root.path())
    .unwrap();
    assert_eq!(
        enabled
            .selection(&disabled_root.path().join("a.json"), &[], None)
            .unwrap(),
        Selection::Language(Language::Json)
    );
    assert_eq!(enabled.settings(Language::Json)["indent"].value, "3");
    let settings_only = overlaid_resolver(
        disabled_root.path(),
        overlay(disabled_root.path(), json!({"json": {"indent": 3}})),
    )
    .for_directory(disabled_root.path())
    .unwrap();
    assert_eq!(
        settings_only
            .selection(&disabled_root.path().join("a.json"), &[], None)
            .unwrap(),
        Selection::Disabled(Language::Json)
    );
}

#[test]
fn overlay_converts_numbers_booleans_and_arrays_to_settings() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "typescript {}\n");
    let effective = overlaid_resolver(
        root.path(),
        overlay(
            root.path(),
            json!({
                "typescript": {
                    "width": 100,
                    "remove_unused_exports": true,
                    "tailwind_functions": ["cn", "clsx"],
                    "some_future_option": false,
                }
            }),
        ),
    )
    .for_directory(root.path())
    .unwrap();
    let settings = effective.settings(Language::TypeScript);
    assert_eq!(settings["width"].value, "100");
    assert_eq!(settings["remove_unused_exports"].value, "true");
    assert_eq!(settings["tailwind_functions"].value, "cn,clsx");
    assert_eq!(settings["some_future_option"].value, "false");
    assert_eq!(
        settings["width"].source,
        root.path().join(".vscode/settings.json")
    );
}

#[test]
fn overlay_null_values_are_inherited() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "{ width = 80 }\njson {}\n");
    let empty = ConfigOverlay::from_json(
        &json!({
            "global": {"width": null},
            "lua": null,
            "included_files": null,
        }),
        root.path(),
    )
    .unwrap();
    assert!(empty.is_empty());
    let effective = overlaid_resolver(root.path(), empty)
        .for_directory(root.path())
        .unwrap();
    assert_eq!(effective.settings(Language::Json)["width"].value, "80");
    assert_eq!(effective.sources(), [root.path().join(CONFIG_NAME)]);

    let nulls = ConfigOverlay::from_json(
        &json!({"json": {"indent": null, "enabled": null}}),
        root.path(),
    )
    .unwrap();
    assert!(nulls.is_empty());
    let effective = overlaid_resolver(root.path(), nulls)
        .for_directory(root.path())
        .unwrap();
    assert!(!effective.settings(Language::Json).contains_key("indent"));
    assert_eq!(effective.sources(), [root.path().join(CONFIG_NAME)]);
}

#[test]
fn overlay_empty_strings_are_inherited() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        "css { block_selector_linebreak = always\n quotes = single }\n",
    );
    let empty = ConfigOverlay::from_json(
        &json!({
            "global": {"quote_style": ""},
            "css": {"block_selector_linebreak": "", "quotes": ""},
        }),
        root.path(),
    )
    .unwrap();
    assert!(empty.is_empty());
    let effective = overlaid_resolver(root.path(), empty)
        .for_directory(root.path())
        .unwrap();
    assert_eq!(
        effective.settings(Language::Css)["block_selector_linebreak"].value,
        "always"
    );
    assert_eq!(effective.settings(Language::Css)["quotes"].value, "single");
    assert_eq!(effective.sources(), [root.path().join(CONFIG_NAME)]);
}

#[test]
fn overlay_empty_arrays_are_inherited() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "typescript {}\n");
    let empty = ConfigOverlay::from_json(
        &json!({
            "typescript": {
                "tailwind_functions": [],
                "tailwind_attributes": [null, ""],
                "import_internal": [""],
            }
        }),
        root.path(),
    )
    .unwrap();
    assert!(empty.is_empty());
    let effective = overlaid_resolver(root.path(), empty)
        .for_directory(root.path())
        .unwrap();
    let settings = effective.settings(Language::TypeScript);
    assert!(!settings.contains_key("tailwind_functions"));
    assert!(!settings.contains_key("tailwind_attributes"));
    assert!(!settings.contains_key("import_internal"));
    assert_eq!(effective.sources(), [root.path().join(CONFIG_NAME)]);
}

#[test]
fn overlay_preserves_nonempty_strings_and_arrays() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "css {}\ntypescript {}\n");
    let effective = overlaid_resolver(
        root.path(),
        overlay(
            root.path(),
            json!({
                "css": {"block_selector_linebreak": "wrap", "quotes": "single"},
                "typescript": {"tailwind_functions": ["cn", "clsx"]},
            }),
        ),
    )
    .for_directory(root.path())
    .unwrap();
    assert_eq!(
        effective.settings(Language::Css)["block_selector_linebreak"].value,
        "wrap"
    );
    assert_eq!(effective.settings(Language::Css)["quotes"].value, "single");
    assert_eq!(
        effective.settings(Language::TypeScript)["tailwind_functions"].value,
        "cn,clsx"
    );
}

#[test]
fn overlay_rejects_unknown_blocks_and_invalid_values() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join(".vscode/settings.json");
    for (value, expected) in [
        (json!({"python": {}}), "unknown language"),
        (json!({"width": 80}), "unknown language"),
        (json!([]), "must be an object"),
        (json!({"global": 80}), "global must be an object"),
        (json!({"json": 80}), "json must be an object"),
        (
            json!({"json": {"enabled": "yes"}}),
            "enabled must be true or false",
        ),
        (
            json!({"included_files": "src"}),
            "must be an array of strings",
        ),
        (json!({"json": {"include": [1]}}), "entries must be strings"),
        (
            json!({"json": {"tailwind_functions": [["cn"]]}}),
            "must be a string",
        ),
        (
            json!({"json": {"tailwind_functions": {"cn": true}}}),
            "must be a string",
        ),
    ] {
        let error = ConfigOverlay::from_json(&value, root.path()).unwrap_err();
        assert!(error.to_string().contains(expected), "{value}: {error}");
        assert!(
            error.to_string().contains(&source.display().to_string()),
            "{error}"
        );
        assert_eq!(error.kind, DiagnosticKind::Configuration, "{value}");
    }
}

#[test]
fn overlay_invalid_patterns_are_configuration_diagnostics() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join(".vscode/settings.json");
    let error =
        ConfigOverlay::from_json(&json!({"included_files": ["[z-a]"]}), root.path()).unwrap_err();
    assert!(
        error.to_string().contains("invalid included_files pattern"),
        "{error}"
    );
    assert!(
        error.to_string().contains(&source.display().to_string()),
        "{error}"
    );
    assert_eq!(error.kind, DiagnosticKind::Configuration);
}

#[test]
fn empty_overlay_is_a_no_op() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), "{ width = 80 }\njson {}\n");
    let empty = ConfigOverlay::from_json(&json!({}), root.path()).unwrap();
    assert!(empty.is_empty());
    let mut direct = Effective::default();
    direct.apply_overlay(empty.clone());
    assert!(direct.sources().is_empty());
    let plain = resolver(root.path()).for_directory(root.path()).unwrap();
    let overlaid = overlaid_resolver(root.path(), empty)
        .for_directory(root.path())
        .unwrap();
    assert_eq!(
        overlaid.settings(Language::Json)["width"].value,
        plain.settings(Language::Json)["width"].value
    );
    assert_eq!(overlaid.sources(), plain.sources());
}
