use dotfile_source::{RepoPath, SourceText};

use crate::{
    Appearance, ApplicationState, ObsidianDerived, ObsidianValue, ThemeDocument, ThemeFileKind,
    ThemeLoweringError, ThemeMap, classify_theme_path, lower_parsed_theme_file,
    lower_schema_theme_file, lower_theme_file,
};

fn lower(path: &str, source: &str) -> crate::ThemeLowering {
    lower_theme_file(
        &RepoPath::new(path).expect("test path"),
        &SourceText::from(source),
    )
}

fn valid(path: &str, source: &str) -> ThemeDocument {
    let result = lower(path, source);
    assert!(
        !result.has_errors(),
        "unexpected diagnostics: {:#?}",
        result.diagnostics()
    );
    result.document().cloned().expect("validated document")
}

fn codes(path: &str, source: &str) -> Vec<String> {
    lower(path, source)
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.clone())
        .collect()
}

#[test]
fn path_classification_is_exact_and_profile_name_is_identity() {
    for (path, kind) in [
        ("theme/roles.dotfile", ThemeFileKind::Roles),
        ("theme/fonts.dotfile", ThemeFileKind::Fonts),
        (
            "theme/maps/catppuccin.dotfile",
            ThemeFileKind::CatppuccinMap,
        ),
        ("theme/maps/eza.dotfile", ThemeFileKind::EzaMap),
        ("theme/maps/gtk.dotfile", ThemeFileKind::GtkMap),
        ("theme/maps/kde.dotfile", ThemeFileKind::KdeMap),
        ("theme/maps/obsidian.dotfile", ThemeFileKind::ObsidianMap),
    ] {
        let classification = classify_theme_path(&RepoPath::new(path).unwrap()).unwrap();
        assert_eq!(classification.kind, kind);
        assert_eq!(classification.profile_name, None);
    }
    let profile =
        classify_theme_path(&RepoPath::new("theme/profiles/7z.dotfile").unwrap()).unwrap();
    assert_eq!(profile.kind, ThemeFileKind::Profile);
    assert_eq!(profile.profile_name.as_deref(), Some("7z"));

    for path in [
        "theme/profiles/_bad.dotfile",
        "theme/profiles/import.dotfile",
        "theme/profiles/a b.dotfile",
        "theme/profiles/a@b.dotfile",
        "theme/profiles/a%b.dotfile",
        "theme/profiles/a💩.dotfile",
        "theme/profiles/dark/nested.dotfile",
        "theme/profiles/mocha.toml",
        "theme/maps/unknown.dotfile",
        "other/theme/roles.dotfile",
    ] {
        assert!(
            classify_theme_path(&RepoPath::new(path).unwrap()).is_err(),
            "{path}"
        );
    }
    let result = lower("theme/maps/unknown.dotfile", "colors {}\n");
    assert_eq!(result.kind(), None);
    assert_eq!(
        codes("theme/maps/unknown.dotfile", "colors {}\n"),
        ["schema/context"]
    );
    assert_eq!(
        codes("theme/profiles/_bad.dotfile", "palette {}\n"),
        ["schema/context"]
    );
    assert!(codes("theme/profiles/mocha.toml", "palette {}\n").is_empty());
}

#[test]
fn schema_classification_hir_and_source_map_are_the_theme_facade_authority() {
    use dotfile_schema::{
        Domain, DomainClassifier, HirRoot, PathClassification, classify_static, lower_path,
    };

    let path = RepoPath::new("theme/roles.dotfile").unwrap();
    let source = SourceText::from("roles { foreground = blue }\nunknown {}\n");
    let parsed = dotfile_syntax::parse(&path, &source);
    let classification = classify_static(&path);
    assert!(matches!(
        classification,
        PathClassification::Known(ref known) if known.domain == Domain::ThemeRoles
    ));
    let schema = lower_path(&path, &source, &parsed, &DomainClassifier::without_groups()).unwrap();
    let root_id = schema.hir().root.hir_id();
    let root_origin = schema.source_map().source_for_hir(root_id).unwrap();
    assert_eq!(root_origin.range.start(), 0);
    assert_eq!(root_origin.range.end(), source.len());
    assert!(matches!(schema.hir().root, HirRoot::Deferred(_)));

    let result = lower_schema_theme_file(&source, &parsed, schema).unwrap();
    assert_eq!(result.schema().domain(), Some(Domain::ThemeRoles));
    assert_eq!(
        result.source_map().source_for_hir(root_id),
        Some(root_origin)
    );
    assert!(result.document().is_none());
    assert!(matches!(
        result.partial_document(),
        Some(ThemeDocument::Roles(_))
    ));
    let dump = result.dump_json();
    assert_eq!(dump["validated"], false);
    assert!(dump["document"].is_null());
    assert_eq!(dump["partial_document"]["type"], "roles");
    assert!(dump["hir"]["nodes"].is_array());
}

