use super::*;
#[test]
fn oracle_all_profiles_and_outputs() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/theme/oracle.json")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    for (path, body) in oracle["sources"].as_object().unwrap() {
        let target = directory.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, body.as_str().unwrap()).unwrap();
    }
    let repo = model::Repository::load(directory.path()).unwrap();
    validate::all(&repo).unwrap();
    let targets = emitters::targets(&repo).unwrap();
    assert_eq!(targets.len(), 40);
    for (name, files) in oracle["expected"].as_object().unwrap() {
        let t = repo.theme(name).unwrap();
        for target in &targets {
            let actual = emitters::emit(&repo, t, target).unwrap();
            let expected = files[&target.path].as_str().unwrap();
            if actual != expected {
                let a = actual.lines().collect::<Vec<_>>();
                let b = expected.lines().collect::<Vec<_>>();
                let mismatch = a
                    .iter()
                    .zip(&b)
                    .position(|(a, b)| a != b)
                    .unwrap_or(a.len().min(b.len()));
                panic!(
                    "{name}: {} line {}\nactual: {:?}\nexpected: {:?}",
                    target.path,
                    mismatch + 1,
                    a.get(mismatch),
                    b.get(mismatch)
                );
            }
        }
    }
}

#[test]
fn color_math_and_expression_contracts() {
    use color::Color;
    for (a, b, amount, expected) in [
        ("#1e1e2e", "#cdd6f4", 0.25, "#44465a"),
        ("#1e1e2e", "#cdd6f4", -0.1, "#100f1e"),
        ("#eff1f5", "#4c4f69", 0.15, "#d5d7df"),
        ("#40a02b", "#4c4f69", 0.2, "#459042"),
    ] {
        assert_eq!(
            Color::parse(a)
                .unwrap()
                .mix(Color::parse(b).unwrap(), amount)
                .to_string(),
            expected
        );
    }
    assert_eq!(Color::BLACK.contrast(Color::WHITE), 21.);
    for rgb in [
        [0, 0, 0],
        [255, 255, 255],
        [1, 2, 3],
        [128, 64, 255],
        [64, 160, 43],
    ] {
        let c = Color(rgb);
        assert_eq!(Color::from_lab(c.lab()), c);
    }
    for expr in [
        "",
        "bg/",
        "bg/abc",
        "bg/2.5",
        "magenta/150%",
        "magenta/-10%",
        "magenta/1.%",
        "magenta/.5%",
        "red~yellow",
        "red~yellow~blue/500",
        "bg/40%/250",
        "bg/250/40%/10",
        "contrast(bg/250",
        "on(bg,nan)",
        "readable(fg,bg,inf)",
        "bg)(",
    ] {
        assert!(expression::Expr::parse(expr).is_err(), "{expr}");
    }
    let expr = expression::Expr::parse("on(bg/250)/12.5%").unwrap();
    let r = expr
        .evaluate(
            &mut |_| unreachable!(),
            Color::parse("#1e1e2e").unwrap(),
            Color::parse("#cdd6f4").unwrap(),
        )
        .unwrap();
    assert_eq!(r.alpha, Some(0.125));
    assert!(
        expression::Expr::parse("contrast(red/30%)")
            .unwrap()
            .evaluate(&mut |_| Ok(Color([255, 0, 0])), Color::BLACK, Color::WHITE)
            .is_err()
    );
}
#[test]
fn selection_edit_keeps_comments_and_removes_only_requested_overrides() {
    let selected = selection::Selection {
        groups: std::collections::BTreeMap::from([
            (
                "shared".into(),
                std::collections::BTreeMap::from([
                    ("theme".into(), "mocha".into()),
                    ("obsidian".into(), "latte".into()),
                ]),
            ),
            (
                "linux/kde".into(),
                std::collections::BTreeMap::from([("theme".into(), "latte".into())]),
            ),
        ]),
    };
    let source = "# themes\nshared {\n  theme = mocha  # base\n  obsidian = latte\n}\n\nlinux/kde {\n  theme = latte\n}\n";
    let global = selection::switched(source, &selected, "shared", "theme", true, "latte");
    assert_eq!(global, "# themes\nshared {\n  theme = latte  # base\n}\n");
    assert_eq!(
        selection::switched(source, &selected, "shared", "theme", false, "mocha"),
        source
    );
    let package = selection::switched(source, &selected, "shared", "zsh", false, "latte");
    assert!(package.contains("  obsidian = latte\n  zsh = latte\n}"));
    assert!(package.contains("theme = mocha  # base"));
    assert_eq!(selected.for_path("shared/obsidian/theme.css"), "latte");
    assert_eq!(selected.for_path("shared/zsh/theme.zsh"), "mocha");
    assert_eq!(selected.for_path("linux/kde/plasma/kdeglobals"), "latte");
    assert_eq!(
        selected.for_path("linux/arch/fastfetch/config.jsonc"),
        "mocha"
    );
}

