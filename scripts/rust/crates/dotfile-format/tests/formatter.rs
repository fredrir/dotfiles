use dotfile_format::{
    Domain, DomainClassifier, FormatError, FormatSchema, GroupLayout, GroupLayoutEntry,
    format_parsed, format_parsed_with_schema, format_schema, format_source,
    format_source_with_schema, is_canonical,
};
use dotfile_source::{RepoPath, SourceText};
use dotfile_syntax::{Atom, Block, Entry, StringExpr, StringSegment, Value, parse};

fn path() -> RepoPath {
    RepoPath::new("shared/package.dotfile").unwrap()
}

fn classifier() -> DomainClassifier {
    DomainClassifier::new(
        GroupLayout::try_new([GroupLayoutEntry {
            group: "shared".to_owned(),
            directory: RepoPath::new("shared").unwrap(),
        }])
        .unwrap(),
    )
}

fn domain_path(domain: Domain) -> RepoPath {
    let path = match domain {
        Domain::Profiles => "config/profiles.dotfile",
        Domain::Hosts => "config/hosts.dotfile",
        Domain::GroupRootRequirements => "shared/package.dotfile",
        Domain::FacetRequirements => "shared/tool/package.dotfile",
        Domain::OverrideVariant => "shared/overrides/variant/tool/package.dotfile",
        Domain::RecipientKeys => "config/keys.dotfile",
        Domain::SecretScanRules => "config/scan.dotfile",
        Domain::BenchmarkBaselines => "benchmarks/baselines.dotfile",
        Domain::ThemeRoles => "theme/roles.dotfile",
        Domain::ThemeFonts => "theme/fonts.dotfile",
        Domain::ThemeMapCatppuccin => "theme/maps/catppuccin.dotfile",
        Domain::ThemeMapEza => "theme/maps/eza.dotfile",
        Domain::ThemeMapGtk => "theme/maps/gtk.dotfile",
        Domain::ThemeMapKde => "theme/maps/kde.dotfile",
        Domain::ThemeMapObsidian => "theme/maps/obsidian.dotfile",
        Domain::ThemeProfiles => "theme/profiles/test.dotfile",
        Domain::TemplateVariables => "vars.enc.yaml",
        Domain::GeneratedLock => "package.lock.dotfile",
    };
    RepoPath::new(path).unwrap()
}

fn formatted(input: &str, domain: Domain) -> String {
    let source = SourceText::from(input);
    let output = format_source(&domain_path(domain), &source, &classifier()).unwrap();
    String::from_utf8(output.bytes).unwrap()
}

fn assert_idempotent(input: &str, domain: Domain) -> String {
    let once = formatted(input, domain);
    let twice = formatted(&once, domain);
    assert_eq!(twice, once, "formatter was not idempotent for {once:?}");
    let source = SourceText::from(once.as_str());
    let domain_path = domain_path(domain);
    let parsed = parse(&domain_path, &source);
    assert!(!parsed.has_errors(), "canonical output must parse");
    assert!(
        is_canonical(&domain_path, &source, &parsed, &classifier()).unwrap(),
        "second pass must report canonical bytes"
    );
    once
}

#[test]
fn canonicalizes_whitespace_blocks_and_final_newline() {
    let output = assert_idempotent(
        "wezterm{ @version=\"1\",@check=\"command\"}\r\n\r\n\r\nzsh",
        Domain::GroupRootRequirements,
    );
    assert_eq!(
        output,
        "wezterm { @check = \"command\", @version = \"1\" }\n\nzsh\n"
    );
}

#[test]
fn empty_and_comment_only_sources_have_canonical_endings() {
    assert_eq!(formatted("", Domain::GroupRootRequirements), "");
    assert_eq!(formatted(" \t\n\n", Domain::GroupRootRequirements), "");
    assert_eq!(
        assert_idempotent("\u{feff}# one\r\n\r\n# two", Domain::GroupRootRequirements),
        "# one\n\n# two\n"
    );
}

#[test]
fn string_atoms_and_escapes_use_one_canonical_string() {
    let output = assert_idempotent(
        "@let vault=\"~/main\"\n@theme=$vault \"/x\"\nname=\"\\u{0041}\\u{007F}\\u{0085}\\b\\f\\n\\\"\\\\\\${literal}\"\n",
        Domain::GroupRootRequirements,
    );
    assert_eq!(
        output,
        "@let vault = \"~/main\"\n@theme = \"${vault}/x\"\nname = \"A\\u{7f}\\u{85}\\b\\f\\n\\\"\\\\\\${literal}\"\n"
    );
}

#[test]
fn source_paths_choose_bare_or_canonical_quoted_spelling() {
    let output = assert_idempotent(
        "./\"safe/path-%@=\"\n./\"space dir/\\u{0066}ile\"\n",
        Domain::FacetRequirements,
    );
    assert_eq!(output, "./safe/path-%@=\n./\"space dir/file\"\n");
}

#[test]
fn invalid_decoded_paths_are_stable_sorting_barriers() {
    let decomposed = assert_idempotent("./z\n./\"cafe\u{301}\"\n./a\n", Domain::FacetRequirements);
    assert!(decomposed.find("./z").unwrap() < decomposed.find("cafe\u{301}").unwrap());
    assert!(decomposed.find("cafe\u{301}").unwrap() < decomposed.find("./a").unwrap());
}