#[test]
fn invalid_optional_fields_count_as_authored_presence_and_duplicates() {
    let optional_only = lower(
        "theme/profiles/mocha.dotfile",
        r##"?display-name = "Mocha"
appearance = "dark"
icons = "Icons"
nvim { flavour = "mocha" }
palette { base = "#abcdef" }
"##,
    );
    assert!(optional_only.document().is_none());
    assert!(
        optional_only
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("optional entries"))
    );
    assert!(
        optional_only.diagnostics().iter().all(|diagnostic| {
            diagnostic.summary != "missing required theme field `display-name`"
        })
    );

    let duplicate = lower(
        "theme/profiles/mocha.dotfile",
        r##"?display-name = "first"
display-name = "second"
appearance = "dark"
icons = "Icons"
nvim { flavour = "mocha" }
palette { base = "#abcdef" }
"##,
    );
    assert!(
        duplicate
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "schema/duplicate")
    );
    assert!(
        duplicate.diagnostics().iter().all(|diagnostic| {
            diagnostic.summary != "missing required theme field `display-name`"
        })
    );
}

#[test]
fn invalid_present_fields_do_not_cascade_into_missing_field_diagnostics() {
    let fonts = lower(
        "theme/fonts.dotfile",
        r#"fonts {
    general = blue
    nerd = ""
}
sizes {
    terminal = "0"
    terminal_mac = "01"
    interface = blue
}
applications { obsidian = enabled }
"#,
    );
    assert!(fonts.document().is_none());
    assert!(
        fonts
            .diagnostics()
            .iter()
            .all(|diagnostic| !diagnostic.summary.contains("missing required")),
        "{:#?}",
        fonts.diagnostics()
    );

    let profile = lower(
        "theme/profiles/mocha.dotfile",
        r##"display-name = ""
appearance = "sepia"
icons = ""
nvim { flavour = "" }
palette { base = "#ABCDEF" }
"##,
    );
    assert!(profile.document().is_none());
    assert!(
        profile
            .diagnostics()
            .iter()
            .all(|diagnostic| !diagnostic.summary.contains("missing required")),
        "{:#?}",
        profile.diagnostics()
    );
}

#[test]
fn independent_duplicate_checks_survive_invalid_sibling_values() {
    let roles = lower(
        "theme/roles.dotfile",
        "roles { duplicate = \"not-a-reference\", duplicate = blue }\n",
    );
    assert!(
        roles
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "schema/duplicate"),
        "{:#?}",
        roles.diagnostics()
    );

    let catppuccin = lower(
        "theme/maps/catppuccin.dotfile",
        r#"colors {
    entry { key = "1e1e2e" }
    entry { key = "1e1e2e", palette = base }
}

"#,
    );
    assert!(
        catppuccin
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "schema/duplicate"),
        "{:#?}",
        catppuccin.diagnostics()
    );

    let obsidian = lower(
        "theme/maps/obsidian.dotfile",
        r#"derived { source = mauve }
variables {
    variable { key = "--same", palette = red, rgb = red }
    variable { key = "--same", literal = "red" }
}
"#,
    );
    assert!(
        obsidian
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "schema/duplicate"),
        "{:#?}",
        obsidian.diagnostics()
    );

    for (path, source) in [
        (
            "theme/maps/catppuccin.dotfile",
            r#"colors {
    entry { key = "ABCDEF", palette = base }
    entry { key = "ABCDEF", palette = text }
}
"#,
        ),
        (
            "theme/maps/eza.dotfile",
            r#"categories {
    category { name = image, extensions = [".png", ".png"] }
}
"#,
        ),
        (
            "theme/maps/obsidian.dotfile",
            r#"derived { source = mauve }
variables {
    variable { key = "not-a-custom-property", literal = "one" }
    variable { key = "not-a-custom-property", literal = "two" }
}
"#,
        ),
    ] {
        let invalid_keys = lower(path, source);
        assert!(
            invalid_keys
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "schema/duplicate"),
            "{path}: {:#?}",
            invalid_keys.diagnostics()
        );
    }
}