fn fixture() -> tempfile::TempDir {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/theme/oracle.json")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    for (path, body) in oracle["sources"].as_object().unwrap() {
        let target = directory.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, body.as_str().unwrap()).unwrap();
    }
    directory
}
#[test]
fn expression_oracle_preserves_dark_light_negative_ladders_nested_functions_and_alpha() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/theme/expressions.json")).unwrap();
    for case in oracle["cases"].as_array().unwrap() {
        let source = case["expression"].as_str().unwrap();
        let expression = expression::Expr::parse(source).unwrap();
        let value = expression
            .evaluate(
                &mut |name| {
                    color::Color::parse(
                        oracle["palette"][name]
                            .as_str()
                            .ok_or_else(|| format!("unknown palette color: {name}"))?,
                    )
                },
                color::Color::parse(case["background"].as_str().unwrap()).unwrap(),
                color::Color::parse(case["foreground"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
        assert_eq!(
            value.color.to_string(),
            case["color"],
            "{source} on {}",
            case["background"]
        );
        assert_eq!(value.alpha, case["alpha"].as_f64(), "{source}");
    }
}
#[test]
fn schema_rejects_unknown_missing_and_wrong_types_and_normalizes_colors() {
    let root = fixture();
    let repo = model::Repository::load(root.path()).unwrap();
    let raw = repo.theme("latte").unwrap().raw.clone();
    type Mutation = fn(&mut serde_json::Value);
    let cases: [(Mutation, &str); 7] = [
        (|v| v["tokens"] = serde_json::json!({}), "unknown 'tokens'"),
        (
            |v| {
                v["ui"].as_object_mut().unwrap().remove("surface");
            },
            "missing 'surface'",
        ),
        (
            |v| v["ansi"]["normal"]["orange"] = "#123456".into(),
            "unknown 'orange'",
        ),
        (|v| v["ui"]["primary"] = "blue".into(), "six-digit hex"),
        (|v| v["dark"] = "true".into(), "dark must be"),
        (|v| v["name"] = "  ".into(), "name must be"),
        (|v| v["ansi"] = false.into(), "ansi: must be a table"),
    ];
    for (mutate, message) in cases {
        let mut raw = raw.clone();
        mutate(&mut raw);
        let error = model::Theme::new("test", raw, repo.data.clone())
            .err()
            .unwrap();
        assert!(error.contains(message), "{error}");
    }
    let mut raw = raw;
    raw["name"] = "  trimmed  ".into();
    raw["ui"]["primary"] = "#ABCDEF".into();
    let t = model::Theme::new("test", raw, repo.data.clone()).unwrap();
    assert_eq!(t.name, "trimmed");
    assert_eq!(t.color("ui.primary").unwrap().to_string(), "#abcdef");
    assert_eq!(t.raw["ui"]["primary"], "#abcdef");
    assert_eq!(t.color("sidebar").unwrap(), t.color("ui.accent").unwrap());
    assert!(t.color("surface_alt").is_err());
    assert!(repo.theme("missing").err().unwrap().contains("mocha"));
}
#[test]
fn all_contrast_pairs_have_unique_states_and_tmux_indexed_colors_remain_readable() {
    let root = fixture();
    let repo = model::Repository::load(root.path()).unwrap();
    for t in repo.themes.values() {
        let pairs = validate::pairs(t).unwrap();
        let mut states = std::collections::HashSet::new();
        for pair in &pairs {
            assert!(
                states.insert((&pair.area, &pair.state)),
                "duplicate {}.{}",
                pair.area,
                pair.state
            );
            assert!(pair.passes(), "{}.{}", pair.area, pair.state);
        }
        let colors = emitters::tmux::colors(t).unwrap();
        for name in ["primary", "fg", "muted", "success"] {
            let i = emitters::tmux::indexed(colors[name], colors["bg"]).unwrap();
            assert!((16..256).contains(&i));
            let rgb = if i >= 232 {
                [8 + 10 * (i - 232); 3]
            } else {
                let ramp = [0, 95, 135, 175, 215, 255];
                let n = i - 16;
                [ramp[n / 36], ramp[n / 6 % 6], ramp[n % 6]]
            };
            assert!(color::Color(rgb.map(|v| v as u8)).contrast(colors["bg"]) >= 4.5);
        }
        assert!(!emitters::tmux::render(t).unwrap().contains("#("));
    }
}
#[test]
fn marker_ini_and_lua_edits_preserve_surroundings_and_escaping() {
    assert_eq!(
        render::between(
            "before\n  # theme:x\n  old\n  # theme:x:end\nafter",
            "x",
            &["new".into(), "".into(), "last".into()]
        )
        .unwrap(),
        "before\n  # theme:x\n  new\n\n  last\n  # theme:x:end\nafter"
    );
    assert!(render::between("no markers", "x", &[]).is_err());
    assert!(render::between("# theme:x:end\n# theme:x", "x", &[]).is_err());
    assert_eq!(
        render::section("[A]\nold=1\n\n\n[B]\ny=2\n", "A", &["new=2".into()]).unwrap(),
        "[A]\nnew=2\n\n\n[B]\ny=2\n"
    );
    assert_eq!(
        render::ini("[A]\na=1\nz=2\n", "A", "m", "3").unwrap(),
        "[A]\na=1\nm=3\nz=2\n"
    );
    assert!(render::ini("[A]\na=1\n", "B", "x", "3").is_err());
    assert_eq!(render::lua_key("regular_key"), "regular_key");
    assert_eq!(render::lua_key("end"), "[\"end\"]");
    assert_eq!(render::lua_key("a-b"), "[\"a-b\"]");
    assert_eq!(render::lua("a\"b\\c"), "\"a\\\"b\\\\c\"");
}
#[test]
fn cascade_routes_group_package_and_profile_defaults() {
    let root = fixture();
    let repo = model::Repository::load(root.path()).unwrap();
    let selection = selection::Selection {
        groups: std::collections::BTreeMap::from([
            (
                "shared".into(),
                std::collections::BTreeMap::from([("theme".into(), "mocha".into())]),
            ),
            (
                "linux/kde".into(),
                std::collections::BTreeMap::from([("plasma".into(), "latte".into())]),
            ),
        ]),
    };
    let groups = std::collections::BTreeMap::from([
        ("linux/arch".into(), vec!["fastfetch".into()]),
        (
            "linux/kde".into(),
            vec!["panel-colorizer".into(), "plasma".into()],
        ),
        ("shared".into(), vec!["nvim".into(), "zsh".into()]),
    ]);
    for (options, expected) in [
        (vec![], Some("menu")),
        (vec!["sync"], None),
        (vec!["preview"], Some("profile")),
        (vec!["preview", "mocha"], None),
        (vec!["switch"], Some("scope")),
        (vec!["switch", "global"], Some("profile")),
        (vec!["switch", "linux/arch"], Some("profile")),
        (vec!["switch", "linux/kde"], Some("package")),
        (vec!["switch", "linux/kde", "plasma"], Some("profile")),
        (vec!["switch", "linux/kde", "plasma", "latte"], None),
    ] {
        let mut picks = Vec::new();
        for option in options {
            let column = preview::next(&repo, &selection, &groups, "", &picks).unwrap();
            let index = column.options.iter().position(|s| s == option).unwrap();
            picks.push(preview::Pick {
                kind: column.kind,
                option: option.into(),
                index,
            });
        }
        let column = preview::next(&repo, &selection, &groups, "", &picks);
        assert_eq!(column.as_ref().map(|c| c.kind), expected);
        if picks.last().is_some_and(|p| p.option == "plasma") {
            let c = column.unwrap();
            assert_eq!(c.options[c.index], "latte");
        }
    }
    assert_eq!(
        preview::next(&repo, &selection, &groups, "switch", &[])
            .unwrap()
            .kind,
        "scope"
    );
}
