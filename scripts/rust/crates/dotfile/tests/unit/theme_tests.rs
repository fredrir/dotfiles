use super::*;

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
    assert_eq!(selected.for_path("shared/zsh/20-theme.zsh"), "mocha");
    assert_eq!(selected.for_path("linux/kde/plasma/kdeglobals"), "latte");
    assert_eq!(
        selected.for_path("linux/arch/fastfetch/config.jsonc"),
        "mocha"
    );
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