#[test]
fn independent_value_checks_survive_invalid_container_shapes() {
    let kde = lower(
        "theme/maps/kde.dotfile",
        r#"groups {
    entry { key = "x", roles = [one, "not-a-reference", three] }
}
foregrounds {}
selection-foregrounds {}
"#,
    );
    assert!(
        kde.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("exactly two references"))
    );
    assert!(
        kde.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("must be a bare reference"))
    );

    let obsidian = lower(
        "theme/maps/obsidian.dotfile",
        r#"derived { source = red }
variables {
    variable { key = "--x", palette = red, rgb = blue, alpha = "0.5" }
}
"#,
    );
    assert!(
        obsidian
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("exactly one value shape"))
    );
    assert!(obsidian.diagnostics().iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("alpha is valid only with the color shape")
    }));

    let eza = lower(
        "theme/maps/eza.dotfile",
        r#"categories {
    category { name = "wrong-type", extensions = ["png"] }
    category { name = other, extensions = ["png"] }
}
"#,
    );
    assert!(
        eza.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "theme/map")
    );

    let palette = lower(
        "theme/profiles/mocha.dotfile",
        r##"display-name = "Mocha"
appearance = "dark"
icons = "Icons"
nvim { flavour = "mocha" }
palette { first = "#ABCDEF", second = "#ABCDEF" }
"##,
    );
    assert!(
        palette
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "schema/duplicate")
    );
}

#[test]
fn extension_conflicts_report_every_prior_origin_in_canonical_order() {
    let result = lower(
        "theme/maps/eza.dotfile",
        r#"categories {
    category { name = first, extensions = ["png"] }
    category { name = second, extensions = ["png"] }
    category { name = third, extensions = ["png"] }
}
"#,
    );
    let conflicts: Vec<_> = result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == "theme/map")
        .collect();
    assert_eq!(conflicts.len(), 1, "{:#?}", result.diagnostics());
    assert_eq!(conflicts[0].related_spans.len(), 2);
    assert!(conflicts[0].related_spans[0].start_byte < conflicts[0].primary_span.start_byte);
    assert!(conflicts[0].primary_span.start_byte < conflicts[0].related_spans[1].start_byte);
}

#[test]
fn every_later_duplicate_reports_all_prior_origins() {
    let result = lower(
        "theme/roles.dotfile",
        "roles { same = red, same = blue, same = green }\n",
    );
    let duplicates = result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == "schema/duplicate")
        .collect::<Vec<_>>();
    assert_eq!(duplicates.len(), 2, "{:#?}", result.diagnostics());
    assert_eq!(duplicates[0].related_spans.len(), 1);
    assert_eq!(duplicates[1].related_spans.len(), 2);
    assert!(duplicates[1].related_spans[0].start_byte < duplicates[1].related_spans[1].start_byte);
    assert!(duplicates[1].related_spans[1].start_byte < duplicates[1].primary_span.start_byte);
}

#[test]
fn roles_lower_open_maps_structural_children_and_patterns_in_source_order() {
    let document = valid(
        "theme/roles.dotfile",
        r#"roles {
    second = blue
    first = red
}
terminal {
    foreground = text
    ansi { black = surface1 }
    tabs { active = mauve }
}
eza {
    fi = text
    categories { image = mauve }
    pattern { key = "*.toml", role = orange }
    pattern { key = "*.json", role = yellow }
}
kde { window_bg = base }
konsole { background = base }
"#,
    );
    let ThemeDocument::Roles(roles) = document else {
        panic!("expected roles");
    };
    let names: Vec<_> = roles
        .roles
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| entry.name.value)
        .collect();
    assert_eq!(names, ["second", "first"]);
    let terminal = roles.terminal.unwrap();
    assert_eq!(terminal.direct[0].palette.name, "text");
    assert_eq!(terminal.ansi.unwrap().entries[0].name.value, "black");
    let eza = roles.eza.unwrap();
    assert_eq!(eza.categories.unwrap().entries[0].name.value, "image");
    assert_eq!(
        eza.patterns
            .iter()
            .map(|pattern| pattern.key.value.as_str())
            .collect::<Vec<_>>(),
        ["*.toml", "*.json"]
    );
    assert!(eza.patterns[0].span.start() < eza.patterns[1].span.start());
}

#[test]
fn roles_reject_closed_shape_violations_and_decoded_duplicates() {
    let result = lower(
        "theme/roles.dotfile",
        r#"unknown { key = blue }
terminal { ansi = blue }
eza {
    categories { image = mauve, image = red }
    pattern { key = "*.toml", role = orange, extra = red }
    pattern { key = "*.\u{74}oml", role = yellow }
}
"#,
    );
    assert!(result.document().is_none());
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|item| item.code == "schema/context")
    );
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|item| item.code == "schema/duplicate")
    );
}

#[test]
fn missing_field_diagnostics_use_zero_width_insertion_anchors() {
    let profile_source = r##"display-name = "Mocha"
appearance = "dark"
nvim { flavour = "mocha" }
palette { base = "#abcdef" }
"##;
    let profile = lower("theme/profiles/mocha.dotfile", profile_source);
    let icons = profile
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.summary.ends_with("`icons`"))
        .expect("missing root field");
    assert_eq!(icons.primary_span.start_byte, profile_source.len() as u64);
    assert_eq!(icons.primary_span.end_byte, profile_source.len() as u64);
    assert_eq!(icons.related_spans.len(), 1);

    let gtk_source = "colors { entry { key = \"x\" } }\n";
    let gtk = lower("theme/maps/gtk.dotfile", gtk_source);
    let role = gtk
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.summary.ends_with("`GTK entry.role`"))
        .expect("missing record field");
    let inner_close = gtk_source.find("} }").unwrap() as u64;
    assert_eq!(role.primary_span.start_byte, inner_close);
    assert_eq!(role.primary_span.end_byte, inner_close);
    assert_eq!(role.related_spans.len(), 1);
}

