#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::Arc;

use dotfmt_css::config::Config;
use dotfmt_css::dialect::Dialect;
use dotfmt_css::format;
use dotfmt_tailwind::{OrderTable, Sorter};

fn sorter() -> Sorter {
    Sorter::new(
        Arc::new(OrderTable::from_pairs([
            ("m-2".to_owned(), Some(1)),
            ("flex".to_owned(), Some(2)),
            ("p-4".to_owned(), Some(3)),
            ("block".to_owned(), Some(4)),
            ("bg-[url(\"/a b.svg\")]".to_owned(), Some(5)),
            ("content-[\";\"]".to_owned(), Some(6)),
            ("grid-cols-[repeat(2,minmax(0,1fr))]".to_owned(), Some(7)),
            ("hover:bg-red-500".to_owned(), Some(8)),
            ("md:flex".to_owned(), Some(9)),
        ])),
        dotfmt_tailwind::Config::default(),
    )
}

fn tailwind_config() -> Config {
    let mut config = Config {
        tailwind_sorter: Some(sorter()),
        ..Config::default()
    };
    config.tailwind.mode = dotfmt_tailwind::Mode::On;
    config
}

fn run_with(config: &Config, input: &str, dialect: Dialect) -> String {
    format(Path::new("style.css"), input, config, dialect).unwrap()
}

fn run(input: &str, dialect: Dialect) -> String {
    run_with(&tailwind_config(), input, dialect)
}

#[test]
fn tailwind_v4_at_rules_keep_their_structure_and_sort_apply() {
    let input = r#"@import "tailwindcss";
@theme{--color-brand:oklch(0.7 0.1 200);--spacing:0.25rem;}
@utility flexy{display:flex;@apply p-4   m-2;}
@custom-variant dark (&:where(.dark,.dark *));
@layer components{.card{@apply p-4   m-2;}}
"#;
    assert_eq!(
        run(input, Dialect::Css),
        r#"@import "tailwindcss";
@theme {
  --color-brand: oklch(0.7 0.1 200);
  --spacing: 0.25rem;
}
@utility flexy {
  display: flex;
  @apply m-2 p-4;
}
@custom-variant dark (&:where(.dark,.dark *));
@layer components {
  .card {
    @apply m-2 p-4;
  }
}
"#
    );
}

#[test]
fn nesting_container_property_and_starting_style_are_formatted() {
    let input = r#".card{color:red;&:hover{color:blue;}@media (width >= 40rem){@apply p-4 m-2;}@container card (width > 400px){&>.title{font-weight:bold;}}}
@property --x{syntax:"<length>";inherits:false;initial-value:0px;}
@starting-style{.toast{opacity:0;}}
"#;
    assert_eq!(
        run(input, Dialect::Css),
        r#".card {
  color: red;
  &:hover {
    color: blue;
  }
  @media (width >= 40rem) {
    @apply m-2 p-4;
  }
  @container card (width > 400px) {
    & > .title {
      font-weight: bold;
    }
  }
}
@property --x {
  syntax: "<length>";
  inherits: false;
  initial-value: 0px;
}
@starting-style {
  .toast {
    opacity: 0;
  }
}
"#
    );
}

#[test]
fn scss_and_less_format_and_sort_apply() {
    let scss = r#"$gap: .5rem;
.card {
  color: red;
  &:hover { @apply p-4 m-2; }
  @include mixin($gap);
}
"#;
    assert_eq!(
        run(scss, Dialect::Scss),
        r#"$gap: 0.5rem;
.card {
  color: red;
  &:hover {
    @apply m-2 p-4;
  }
  @include mixin($gap);
}
"#
    );

    let less = r#"@gap: .5rem;
.card {
  &:hover { @apply p-4 m-2; }
  .mixin();
}
"#;
    assert_eq!(
        run(less, Dialect::Less),
        r#"@gap: 0.5rem;
.card {
  &:hover {
    @apply m-2 p-4;
  }
  .mixin();
}
"#
    );
}

#[test]
fn sass_indented_syntax_sorts_apply() {
    assert_eq!(
        run(
            ".card\n  color: red\n  &:hover\n    @apply p-4 m-2\n",
            Dialect::Sass
        ),
        ".card\n  color: red\n  &:hover\n    @apply m-2 p-4\n"
    );
}

#[test]
fn every_apply_rule_is_sorted() {
    assert_eq!(
        run("a{@apply p-4   m-2;@apply flex   block;}", Dialect::Css),
        "a {\n  @apply m-2 p-4;\n  @apply flex block;\n}\n"
    );
}

#[test]
fn important_annotations_stay_at_the_end() {
    assert_eq!(
        run("a{@apply p-4 m-2 !important;}", Dialect::Css),
        "a {\n  @apply m-2 p-4 !important;\n}\n"
    );
    assert_eq!(
        run("a{@apply p-4 m-2 ! important;}", Dialect::Css),
        "a {\n  @apply m-2 p-4 ! important;\n}\n"
    );
    assert_eq!(
        run("a{@apply m-2 p-4!;}", Dialect::Css),
        "a {\n  @apply p-4! m-2;\n}\n"
    );
    assert_eq!(
        run("a{@apply m-2 !p-4;}", Dialect::Css),
        "a {\n  @apply !p-4 m-2;\n}\n"
    );
}