#[test]
fn lists_preserve_order_and_wrap_at_scalar_width() {
    let long_a = "a".repeat(55);
    let long_b = "b".repeat(55);
    let input = format!("font {{}}\n@family=[\"{long_a}\",\"{long_b}\",]\n");
    let output = assert_idempotent(&input, Domain::FacetRequirements);
    assert!(output.contains(&format!(
        "@family = [\n    \"{long_a}\",\n    \"{long_b}\",\n]"
    )));
    assert!(output.find(&long_a).unwrap() < output.find(&long_b).unwrap());

    let unsplittable = "x".repeat(101);
    let output = assert_idempotent(
        &format!("field=[\"{unsplittable}\"]\n"),
        Domain::GroupRootRequirements,
    );
    assert!(output.contains(&format!("field = [\n    \"{unsplittable}\",\n]")));
    assert!(!output.contains("[\n\n"));

    let long_name = "n".repeat(98);
    let empty = assert_idempotent(&format!("{long_name}=[]\n"), Domain::GroupRootRequirements);
    assert_eq!(empty, format!("{long_name} =\n    []\n"));

    let nonempty = assert_idempotent(
        &format!("{long_name}=[value]\n"),
        Domain::GroupRootRequirements,
    );
    assert_eq!(
        nonempty,
        format!("{long_name} =\n    [\n        value,\n    ]\n")
    );
}

#[test]
fn multiline_blocks_have_four_space_indent_and_commas() {
    let output = assert_idempotent(
        "outer { first=\"1\", second=\"2\", inner { key=\"v\" }, fourth=\"4\" }\n",
        Domain::GroupRootRequirements,
    );
    assert_eq!(
        output,
        "outer {\n    first = \"1\",\n    fourth = \"4\",\n    inner { key = \"v\" },\n    second = \"2\",\n}\n"
    );
}

#[test]
fn empty_blocks_expand_when_only_the_closing_brace_exceeds_width() {
    let fits = "f".repeat(97);
    assert_eq!(
        assert_idempotent(&format!("{fits}{{}}\n"), Domain::GroupRootRequirements),
        format!("{fits} {{}}\n")
    );

    let boundary = "b".repeat(98);
    assert_eq!(
        assert_idempotent(&format!("{boundary}{{}}\n"), Domain::GroupRootRequirements),
        format!("{boundary} {{\n}}\n")
    );

    let unsplittable = "u".repeat(101);
    let output = assert_idempotent(
        &format!("{unsplittable}{{}}\n"),
        Domain::GroupRootRequirements,
    );
    assert_eq!(output, format!("{unsplittable} {{}}\n"));
    assert!(output.lines().next().unwrap().chars().count() > 100);

    let target = "t".repeat(83);
    assert_eq!(
        assert_idempotent(
            &format!("@extend entity/{target}{{}}\n"),
            Domain::GroupRootRequirements
        ),
        format!("@extend entity/{target} {{\n}}\n")
    );

    let resource = "r".repeat(97);
    assert_eq!(
        assert_idempotent(&format!("@{resource}{{}}\n"), Domain::GroupRootRequirements),
        format!("@{resource} {{\n}}\n")
    );
}

#[test]
fn width_is_counted_in_unicode_scalars_not_utf8_bytes_or_display_cells() {
    let astral = "😀".repeat(70);
    let output = assert_idempotent(
        &format!("outer {{ text=\"{astral}\" }}\n"),
        Domain::GroupRootRequirements,
    );
    assert_eq!(output.lines().count(), 1);
    assert!(output.len() > 100);

    let combining = "\u{301}".repeat(90);
    let output = assert_idempotent(
        &format!("outer {{ text=\"{combining}\" }}\n"),
        Domain::GroupRootRequirements,
    );
    assert!(output.starts_with("outer {\n"));
}

#[test]
fn scalar_assignments_soft_wrap_after_equals() {
    let name = "field".repeat(7);
    let value = "v".repeat(75);
    let assignment = assert_idempotent(
        &format!("{name}=\"{value}\"\n"),
        Domain::GroupRootRequirements,
    );
    assert_eq!(assignment, format!("{name} =\n    \"{value}\"\n"));

    let binding = "binding".repeat(5);
    let initializer = "i".repeat(70);
    let declaration = assert_idempotent(
        &format!("@let {binding}=\"{initializer}\"\n"),
        Domain::GroupRootRequirements,
    );
    assert_eq!(
        declaration,
        format!("@let {binding} =\n    \"{initializer}\"\n")
    );

    let inner_name = "inner".repeat(6);
    let inner_value = "x".repeat(70);
    let nested = assert_idempotent(
        &format!("outer {{ {inner_name}=\"{inner_value}\" }}\n"),
        Domain::GroupRootRequirements,
    );
    assert!(nested.contains(&format!("    {inner_name} =\n        \"{inner_value}\",")));
}