#[test]
fn same_named_missing_fields_receive_only_their_own_diagnostic_poison() {
    let result = lower(
        "theme/maps/obsidian.dotfile",
        "derived { source = red }\nvariables {\nvariable { palette = red }\nvariable { palette = blue }\n}\n",
    );
    let missing_keys = result
        .hir()
        .unwrap()
        .nodes()
        .iter()
        .filter(|node| {
            matches!(
                &node.kind,
                crate::ThemeHirNodeKind::MissingField { name, .. } if name == "key"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(missing_keys.len(), 2);
    assert_ne!(missing_keys[0].range, missing_keys[1].range);
    for node in missing_keys {
        assert!(node.range.is_empty());
        assert!(
            node.poison.iter().all(|poison| poison.range == node.range),
            "missing-field poison escaped its semantic node: {node:#?}"
        );
    }
}

#[test]
fn every_theme_string_must_be_one_literal_one_line_nfc_token() {
    for source in [
        "roles { x = \"blue\" }\n",
        "eza { pattern { key = \"${pattern}\", role = blue } }\n",
        "eza { pattern { key = \"a\" \"b\", role = blue } }\n",
        "eza { pattern { key = \"a\\nline\", role = blue } }\n",
        "eza { pattern { key = \"e\u{301}\", role = blue } }\n",
    ] {
        let result = lower("theme/roles.dotfile", source);
        assert!(result.document().is_none(), "{source}");
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|item| item.code == "schema/context")
        );
    }
}

#[test]
fn fonts_validate_required_open_and_closed_maps() {
    let document = valid(
        "theme/fonts.dotfile",
        r#"fonts {
    general = "Noto Sans"
    nerd = "Hack Nerd Font Mono"
    emoji = "Noto Color Emoji"
}
sizes {
    terminal = "12"
    terminal_mac = "13.5"
    interface = "0.5"
}
applications {
    obsidian = "enabled"
    gtk = "disabled"
}
"#,
    );
    let ThemeDocument::Fonts(fonts) = document else {
        panic!("expected fonts");
    };
    assert_eq!(fonts.fonts.entries.len(), 3);
    assert_eq!(fonts.sizes.terminal.unwrap().value.as_str(), "12");
    assert_eq!(
        fonts.applications.entries[0].state.value,
        ApplicationState::Enabled
    );

    let invalid = lower(
        "theme/fonts.dotfile",
        r#"fonts { general = "A, B" }
sizes { terminal = "0", terminal_mac = "01", interface = "1.0", extra = "2" }
applications { obsidian = "yes" }
"#,
    );
    assert!(invalid.document().is_none());
    assert!(
        invalid.diagnostics().len() >= 6,
        "{:#?}",
        invalid.diagnostics()
    );
}

#[test]
fn profile_uses_filename_identity_and_retains_sparse_overrides() {
    let document = valid(
        "theme/profiles/mocha.dotfile",
        r##"display-name = "Catppuccin Mocha"
appearance = "dark"
icons = "Breeze Chameleon Dark"
nvim { flavour = "mocha" }
palette {
    base = "#1e1e2e"
    text = "#cdd6f4"
}
terminal { foreground = text }
fonts { general = "Profile Sans" }
sizes { terminal = "14" }
applications { obsidian = "enabled" }
"##,
    );
    let ThemeDocument::Profile(profile) = document else {
        panic!("expected profile");
    };
    assert_eq!(profile.identity.name, "mocha");
    assert_eq!(profile.appearance.value, Appearance::Dark);
    assert_eq!(profile.palette.entries[0].name.value, "base");
    assert_eq!(profile.overrides.fonts.unwrap().entries.len(), 1);
    assert!(profile.overrides.sizes.unwrap().terminal_mac.is_none());
}

#[test]
fn profile_rejects_missing_fields_bad_hex_and_duplicate_palette_values() {
    let result = lower(
        "theme/profiles/mocha.dotfile",
        r##"display-name = "Mocha"
appearance = "night"
nvim { flavour = "" }
palette {
    base = "#1E1E2E"
    other = "#abcdef"
    duplicate = "#abcdef"
}
"##,
    );
    assert!(result.document().is_none());
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|item| item.code == "schema/context")
    );
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|item| item.code == "schema/duplicate")
    );
}

