#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use dotfmt_core::config::{CONFIG_NAME, Language, Resolver};

fn config(directory: &Path, source: &str) {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join(CONFIG_NAME), source).unwrap();
}

fn resolver(directory: &Path) -> Resolver {
    Resolver::with_paths(directory.to_path_buf(), None)
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
        ("json {}\njson {}", "duplicate json block"),
        ("markdown {}\nmd {}", "duplicate markdown block"),
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
        assert!(error.contains(diagnostic), "{source}: {error}");
        assert!(
            error.contains(&format!("{}:", root.path().join(CONFIG_NAME).display())),
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