#[test]
fn requirement_order_uses_attributes_paths_resources_extensions_and_demands() {
    let output = assert_idempotent(
        concat!(
            "?zeta\n",
            "alpha\n",
            "?alpha\n",
            "@extend entity/zeta { @version=\"1\" }\n",
            "@font { @family=\"Z\", @key=zfont }\n",
            "@font { @key=afont, @family=\"A\" }\n",
            "./z { @deploy=\"none\" }\n",
            "./a { @deploy=\"none\" }\n",
            "@description=\"facet\"\n",
            "@deploy=\"copy\"\n",
        ),
        Domain::FacetRequirements,
    );
    let expected_order = [
        "@deploy =",
        "@description =",
        "./a ",
        "./z ",
        "@font { @key = afont",
        "@font { @key = zfont",
        "@extend entity/zeta",
        "alpha\n",
        "?alpha\n",
        "?zeta\n",
    ];
    let mut cursor = 0;
    for needle in expected_order {
        let found = output[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in {output}"));
        cursor += found + needle.len();
    }
}

#[test]
fn let_prologue_is_stable_and_late_let_is_a_barrier() {
    let output = assert_idempotent(
        "@let second=\"2\"\n@let first=\"1\"\nzeta\n@let late=\"x\"\nalpha\n",
        Domain::GroupRootRequirements,
    );
    assert_eq!(
        output,
        "@let second = \"2\"\n@let first = \"1\"\nzeta\n@let late = \"x\"\nalpha\n"
    );
}

#[test]
fn comments_attach_move_and_form_sorting_boundaries() {
    let output = assert_idempotent(
        "zsh\n# belongs to alpha\nalpha\n\n# tools\n\nbeta # trailing\n",
        Domain::GroupRootRequirements,
    );
    assert_eq!(
        output,
        "# belongs to alpha\nalpha\nzsh\n\n# tools\n\nbeta  # trailing\n"
    );
    assert_eq!(output.matches("# belongs to alpha").count(), 1);
    assert_eq!(output.matches("# tools").count(), 1);
    assert_eq!(output.matches("# trailing").count(), 1);
}

#[test]
fn standalone_comments_attach_independently_across_intervening_entries() {
    let published = assert_idempotent(
        concat!(
            "sizes{terminal=\"9\"}\n",
            "fonts=\"wrong\"\n",
            "# attached to fonts\n",
            "fonts{general=\"First\"}\n",
            "# first duplicate sizes\n",
            "sizes{terminal=\"10\"}\n",
            "# second duplicate sizes\n",
            "sizes{terminal=\"11\"}\n",
            "# attached to applications\n",
            "applications{obsidian=\"enabled\"}\n",
        ),
        Domain::ThemeFonts,
    );
    assert!(
        published.find("# attached to fonts").unwrap()
            < published.find("fonts { general = \"First\" }").unwrap()
    );
    assert!(
        published.find("# first duplicate sizes").unwrap()
            < published.find("sizes { terminal = \"10\" }").unwrap()
    );
    assert!(
        published.find("# second duplicate sizes").unwrap()
            < published.find("sizes { terminal = \"11\" }").unwrap()
    );
    assert!(
        published.find("# attached to applications").unwrap()
            < published
                .find("applications { obsidian = \"enabled\" }")
                .unwrap()
    );

    let names = assert_idempotent(
        concat!(
            "zulu{role=\"server\"}\n",
            "barrier=\"wrong\"\n",
            "# first alpha\n",
            "alpha{role=\"desktop\"}\n",
            "# second alpha\n",
            "alpha{role=\"laptop\"}\n",
            "# attached beta\n",
            "beta{role=\"server\"}\n",
        ),
        Domain::Hosts,
    );
    assert!(
        names.find("# first alpha").unwrap() < names.find("alpha { role = \"desktop\" }").unwrap()
    );
    assert!(
        names.find("# second alpha").unwrap() < names.find("alpha { role = \"laptop\" }").unwrap()
    );
    assert!(
        names.find("# attached beta").unwrap() < names.find("beta { role = \"server\" }").unwrap()
    );

    let resource = assert_idempotent(
        concat!(
            "zsh\n",
            "@font{\n",
            "# first identity\n",
            "@key=hack\n",
            "# duplicate identity\n",
            "@key=other\n",
            "@description=\"font\"\n",
            "}\n",
            "wezterm\n",
            "alacritty\n",
        ),
        Domain::GroupRootRequirements,
    );
    assert!(resource.find("# first identity").unwrap() < resource.find("@key = hack").unwrap());
    assert!(
        resource.find("# duplicate identity").unwrap() < resource.find("@key = other").unwrap()
    );
    for comment in [
        "# attached to fonts",
        "# first duplicate sizes",
        "# second duplicate sizes",
        "# attached to applications",
    ] {
        assert_eq!(published.matches(comment).count(), 1);
    }
}

#[test]
fn recipient_and_benchmark_keys_sort_by_bytes() {
    assert_eq!(
        assert_idempotent(
            "recipients { z=\"age1z\", a=\"age1a\" }\n",
            Domain::RecipientKeys,
        ),
        "recipients { a = \"age1a\", z = \"age1z\" }\n"
    );
    assert_eq!(
        assert_idempotent(
            "zhost { ffffffff=\"z\", 00000000=\"a\" }\nahost {}\n",
            Domain::BenchmarkBaselines,
        ),
        "ahost {}\nzhost { 00000000 = \"a\", ffffffff = \"z\" }\n"
    );
}

#[test]
fn profile_host_and_scan_published_orders_are_schema_driven() {
    let profiles = assert_idempotent(
        "@theme=mocha\n@profiles { p { @description=\"p\", @os=\"linux\", @groups=[g], @manager=\"apt\" } }\n@groups { g { @description=\"g\", @directory=\"g\" } }\n@dotfile-version=\"1\"\n",
        Domain::Profiles,
    );
    assert!(profiles.starts_with("@dotfile-version = \"1\"\n@groups"));
    let groups = &profiles[profiles.find("@groups").unwrap()..profiles.find("@profiles").unwrap()];
    assert!(groups.find("@directory").unwrap() < groups.find("@description").unwrap());
    assert!(profiles.find("@profiles").unwrap() < profiles.rfind("@theme").unwrap());

    let hosts = assert_idempotent(
        "z { CPU=\"z\", @theme=t, role=\"desktop\", @profile=p, hostnames=[\"z\"] }\na { @profile=p, role=\"server\", hostnames=[\"a\"] }\n",
        Domain::Hosts,
    );
    assert!(hosts.starts_with("a {"));
    let z = hosts.find("z {").unwrap();
    let z_text = &hosts[z..];
    assert!(z_text.find("hostnames").unwrap() < z_text.find("role").unwrap());
    assert!(z_text.find("role").unwrap() < z_text.find("@profile").unwrap());

    let scan = assert_idempotent(
        "allow { rule { inspect=\"path\", pattern=\"z\" }, rule { pattern=\"a\", inspect=\"value\" } }\n",
        Domain::SecretScanRules,
    );
    assert!(scan.find("pattern = \"z\"").unwrap() < scan.find("pattern = \"a\"").unwrap());
}

#[test]
fn theme_singletons_sort_but_open_maps_and_records_do_not() {
    let fonts = assert_idempotent(
        "applications { z=\"enabled\", a=\"disabled\" }\nsizes { interface=\"10\", terminal_mac=\"13\", terminal=\"12\" }\nfonts { nerd=\"N\", general=\"G\" }\n",
        Domain::ThemeFonts,
    );
    assert!(fonts.starts_with("fonts { nerd = \"N\", general = \"G\" }\nsizes"));
    assert!(
        fonts.contains("sizes { terminal = \"12\", terminal_mac = \"13\", interface = \"10\" }")
    );
    assert!(fonts.ends_with("applications { z = \"enabled\", a = \"disabled\" }\n"));

    let roles = assert_idempotent(
        "eza { pattern { role=red, key=\"z\" }, direct_b=blue, categories { image=red }, direct_a=green }\nterminal { tabs { active=blue }, foreground=text, ansi { black=base }, background=base }\n",
        Domain::ThemeRoles,
    );
    let eza = &roles[roles.find("eza {").unwrap()..];
    assert!(eza.find("direct_b").unwrap() < eza.find("direct_a").unwrap());
    assert!(eza.find("direct_a").unwrap() < eza.find("categories").unwrap());
    assert!(eza.find("categories").unwrap() < eza.find("pattern").unwrap());
    let terminal = &roles[roles.find("terminal").unwrap()..];
    assert!(terminal.find("foreground").unwrap() < terminal.find("background").unwrap());
    assert!(terminal.find("background").unwrap() < terminal.find("ansi").unwrap());
    assert!(terminal.find("ansi").unwrap() < terminal.find("tabs").unwrap());

    let records = assert_idempotent(
        "categories { category { extensions=[\"zip\",\"tar\"], name=archive }, category { name=image, extensions=[\"png\",\"jpg\"] } }\n",
        Domain::ThemeMapEza,
    );
    assert!(records.find("name = archive").unwrap() < records.find("name = image").unwrap());
    assert!(records.find("\"zip\"").unwrap() < records.find("\"tar\"").unwrap());
}

#[test]
fn wrong_form_theme_tail_names_are_stable_barriers() {
    let terminal = assert_idempotent(
        "terminal { tabs { active=blue }, ansi=base, foreground=text }\n",
        Domain::ThemeRoles,
    );
    assert!(terminal.find("tabs").unwrap() < terminal.find("ansi = base").unwrap());
    assert!(terminal.find("ansi = base").unwrap() < terminal.find("foreground").unwrap());

    let eza = assert_idempotent(
        "eza { pattern { key=\"x\", role=red }, categories=base, direct=blue }\n",
        Domain::ThemeRoles,
    );
    assert!(eza.find("pattern {").unwrap() < eza.find("categories = base").unwrap());
    assert!(eza.find("categories = base").unwrap() < eza.find("direct = blue").unwrap());
}

#[test]
fn invalid_open_map_leaves_are_stable_barriers() {
    let quoted = assert_idempotent(
        "terminal { tabs { active=blue }, invalid=\"blue\", foreground=text }\n",
        Domain::ThemeRoles,
    );
    assert!(quoted.find("tabs {").unwrap() < quoted.find("invalid = \"blue\"").unwrap());
    assert!(quoted.find("invalid = \"blue\"").unwrap() < quoted.find("foreground = text").unwrap());

    let optional = assert_idempotent(
        "eza { pattern { key=\"x\", role=red }, ?invalid=blue, direct=green }\n",
        Domain::ThemeRoles,
    );
    assert!(optional.find("pattern {").unwrap() < optional.find("?invalid = blue").unwrap());
    assert!(optional.find("?invalid = blue").unwrap() < optional.find("direct = green").unwrap());

    let profile = assert_idempotent(
        "terminal { tabs { active=blue }, foreground=text }\n",
        Domain::ThemeProfiles,
    );
    assert!(profile.find("foreground = text").unwrap() < profile.find("tabs {").unwrap());
}

#[test]
fn invalid_group_and_host_named_forms_are_stable_barriers() {
    let groups = assert_idempotent(
        "@dotfile-version=\"1\"\n@groups { root { z {}, invalid=value, a {} } }\n@profiles {}\n",
        Domain::Profiles,
    );
    assert!(groups.find("z {}").unwrap() < groups.find("invalid = value").unwrap());
    assert!(groups.find("invalid = value").unwrap() < groups.find("a {}").unwrap());

    let optional_group = assert_idempotent(
        "@dotfile-version=\"1\"\n@groups { root { z {}, ?invalid {}, a {} } }\n@profiles {}\n",
        Domain::Profiles,
    );
    assert!(optional_group.find("z {}").unwrap() < optional_group.find("?invalid {}").unwrap());
    assert!(optional_group.find("?invalid {}").unwrap() < optional_group.find("a {}").unwrap());

    let hosts = assert_idempotent(
        "machine { Z_FACT=\"z\", unknown=\"invalid\", A_FACT=\"a\", @theme=t, @profile=p, role=\"desktop\", hostnames=[\"machine\"] }\n",
        Domain::Hosts,
    );
    assert!(hosts.find("Z_FACT").unwrap() < hosts.find("unknown").unwrap());
    assert!(hosts.find("unknown").unwrap() < hosts.find("A_FACT").unwrap());
}

#[test]
fn recursive_group_attributes_use_the_group_schema_order() {
    let profiles = assert_idempotent(
        concat!(
            "@dotfile-version=\"1\"\n",
            "@groups { root { child { @description=\"child\", @directory=\"nested\" } } }\n",
            "@profiles {}\n",
        ),
        Domain::Profiles,
    );
    let child = &profiles[profiles.find("child {").unwrap()..];
    assert!(child.find("@directory").unwrap() < child.find("@description").unwrap());
}

#[test]
fn wrong_form_published_entries_are_stable_barriers() {
    let profiles = assert_idempotent(
        "@theme=mocha\n@groups=invalid\n@dotfile-version=\"1\"\n",
        Domain::Profiles,
    );
    assert!(profiles.find("@theme").unwrap() < profiles.find("@groups = invalid").unwrap());
    assert!(
        profiles.find("@groups = invalid").unwrap() < profiles.find("@dotfile-version").unwrap()
    );

    let roles = assert_idempotent("terminal {}\nroles=invalid\neza {}\n", Domain::ThemeRoles);
    assert!(roles.find("terminal {}").unwrap() < roles.find("roles = invalid").unwrap());
    assert!(roles.find("roles = invalid").unwrap() < roles.find("eza {}").unwrap());

    let fonts = assert_idempotent(
        "applications {}\nsizes=invalid\nfonts {}\n",
        Domain::ThemeFonts,
    );
    assert!(fonts.find("applications {}").unwrap() < fonts.find("sizes = invalid").unwrap());
    assert!(fonts.find("sizes = invalid").unwrap() < fonts.find("fonts {}").unwrap());

    let record = assert_idempotent(
        "colors { entry { palette=base, key {}, key=blue } }\n",
        Domain::ThemeMapCatppuccin,
    );
    assert!(record.find("palette = base").unwrap() < record.find("key {}").unwrap());
    assert!(record.find("key {}").unwrap() < record.find("key = blue").unwrap());
}

#[test]
fn wrong_form_byte_sorted_entries_are_stable_barriers() {
    let hosts = assert_idempotent("z {}\ninvalid=value\na {}\n", Domain::Hosts);
    assert!(hosts.find("z {}").unwrap() < hosts.find("invalid = value").unwrap());
    assert!(hosts.find("invalid = value").unwrap() < hosts.find("a {}").unwrap());

    let recipients = assert_idempotent(
        "recipients { z=zed, invalid {}, a=aye }\n",
        Domain::RecipientKeys,
    );
    assert!(recipients.find("z = zed").unwrap() < recipients.find("invalid {}").unwrap());
    assert!(recipients.find("invalid {}").unwrap() < recipients.find("a = aye").unwrap());
}

#[test]
fn invalid_byte_sort_identities_are_stable_barriers() {
    let recipients = assert_idempotent(
        concat!(
            "recipients {\n",
            "zulu=\"z\"\n",
            "# invalid recipient label\n",
            "bad+label=\"invalid\"\n",
            "# lower valid label two\n",
            "beta=\"b\"\n",
            "# lower valid label one\n",
            "alpha=\"a\"\n",
            "}\n",
        ),
        Domain::RecipientKeys,
    );
    let recipient_order = [
        "zulu = \"z\"",
        "# invalid recipient label",
        "bad+label = \"invalid\"",
        "# lower valid label one",
        "alpha = \"a\"",
        "# lower valid label two",
        "beta = \"b\"",
    ];
    assert_needles_in_order(&recipients, &recipient_order);

    let benchmarks = assert_idempotent(
        concat!(
            "archie {\n",
            "ffffffff=\"high\"\n",
            "# invalid benchmark epoch\n",
            "ABCDEF01=\"invalid\"\n",
            "# lower valid epoch two\n",
            "00000002=\"two\"\n",
            "# lower valid epoch one\n",
            "00000001=\"one\"\n",
            "}\n",
        ),
        Domain::BenchmarkBaselines,
    );
    let benchmark_order = [
        "ffffffff = \"high\"",
        "# invalid benchmark epoch",
        "ABCDEF01 = \"invalid\"",
        "# lower valid epoch one",
        "00000001 = \"one\"",
        "# lower valid epoch two",
        "00000002 = \"two\"",
    ];
    assert_needles_in_order(&benchmarks, &benchmark_order);
}

#[test]
fn duplicate_sort_keys_retain_their_relative_source_order() {
    let theme = assert_idempotent(
        "sizes { interface=first, terminal=value, interface=second }\n",
        Domain::ThemeFonts,
    );
    assert!(theme.find("terminal = value").unwrap() < theme.find("interface = first").unwrap());
    assert!(theme.find("interface = first").unwrap() < theme.find("interface = second").unwrap());

    let requirement = assert_idempotent(
        "@description=\"first\"\n@deploy=\"copy\"\n@description=\"second\"\n",
        Domain::FacetRequirements,
    );
    assert!(requirement.find("@deploy").unwrap() < requirement.find("first").unwrap());
    assert!(requirement.find("first").unwrap() < requirement.find("second").unwrap());
}

#[test]
fn schema_invalid_but_parse_valid_input_is_total_and_stable() {
    let input = "@future=\"kept\"\n@description=\"known\"\n@font { @family=\"keyless\" }\n@future=\"duplicate\"\n";
    let output = assert_idempotent(input, Domain::FacetRequirements);
    assert!(output.contains("@future = \"kept\""));
    assert!(output.contains("@future = \"duplicate\""));
    assert!(output.contains("@font { @family = \"keyless\" }"));
}

#[test]
fn invalid_named_demand_value_shapes_are_stable_barriers() {
    let reference = assert_idempotent(
        "# invalid reference sugar\nz=bad_ref\na\n",
        Domain::GroupRootRequirements,
    );
    assert_needles_in_order(
        &reference,
        &["# invalid reference sugar", "z = bad_ref", "a\n"],
    );

    let list = assert_idempotent("z=[\"bad\"]\na\n", Domain::GroupRootRequirements);
    assert_needles_in_order(&list, &["z = [\"bad\"]", "a\n"]);

    let valid_string = assert_idempotent("z=\"package\"\na\n", Domain::GroupRootRequirements);
    assert_needles_in_order(&valid_string, &["a\n", "z = \"package\""]);
}

#[test]
fn forbidden_structural_entries_are_stable_sorting_barriers() {
    let group = assert_idempotent(
        "zeta\n./invalid { @deploy=\"none\" }\nalpha\n",
        Domain::GroupRootRequirements,
    );
    assert!(group.find("zeta").unwrap() < group.find("./invalid").unwrap());
    assert!(group.find("./invalid").unwrap() < group.find("alpha").unwrap());

    let variant = assert_idempotent(
        "@description=\"before\"\nzeta\n@deploy=\"copy\"\n",
        Domain::OverrideVariant,
    );
    assert!(variant.find("@description").unwrap() < variant.find("zeta").unwrap());
    assert!(variant.find("zeta").unwrap() < variant.find("@deploy").unwrap());

    let path_body = assert_idempotent(
        "./x { @destination=\"~/before\", zeta, @deploy=\"none\" }\n",
        Domain::FacetRequirements,
    );
    assert!(path_body.find("@destination").unwrap() < path_body.find("zeta").unwrap());
    assert!(path_body.find("zeta").unwrap() < path_body.find("@deploy").unwrap());

    let invalid_resources = assert_idempotent(
        "zeta\n@icon { @key=icon }\nalpha\n@font { @family=\"before\", @key=\"not-bare\", @pkg=\"after\" }\n",
        Domain::FacetRequirements,
    );
    assert!(invalid_resources.find("zeta").unwrap() < invalid_resources.find("@icon").unwrap());
    assert!(invalid_resources.find("@icon").unwrap() < invalid_resources.find("alpha").unwrap());
    let font = &invalid_resources[invalid_resources.find("@font").unwrap()..];
    assert!(font.find("@family").unwrap() < font.find("@key").unwrap());
    assert!(font.find("@key").unwrap() < font.find("@pkg").unwrap());

    let invalid_keys = assert_idempotent(
        concat!(
            "zeta\n",
            "@font { @family=\"keyless\" }\n",
            "alpha\n",
            "@font { @family=\"before\", @key=one, @pkg=\"middle\", @key=two, @version=\"after\" }\n",
            "beta\n",
            "@font { @family=\"before\", @key=\"${interpolated}\", @pkg=\"after\" }\n",
            "gamma\n",
        ),
        Domain::FacetRequirements,
    );
    for pair in [
        ("zeta", "keyless"),
        ("keyless", "alpha"),
        ("alpha", "middle"),
        ("middle", "beta"),
        ("beta", "interpolated"),
        ("interpolated", "gamma"),
    ] {
        assert!(invalid_keys.find(pair.0).unwrap() < invalid_keys.find(pair.1).unwrap());
    }
    let duplicate = &invalid_keys[invalid_keys.find("@key = one").unwrap()..];
    assert!(duplicate.find("@pkg").unwrap() < duplicate.find("@key = two").unwrap());

    let unknown_extension = assert_idempotent(
        concat!(
            "@extend entity/z {}\n",
            "@extend icon/invalid { @description=\"before\", @pkg=\"after\" }\n",
            "@extend entity/a {}\n",
        ),
        Domain::FacetRequirements,
    );
    assert!(
        unknown_extension.find("entity/z").unwrap()
            < unknown_extension.find("icon/invalid").unwrap()
    );
    assert!(
        unknown_extension.find("icon/invalid").unwrap()
            < unknown_extension.find("entity/a").unwrap()
    );
    let invalid_body = &unknown_extension[unknown_extension.find("icon/invalid").unwrap()..];
    assert!(invalid_body.find("@description").unwrap() < invalid_body.find("@pkg").unwrap());
}

#[test]
fn known_but_misplaced_attributes_are_stable_barriers() {
    let facet = assert_idempotent(
        "@description=\"before\"\n@expect=\"file\"\n@deploy=\"copy\"\n",
        Domain::FacetRequirements,
    );
    assert!(facet.find("@description").unwrap() < facet.find("@expect").unwrap());
    assert!(facet.find("@expect").unwrap() < facet.find("@deploy").unwrap());

    let path = assert_idempotent(
        "./x { @expect=\"file\", @description=\"invalid\", @deploy=\"none\" }\n",
        Domain::FacetRequirements,
    );
    assert!(path.find("@expect").unwrap() < path.find("@description").unwrap());
    assert!(path.find("@description").unwrap() < path.find("@deploy").unwrap());

    let resource = assert_idempotent(
        "@font { @family=\"before\", @bin=\"invalid\", @pkg=\"after\", @key=font }\n",
        Domain::FacetRequirements,
    );
    // The illegal entity-only fact is a barrier; even the valid identity is
    // not moved across it.
    assert!(resource.find("@family").unwrap() < resource.find("@bin").unwrap());
    assert!(resource.find("@bin").unwrap() < resource.find("@key").unwrap());
    assert!(resource.find("@key").unwrap() < resource.find("@pkg").unwrap());

    let extensions = assert_idempotent(
        concat!(
            "@extend entity/tool { @description=\"d\", @path=\"/tool\", @pkg=\"pkg\", @bin=\"tool\" }\n",
            "@extend font/font { @family=\"before\", @bin=\"invalid\", @pkg=\"after\" }\n",
        ),
        Domain::FacetRequirements,
    );
    let entity = &extensions[..extensions.find("@extend font").unwrap()];
    assert!(entity.find("@pkg").unwrap() < entity.find("@bin").unwrap());
    assert!(entity.find("@bin").unwrap() < entity.find("@path").unwrap());
    assert!(entity.find("@path").unwrap() < entity.find("@description").unwrap());
    let font = &extensions[extensions.find("@extend font").unwrap()..];
    assert!(font.find("@family").unwrap() < font.find("@bin").unwrap());
    assert!(font.find("@bin").unwrap() < font.find("@pkg").unwrap());
}

#[test]
fn refuses_invalid_syntax_generated_lock_and_mismatched_parse() {
    let classifier = classifier();
    let invalid = SourceText::from("wezterm {\n");
    let error = format_source(&path(), &invalid, &classifier).unwrap_err();
    assert!(matches!(error, FormatError::InvalidSyntax { .. }));

    let lock_path = RepoPath::new("package.lock.dotfile").unwrap();
    let lock = SourceText::from("@lock-version = \"1\"\n");
    assert!(matches!(
        format_source(&lock_path, &lock, &classifier),
        Err(FormatError::GeneratedLockReadOnly)
    ));

    let variables_path = RepoPath::new("vars.enc.yaml").unwrap();
    let variables = SourceText::from("encrypted: payload\n");
    assert!(matches!(
        format_source(&variables_path, &variables, &classifier),
        Err(FormatError::UnsupportedDomain {
            domain: Domain::TemplateVariables
        })
    ));

    let one = SourceText::from("one\n");
    let two = SourceText::from("two\n");
    let parsed = parse(&path(), &one);
    assert!(matches!(
        format_parsed(&path(), &two, &parsed, &classifier),
        Err(FormatError::MismatchedParse)
    ));

    let other_path = RepoPath::new("config/hosts.dotfile").unwrap();
    assert!(matches!(
        format_parsed(&other_path, &one, &parsed, &classifier),
        Err(FormatError::MismatchedParse)
    ));
    let invalid_parsed_elsewhere = parse(&path(), &invalid);
    assert!(matches!(
        format_parsed(
            &other_path,
            &invalid,
            &invalid_parsed_elsewhere,
            &classifier,
        ),
        Err(FormatError::MismatchedParse)
    ));
    assert!(matches!(
        format_parsed_with_schema(
            &other_path,
            &one,
            &parsed,
            &classifier,
            format_schema(Domain::Hosts),
        ),
        Err(FormatError::MismatchedParse)
    ));
}

#[test]
fn path_classification_is_authoritative_over_explicit_schemas() {
    let classifier = classifier();
    let hosts_path = domain_path(Domain::Hosts);
    let hosts = SourceText::from("machine {}\n");
    assert!(matches!(
        format_source_with_schema(
            &hosts_path,
            &hosts,
            &classifier,
            format_schema(Domain::ThemeFonts)
        ),
        Err(FormatError::SchemaMismatch {
            expected: Domain::Hosts,
            actual: Domain::ThemeFonts,
            ..
        })
    ));

    let canonical_hosts = format_schema(Domain::Hosts);
    let tampered_hosts = FormatSchema {
        domain: Domain::Hosts,
        root_order: format_schema(Domain::ThemeFonts).root_order,
        container_rules: canonical_hosts.container_rules,
    };
    assert!(matches!(
        format_source_with_schema(&hosts_path, &hosts, &classifier, &tampered_hosts),
        Err(FormatError::SchemaMismatch {
            expected: Domain::Hosts,
            actual: Domain::Hosts,
            ..
        })
    ));

    let group_path = domain_path(Domain::GroupRootRequirements);
    let requirements = SourceText::from("zsh\n");
    assert!(matches!(
        format_source_with_schema(
            &group_path,
            &requirements,
            &classifier,
            format_schema(Domain::FacetRequirements)
        ),
        Err(FormatError::SchemaMismatch {
            expected: Domain::GroupRootRequirements,
            actual: Domain::FacetRequirements,
            ..
        })
    ));

    assert!(
        format_source(
            &group_path,
            &requirements,
            &DomainClassifier::without_groups()
        )
        .is_ok()
    );

    let unknown_path = RepoPath::new("config/extra.dotfile").unwrap();
    let unknown = SourceText::from("z{b=\"2\",a=\"1\"}\na=\"root\"\n");
    let generic = format_source(&unknown_path, &unknown, &classifier).unwrap();
    assert_eq!(
        String::from_utf8(generic.bytes.clone()).unwrap(),
        "z { b = \"2\", a = \"1\" }\na = \"root\"\n"
    );
    let generic_source = SourceText::from_bytes(generic.bytes);
    assert!(
        !format_source(&unknown_path, &generic_source, &classifier)
            .unwrap()
            .changed
    );
    assert!(matches!(
        format_source_with_schema(
            &unknown_path,
            &unknown,
            &classifier,
            format_schema(Domain::Hosts)
        ),
        Err(FormatError::UnclassifiedPath { .. })
    ));

    let payload_path = RepoPath::new("README.md").unwrap();
    assert!(matches!(
        format_source(&payload_path, &SourceText::from("text\n"), &classifier),
        Err(FormatError::UnclassifiedPath { .. })
    ));
}

#[test]
fn property_all_attribute_permutations_converge_and_are_idempotent() {
    let attributes = [
        "@theme=mocha",
        "@description=\"d\"",
        "@deploy=\"copy\"",
        "@destination=\"~/x\"",
    ];
    let mut permutations = Vec::new();
    let mut values = attributes.to_vec();
    permute(&mut values, 0, &mut permutations);
    assert_eq!(permutations.len(), 24);

    let mut canonical = None;
    for permutation in permutations {
        let input = format!("{}\n", permutation.join("\n"));
        let output = assert_idempotent(&input, Domain::FacetRequirements);
        if let Some(expected) = &canonical {
            assert_eq!(&output, expected);
        } else {
            canonical = Some(output);
        }
    }
}

#[test]
fn property_generated_valid_documents_reparse_and_converge() {
    for count in 0..40 {
        let mut input = String::new();
        for index in (0..count).rev() {
            let optional = if index % 3 == 0 { "?" } else { "" };
            input.push_str(&format!("{optional}item{index:02}\n"));
        }
        let output = assert_idempotent(&input, Domain::GroupRootRequirements);
        let source = SourceText::from(output.as_str());
        let parsed = parse(&path(), &source);
        assert!(!parsed.has_errors());
    }
}

#[test]
fn property_format_reparse_preserves_requirement_semantics() {
    let inputs = [
        concat!(
            "@let prefix=\"tool\"\n",
            "?zeta { @pkg=$prefix \"-pkg\", alpha, @family=[\"second\",\"first\"] }\n",
            "@font { @family=[\"Z\",\"A\"], @key=zfont }\n",
            "./\"space dir/file\" { @destination=\"${prefix}/dest\" }\n",
        ),
        concat!(
            "@description=\"literal \\${binding}\"\n",
            "?alpha=\"package\"\n",
            "alpha\n",
            "@extend entity/alpha { @version=\"\\u{0031}\" }\n",
        ),
    ];

    for input in inputs {
        let before = requirement_signature(input);
        let output = assert_idempotent(input, Domain::FacetRequirements);
        let after = requirement_signature(&output);
        assert_eq!(after, before, "semantic projection changed for {input:?}");
    }
}

fn assert_needles_in_order(haystack: &str, needles: &[&str]) {
    let mut cursor = 0;
    for needle in needles {
        let offset = haystack[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} after byte {cursor} in {haystack:?}"));
        cursor += offset + needle.len();
    }
}

fn permute(values: &mut [&str], start: usize, output: &mut Vec<Vec<String>>) {
    if start == values.len() {
        output.push(values.iter().map(|value| (*value).to_owned()).collect());
        return;
    }
    for index in start..values.len() {
        values.swap(start, index);
        permute(values, start + 1, output);
        values.swap(start, index);
    }
}

fn requirement_signature(input: &str) -> Vec<String> {
    let source = SourceText::from(input);
    let parsed = parse(&path(), &source);
    assert!(
        !parsed.has_errors(),
        "projection input must parse: {input:?}"
    );
    project_entries(parsed.ast(&source).entries())
}

fn project_entries(entries: Vec<Entry<'_>>) -> Vec<String> {
    let mut projected: Vec<_> = entries.into_iter().map(project_entry).collect();
    // Requirement-domain entry order is canonicalized by semantic category;
    // values inside lists remain ordered in `project_value`.
    projected.sort();
    projected
}

fn project_entry(entry: Entry<'_>) -> String {
    match entry {
        Entry::Let(declaration) => format!(
            "let:{}:{}",
            declaration.name().unwrap_or(""),
            declaration
                .value()
                .map(|value| project_string(&value))
                .unwrap_or_default()
        ),
        Entry::Extend(extension) => {
            let target = extension.target();
            format!(
                "extend:{}/{}:{}",
                target.and_then(|target| target.namespace()).unwrap_or(""),
                target.and_then(|target| target.name()).unwrap_or(""),
                extension
                    .block()
                    .map(project_block)
                    .unwrap_or_else(|| "[]".to_owned())
            )
        }
        Entry::Attribute(attribute) => format!(
            "attribute:{}:{}",
            attribute.name().unwrap_or(""),
            attribute
                .value()
                .map(project_value)
                .unwrap_or_else(|| "missing".to_owned())
        ),
        Entry::SigilBlock(resource) => format!(
            "resource:{}:{}:{}",
            resource.optional(),
            resource.name().unwrap_or(""),
            resource
                .block()
                .map(project_block)
                .unwrap_or_else(|| "[]".to_owned())
        ),
        Entry::Named(named) => format!(
            "named:{}:{}:{}:{}",
            named.optional(),
            named.name().unwrap_or(""),
            named
                .value()
                .map(project_value)
                .unwrap_or_else(|| "none".to_owned()),
            named
                .block()
                .map(project_block)
                .unwrap_or_else(|| "[]".to_owned())
        ),
        Entry::Path(path) => format!(
            "path:{}:{}:{}",
            path.optional(),
            path.decoded_path().unwrap_or_default(),
            path.block()
                .map(project_block)
                .unwrap_or_else(|| "[]".to_owned())
        ),
        Entry::Error(_) => "error".to_owned(),
    }
}

fn project_block(block: Block<'_>) -> String {
    format!("{:?}", project_entries(block.entries()))
}

fn project_value(value: Value<'_>) -> String {
    match value {
        Value::String(string) => format!("string:{}", project_string(&string)),
        Value::Reference(reference) => format!("reference:{}", reference.name().unwrap_or("")),
        Value::List(list) => {
            let values: Vec<_> = list.values().into_iter().map(project_value).collect();
            format!("list:{values:?}")
        }
    }
}

fn project_string(expression: &StringExpr<'_>) -> String {
    enum Piece {
        Literal(String),
        Interpolation(String),
    }

    let mut pieces = Vec::new();
    let push_literal = |pieces: &mut Vec<Piece>, text: &str| match pieces.last_mut() {
        Some(Piece::Literal(literal)) => literal.push_str(text),
        Some(Piece::Interpolation(_)) | None => pieces.push(Piece::Literal(text.to_owned())),
    };
    for atom in expression.atoms() {
        match atom {
            Atom::String { data, .. } => {
                for segment in &data.expect("parse-valid string side data").segments {
                    match segment {
                        StringSegment::Literal { text, .. } => push_literal(&mut pieces, text),
                        StringSegment::Interpolation { name, .. } => {
                            pieces.push(Piece::Interpolation(name.clone()));
                        }
                    }
                }
            }
            Atom::Var(variable) => pieces.push(Piece::Interpolation(
                variable.name().unwrap_or("").to_owned(),
            )),
        }
    }
    pieces
        .into_iter()
        .map(|piece| match piece {
            Piece::Literal(text) => format!("L{}:{text}", text.chars().count()),
            Piece::Interpolation(name) => format!("I{}:{name}", name.len()),
        })
        .collect::<Vec<_>>()
        .join("|")
}