#[test]
fn catppuccin_gtk_and_kde_maps_preserve_order_and_exact_records() {
    let catppuccin = valid(
        "theme/maps/catppuccin.dotfile",
        r#"colors {
    entry { key = "1e1e2e", palette = base }
    entry { key = "cdd6f4", palette = text }
}
"#,
    );
    let ThemeDocument::Map(ThemeMap::Catppuccin(catppuccin)) = catppuccin else {
        panic!("expected catppuccin map");
    };
    assert_eq!(catppuccin.entries[0].key.value.as_str(), "1e1e2e");
    assert_eq!(catppuccin.entries[1].palette.name, "text");

    let gtk = valid(
        "theme/maps/gtk.dotfile",
        r#"colors {
    entry { key = "theme_bg_color", role = window_bg }
    entry { key = "error_color", role = negative }
}
"#,
    );
    let ThemeDocument::Map(ThemeMap::Gtk(gtk)) = gtk else {
        panic!("expected GTK map");
    };
    assert_eq!(gtk.entries[0].key.value, "theme_bg_color");

    let kde = valid(
        "theme/maps/kde.dotfile",
        r#"groups {
    entry { key = "Colors:Window", roles = [window_bg, window_alt] }
}
foregrounds {
    entry { key = "ForegroundActive", role = active }
}
selection-foregrounds {
    entry { key = "ForegroundNormal", role = selection_fg }
}
"#,
    );
    let ThemeDocument::Map(ThemeMap::Kde(kde)) = kde else {
        panic!("expected KDE map");
    };
    assert_eq!(kde.groups[0].roles[1].name, "window_alt");
}

#[test]
fn eza_map_validates_extension_syntax_uniqueness_and_ownership() {
    let document = valid(
        "theme/maps/eza.dotfile",
        r#"categories {
    category { name = image, extensions = ["png", "jpg"] }
    category { name = archive, extensions = ["tar", "tar_gz", "7z"] }
}
"#,
    );
    let ThemeDocument::Map(ThemeMap::Eza(eza)) = document else {
        panic!("expected Eza map");
    };
    assert_eq!(eza.categories[1].extensions[2].value.as_str(), "7z");

    let invalid = lower(
        "theme/maps/eza.dotfile",
        r#"categories {
    category { name = image, extensions = [".png", "jpg", "jpg"] }
    category { name = other, extensions = ["jpg"] }
}
"#,
    );
    assert!(invalid.document().is_none());
    assert!(
        invalid
            .diagnostics()
            .iter()
            .any(|item| item.code == "schema/context")
    );
    assert!(
        invalid
            .diagnostics()
            .iter()
            .any(|item| item.code == "schema/duplicate")
    );
    assert!(
        invalid
            .diagnostics()
            .iter()
            .any(|item| item.code == "theme/map")
    );
}

#[test]
fn obsidian_accepts_each_value_shape_and_decimal_boundaries() {
    let document = valid(
        "theme/maps/obsidian.dotfile",
        r#"derived { source = mauve }
variables {
    variable { key = "--palette", palette = crust }
    variable { key = "--rgb", rgb = red }
    variable { key = "--zero", color = crust, alpha = "0" }
    variable { key = "--one", color = crust, alpha = "1" }
    variable { key = "--fraction", color = crust, alpha = "0.72" }
    variable { key = "--derived", derived = accent_hsl }
    variable { key = "--literal", literal = "transparent" }
}
"#,
    );
    let ThemeDocument::Map(ThemeMap::Obsidian(obsidian)) = document else {
        panic!("expected Obsidian map");
    };
    assert!(matches!(
        obsidian.variables[0].value,
        ObsidianValue::Palette(_)
    ));
    assert!(matches!(obsidian.variables[1].value, ObsidianValue::Rgb(_)));
    assert!(matches!(
        obsidian.variables[2].value,
        ObsidianValue::Color { .. }
    ));
    let ObsidianValue::Derived(derived) = &obsidian.variables[5].value else {
        panic!("expected derived");
    };
    assert_eq!(derived.value, ObsidianDerived::AccentHsl);
}

#[test]
fn obsidian_keys_are_css_custom_property_names() {
    for key in ["--x", "--1", "---", "--café"] {
        valid(
            "theme/maps/obsidian.dotfile",
            &format!(
                "derived {{ source = red }}\nvariables {{ variable {{ key = \"{key}\", literal = \"x\" }} }}\n"
            ),
        );
    }
    for key in ["", "--", "x", "--x;}}", "--x:y", "--x/y", "--x y"] {
        let result = lower(
            "theme/maps/obsidian.dotfile",
            &format!(
                "derived {{ source = red }}\nvariables {{ variable {{ key = \"{key}\", literal = \"x\" }} }}\n"
            ),
        );
        assert!(result.document().is_none(), "{key}");
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "schema/context"),
            "{key}: {:#?}",
            result.diagnostics()
        );
    }
}