#[test]
fn arbitrary_values_with_spaces_and_delimiters_stay_whole() {
    assert_eq!(
        run("a{@apply p-4 bg-[url('/a b.svg')] flex;}", Dialect::Css),
        "a {\n  @apply flex p-4 bg-[url(\"/a b.svg\")];\n}\n"
    );
    assert_eq!(
        run("a{@apply p-4 content-[';'] flex;}", Dialect::Css),
        "a {\n  @apply flex p-4 content-[\";\"];\n}\n"
    );
    assert_eq!(
        run(
            "a{@apply p-4 grid-cols-[repeat(2,minmax(0,1fr))] flex;}",
            Dialect::Css
        ),
        "a {\n  @apply flex p-4 grid-cols-[repeat(2,minmax(0,1fr))];\n}\n"
    );
    assert_eq!(
        run(r"a{@apply p-4 foo\;bar m-2;}", Dialect::Css),
        "a {\n  @apply foo\\;bar m-2 p-4;\n}\n"
    );
}

#[test]
fn interpolation_and_variables_disable_sorting_safely() {
    assert_eq!(
        run("a { @apply p-4 #{$extra} m-2; }", Dialect::Scss),
        "a {\n  @apply p-4 #{$extra} m-2;\n}\n"
    );
    assert_eq!(
        run("a { @apply #{$classes}; }", Dialect::Scss),
        "a {\n  @apply #{$classes};\n}\n"
    );
    assert_eq!(
        run("a { @apply p-4 $extra m-2; }", Dialect::Scss),
        "a {\n  @apply p-4 $extra m-2;\n}\n"
    );
    assert_eq!(
        run("a { @apply p-4 @{extra} m-2; }", Dialect::Less),
        "a {\n  @apply p-4 @{extra} m-2;\n}\n"
    );
    assert_eq!(
        run("a { @apply p-4 @extra m-2; }", Dialect::Less),
        "a {\n  @apply p-4 @extra m-2;\n}\n"
    );
}

#[test]
fn comments_strings_and_property_values_are_not_apply_rules() {
    let input = r#"/* @apply p-4 m-2; */
a {
  content: "@apply p-4 m-2;";
  --utility: @apply p-4 m-2;
  @apply p-4 m-2;
}
"#;
    assert_eq!(
        run(input, Dialect::Css),
        r#"/* @apply p-4 m-2; */
a {
  content: "@apply p-4 m-2;";
  --utility: @apply p-4 m-2;
  @apply m-2 p-4;
}
"#
    );
}

#[test]
fn comments_trailing_an_apply_rule_survive_sorting() {
    assert_eq!(
        run("a{@apply p-4 m-2/* note */;}", Dialect::Css),
        "a {\n  @apply m-2 p-4; /* note */\n}\n"
    );
    assert_eq!(
        run("a\n  @apply p-4 m-2 // note\n", Dialect::Sass),
        "a\n  @apply m-2 p-4 // note\n"
    );
    assert_eq!(
        run("a{@apply p-4 /* interior */ m-2;}", Dialect::Css),
        "a {\n  @apply p-4  /* interior */m-2;\n}\n"
    );
}

#[test]
fn sorting_is_a_noop_without_a_sorter_or_when_disabled() {
    let input = "a{@apply   p-4   m-2;}";
    assert_eq!(
        format(
            Path::new("style.css"),
            input,
            &Config::default(),
            Dialect::Auto
        )
        .unwrap(),
        "a {\n  @apply p-4 m-2;\n}\n"
    );

    let mut disabled = tailwind_config();
    disabled.tailwind.mode = dotfmt_tailwind::Mode::Off;
    assert_eq!(
        run_with(&disabled, input, Dialect::Css),
        "a {\n  @apply p-4 m-2;\n}\n"
    );
}

#[test]
fn prettier_ignore_protects_a_rule_and_a_file() {
    assert_eq!(
        run(
            "/* prettier-ignore */\na{color:red}\nb{ color : blue }",
            Dialect::Css
        ),
        "/* prettier-ignore */\na{color:red}\nb {\n  color: blue;\n}\n"
    );
    assert_eq!(
        run(
            "/* prettier-ignore-file */\na{color:red}\nb{ color : blue }",
            Dialect::Css
        ),
        "/* prettier-ignore-file */\na{color:red}\nb{ color : blue }\n"
    );
    assert_eq!(
        run(
            "/* prettier-ignore */\na{@apply p-4 m-2;}\nb{@apply p-4 m-2;}",
            Dialect::Css
        ),
        "/* prettier-ignore */\na{@apply p-4 m-2;}\nb {\n  @apply m-2 p-4;\n}\n"
    );
    assert_eq!(
        run(
            "/* prettier-ignore-file */\na{@apply p-4 m-2;}",
            Dialect::Css
        ),
        "/* prettier-ignore-file */\na{@apply p-4 m-2;}\n"
    );
}

#[test]
fn apply_sorting_is_idempotent_in_every_dialect() {
    for (dialect, input) in [
        ("css", "a{@apply p-4   m-2 flex;}"),
        ("scss", "a { @apply p-4   m-2 flex; }"),
        ("sass", "a\n  @apply p-4   m-2 flex\n"),
        ("less", "a { @apply p-4   m-2 flex; }"),
    ] {
        let dialect = Dialect::parse(dialect, false).unwrap();
        let once = run(input, dialect);
        let twice = run(&once, dialect);
        assert_eq!(once, twice, "{dialect:?}");
    }
}