#[test]
fn maps_reject_duplicates_wrong_cardinality_and_obsidian_shape_combinations() {
    let cat = lower(
        "theme/maps/catppuccin.dotfile",
        r#"colors {
    entry { key = "ABCDEF", palette = base }
    entry { key = "abcdef", palette = base, extra = red }
    entry { key = "abcdef", palette = text }
}
"#,
    );
    assert!(cat.document().is_none());
    assert!(
        cat.diagnostics()
            .iter()
            .any(|item| item.code == "schema/duplicate")
    );

    let kde = lower(
        "theme/maps/kde.dotfile",
        r#"groups { entry { key = "x", roles = [one] } }
foregrounds {}
selection-foregrounds {}
"#,
    );
    assert!(kde.document().is_none());

    for variable in [
        "variable { key = \"--x\", color = red }",
        "variable { key = \"--x\", alpha = \"0.5\", literal = \"x\" }",
        "variable { key = \"--x\", palette = red, rgb = red }",
        "variable { key = \"--x\", color = red, alpha = \"1.1\" }",
        "variable { key = \"--x\", derived = unknown }",
    ] {
        let source = format!("derived {{ source = red }}\nvariables {{ {variable} }}\n");
        let result = lower("theme/maps/obsidian.dotfile", &source);
        assert!(result.document().is_none(), "{variable}");
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|item| item.code == "schema/context")
        );
    }

    let no_shape = lower(
        "theme/maps/obsidian.dotfile",
        "derived { source = red }\nvariables { variable { key = \"--x\" } }\n",
    );
    let missing_choice = no_shape
        .hir()
        .unwrap()
        .nodes()
        .iter()
        .find(|node| {
            matches!(
                node.kind,
                crate::ThemeHirNodeKind::MissingChoice {
                    choice: crate::ThemeChoice::ObsidianValueShape
                }
            )
        })
        .expect("an absent Obsidian value shape is a typed missing choice");
    assert!(missing_choice.range.is_empty());
    assert!(
        no_shape
            .source_map()
            .source_for_hir(missing_choice.id)
            .unwrap()
            .syntax
            .is_none()
    );
    assert!(!no_shape.hir().unwrap().nodes().iter().any(|node| {
        matches!(
            &node.kind,
            crate::ThemeHirNodeKind::MissingField { name, .. } if name == "value-shape"
        )
    }));
}

#[test]
fn parse_errors_prevent_typed_lowering_and_remain_first() {
    let result = lower("theme/roles.dotfile", "roles { foreground = blue\n");
    assert!(result.document().is_none());
    assert_eq!(result.diagnostics()[0].code, "parse/syntax");

    let independent = lower(
        "theme/roles.dotfile",
        "roles { invalid_role = \"not-a-reference\"\n",
    );
    assert!(independent.document().is_none());
    assert_eq!(independent.diagnostics()[0].code, "parse/syntax");
    assert!(
        independent
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "schema/context"),
        "{:#?}",
        independent.diagnostics()
    );

    let poisoned_required = lower(
        "theme/profiles/mocha.dotfile",
        r#"display-name =
appearance = "dark"
icons = "Icons"
nvim { flavour = "mocha" }
palette {}
unknown {}
"#,
    );
    assert!(poisoned_required.document().is_none());
    assert_eq!(poisoned_required.diagnostics()[0].code, "parse/syntax");
    assert!(
        poisoned_required
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.summary != "missing required theme field `display-name`"),
        "{:#?}",
        poisoned_required.diagnostics()
    );
    assert!(
        poisoned_required
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown root field `unknown`")),
        "{:#?}",
        poisoned_required.diagnostics()
    );
}

#[test]
fn parsed_lowering_rejects_stale_source_bytes_or_path_without_panicking() {
    let path = RepoPath::new("theme/roles.dotfile").unwrap();
    let original = SourceText::from("roles { foreground = blue }\n");
    let parsed = dotfile_syntax::parse(&path, &original);

    for stale in [
        SourceText::from("roles { foreground = pink }\n"),
        SourceText::from("x\n"),
    ] {
        assert!(matches!(
            lower_parsed_theme_file(&path, &stale, &parsed),
            Err(ThemeLoweringError::MismatchedParse)
        ));
    }

    let parsed_elsewhere = dotfile_syntax::parse(
        &RepoPath::new("other.dotfile").unwrap(),
        &SourceText::from("roles { invalid = \"not-a-reference\"\n"),
    );
    let same_bytes = SourceText::from("roles { invalid = \"not-a-reference\"\n");
    assert!(matches!(
        lower_parsed_theme_file(&path, &same_bytes, &parsed_elsewhere),
        Err(ThemeLoweringError::MismatchedParse)
    ));
}

#[test]
fn schema_enrichment_rejects_stale_input_and_foreign_synthetic_ranges() {
    use dotfile_schema::{DomainClassifier, lower_path};

    let path = RepoPath::new("theme/roles.dotfile").unwrap();
    let old_source = SourceText::from("roles { foreground = blue }\n");
    let old_parse = dotfile_syntax::parse(&path, &old_source);
    let mut schema = lower_path(
        &path,
        &old_source,
        &old_parse,
        &DomainClassifier::without_groups(),
    )
    .unwrap();

    let foreign_range =
        dotfile_source::ByteRange::new(0, old_source.len() + 1, old_source.len() + 1).unwrap();
    assert_eq!(schema.allocate_synthetic_hir(foreign_range), None);

    let other_path = RepoPath::new("theme/maps/gtk.dotfile").unwrap();
    let other_parse = dotfile_syntax::parse(&other_path, &old_source);
    assert!(matches!(
        lower_schema_theme_file(&old_source, &other_parse, schema.clone()),
        Err(ThemeLoweringError::MismatchedParse)
    ));

    let new_source = SourceText::from("roles { foreground = pink }\n");
    let new_parse = dotfile_syntax::parse(&path, &new_source);
    assert!(matches!(
        lower_schema_theme_file(&new_source, &new_parse, schema.clone()),
        Err(ThemeLoweringError::MismatchedSchema)
    ));
}

#[test]
fn immediate_lowering_does_not_cross_the_m3_resolution_boundary() {
    valid(
        "theme/roles.dotfile",
        "roles { foreground = palette_name_resolved_later }\n",
    );
    valid(
        "theme/profiles/mocha.dotfile",
        r#"display-name = "Mocha"
appearance = "dark"
icons = "Icons"
nvim { flavour = "mocha" }
palette {}
terminal { foreground = palette_name_resolved_later }
"#,
    );
    valid(
        "theme/maps/gtk.dotfile",
        "colors { entry { key = \"external\", role = kde_or_palette_role_resolved_later } }\n",
    );
}

#[test]
fn fixture_projection_is_typed_ordered_and_spanned() {
    let source = "colors {\n    entry { key = \"💩second\", role = blue }\n    entry { key = \"first\", role = red }\n}\n";
    let result = lower("theme/maps/gtk.dotfile", source);
    let Some(ThemeDocument::Map(ThemeMap::Gtk(gtk))) = result.partial_document().as_ref() else {
        panic!("expected GTK partial document");
    };
    assert!(
        !result
            .source_map()
            .hir_for_range(gtk.entries[0].span)
            .is_empty(),
        "typed record range must reuse an authoritative deferred HIR identity"
    );
    let dump = result.dump_json();
    assert_eq!(dump["kind"], "map/gtk");
    assert_eq!(dump["document"]["type"], "map/gtk");
    assert_eq!(dump["document"]["entries"][0]["key"]["value"], "💩second");
    assert_eq!(dump["document"]["entries"][1]["key"]["value"], "first");
    let key_span = &dump["document"]["entries"][0]["key"]["span"];
    let key_start = key_span[0].as_u64().unwrap() as usize;
    let key_end = key_span[1].as_u64().unwrap() as usize;
    assert_eq!(
        &source.as_bytes()[key_start..key_end],
        "\"💩second\"".as_bytes()
    );
    let role_span = &dump["document"]["entries"][0]["role"]["span"];
    let role_start = role_span[0].as_u64().unwrap() as usize;
    let role_end = role_span[1].as_u64().unwrap() as usize;
    assert_eq!(&source.as_bytes()[role_start..role_end], b"blue");
    let record_span = &dump["document"]["entries"][0]["span"];
    let record_start = record_span[0].as_u64().unwrap() as usize;
    let record_end = record_span[1].as_u64().unwrap() as usize;
    assert_eq!(
        &source.as_bytes()[record_start..record_end],
        "entry { key = \"💩second\", role = blue }".as_bytes()
    );
}

#[test]
fn tolerant_theme_hir_retains_poison_missing_unknown_and_independent_siblings() {
    use dotfile_schema::PoisonKind;

    let result = lower(
        "theme/profiles/poison.dotfile",
        r##"display-name = ,
appearance = "dark"
nvim { flavour = "poison" }
palette {
    good = "#abcdef"
    bad = "#ABCDEF"
    duplicate = "#abcdef"
}
unknown {}
"##,
    );
    assert!(result.document().is_none());
    assert!(result.clone().into_validated().is_err());
    let hir = result
        .hir()
        .expect("registered theme always has tolerant HIR");
    let root = hir.node(hir.root()).unwrap();
    assert!(
        root.poison.is_empty(),
        "poison must stay on consuming nodes: {root:#?}"
    );

    let display = hir
        .nodes()
        .iter()
        .find(|node| {
            matches!(
                &node.kind,
                crate::ThemeHirNodeKind::Entry { name: Some(name), .. }
                    if name == "display-name"
            )
        })
        .expect("authored display-name retained");
    let missing_value = hir.node(display.children[0]).unwrap();
    assert!(
        matches!(
            missing_value.kind,
            crate::ThemeHirNodeKind::MissingValue { .. }
        ),
        "{missing_value:#?}"
    );
    assert!(
        missing_value
            .poison
            .iter()
            .any(|poison| poison.kind == PoisonKind::Missing)
    );
    assert!(
        missing_value
            .poison
            .iter()
            .any(|poison| poison.kind == PoisonKind::Syntax)
    );

    let icons = hir
        .nodes()
        .iter()
        .find(|node| {
            matches!(
                &node.kind,
                crate::ThemeHirNodeKind::MissingField { name, .. } if name == "icons"
            )
        })
        .expect("absent required icons retained");
    let icons_origin = result.source_map().source_for_hir(icons.id).unwrap();
    assert!(icons_origin.syntax.is_none());
    assert!(icons.range.is_empty());
    assert!(
        result
            .source_map()
            .hir_for_range(icons.range)
            .contains(&icons.id)
    );

    let appearance = hir
        .nodes()
        .iter()
        .find(|node| {
            matches!(
                &node.kind,
                crate::ThemeHirNodeKind::Value {
                    expected: Some(crate::ThemeValueType::Appearance),
                    decoded: Some(crate::ThemeScalar::String(value)),
                    ..
                } if value == "dark"
            )
        })
        .expect("independent valid appearance retained");
    assert!(appearance.poison.is_empty());

    let unknown = hir
        .nodes()
        .iter()
        .find(|node| {
            matches!(
                &node.kind,
                crate::ThemeHirNodeKind::Entry {
                    name: Some(name),
                    expected: None,
                    ..
                } if name == "unknown"
            )
        })
        .expect("unknown structural subtree retained");
    assert!(
        unknown
            .poison
            .iter()
            .any(|poison| poison.kind == PoisonKind::Context)
    );

    for name in ["good", "bad", "duplicate"] {
        assert!(hir.nodes().iter().any(|node| {
            matches!(
                &node.kind,
                crate::ThemeHirNodeKind::Entry { name: Some(found), .. } if found == name
            )
        }));
    }
    let duplicate = hir
        .nodes()
        .iter()
        .find(|node| {
            matches!(
                &node.kind,
                crate::ThemeHirNodeKind::Value {
                    expected: Some(crate::ThemeValueType::PaletteColor),
                    decoded: Some(crate::ThemeScalar::String(value)),
                    ..
                } if value == "#abcdef"
            ) && node
                .poison
                .iter()
                .any(|poison| poison.kind == PoisonKind::Duplicate)
        })
        .expect("later duplicate color retains duplicate poison");
    assert_eq!(
        duplicate
            .parent
            .and_then(|id| hir.node(id))
            .and_then(|node| {
                match &node.kind {
                    crate::ThemeHirNodeKind::Entry { name, .. } => name.as_deref(),
                    _ => None,
                }
            }),
        Some("duplicate")
    );

    let dump = result.dump_json();
    assert!(dump["document"].is_null());
    assert!(dump["hir"]["nodes"].as_array().unwrap().len() >= hir.nodes().len());
}

#[test]
fn typed_theme_ids_round_trip_through_the_authoritative_source_map() {
    let source =
        SourceText::from("colors { entry { key = \"theme_bg_color\", role = window_bg } }\n");
    let path = RepoPath::new("theme/maps/gtk.dotfile").unwrap();
    let result = lower_theme_file(&path, &source);
    let hir = result.hir().unwrap();
    assert_eq!(hir.root(), result.schema().hir().root.hir_id());
    let key = hir
        .nodes()
        .iter()
        .find(|node| {
            matches!(
                node.kind,
                crate::ThemeHirNodeKind::Value {
                    expected: Some(crate::ThemeValueType::ExternalKey),
                    ..
                }
            )
        })
        .expect("typed GTK key");
    let origin = result.source_map().source_for_hir(key.id).unwrap();
    let syntax = origin
        .syntax
        .expect("authored typed value has syntax identity");
    assert_eq!(origin.range, key.range);
    assert_eq!(result.source_map().hir_for_syntax(syntax), &[key.id]);
    assert!(
        result
            .source_map()
            .hir_for_range(key.range)
            .contains(&key.id)
    );

    let validated = result.into_validated().expect("valid GTK map seals");
    assert!(matches!(
        validated.document(),
        ThemeDocument::Map(ThemeMap::Gtk(_))
    ));
}
