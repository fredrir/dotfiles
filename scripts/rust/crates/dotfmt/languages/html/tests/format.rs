#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::Arc;

use dotfmt_html::{Config, Dialect, format};
use dotfmt_tailwind::{OrderTable, Sorter};
use markup_fmt::config::Quotes;

fn run(path: &str, input: &str, config: &Config, dialect: Dialect) -> String {
    let path = Path::new(path);
    let output = format(path, input, config, dialect).unwrap();
    assert_eq!(
        format(path, &output, config, dialect).unwrap(),
        output,
        "formatting must settle after one pass for {input:?}"
    );
    output
}

fn html(input: &str) -> String {
    run("document.html", input, &Config::default(), Dialect::Html)
}

fn ordered_sorter() -> Sorter {
    let table = OrderTable::from_pairs([
        ("p-4".to_owned(), Some(10)),
        ("flex".to_owned(), Some(5)),
        ("m-4".to_owned(), Some(1)),
        ("block".to_owned(), Some(2)),
        ("bg-red-500".to_owned(), Some(20)),
    ]);
    Sorter::new(Arc::new(table), dotfmt_tailwind::Config::default())
}

fn tailwind(attributes: &[&str]) -> Config {
    Config {
        tailwind: dotfmt_tailwind::Config {
            mode: dotfmt_tailwind::Mode::On,
            attributes: attributes.iter().map(|value| (*value).to_owned()).collect(),
            ..dotfmt_tailwind::Config::default()
        },
        tailwind_sorter: Some(ordered_sorter()),
        ..Config::default()
    }
}

#[test]
fn formats_html_and_settles_after_one_pass() {
    assert_eq!(
        html("<div><span>text</span></div>"),
        "<div><span>text</span></div>\n"
    );
    assert_eq!(
        html("<div>\n<span>x</span>\n</div>"),
        "<div>\n  <span>x</span>\n</div>\n"
    );
    assert_eq!(
        html("<!DOCTYPE html><html><body><p>x</p></body></html>"),
        "<!DOCTYPE html>\n<html><body><p>x</p></body></html>\n"
    );
    assert_eq!(
        html("<!-- note --><div>x</div>"),
        "<!-- note --><div>x</div>\n"
    );
}

#[test]
fn formats_container_children_without_corrupting_optional_tag_boundaries() {
    assert_eq!(
        html("<body><div></div></body>"),
        "<body><div></div></body>\n"
    );
    assert_eq!(
        html("<body><section></section></body>"),
        "<body><section></section></body>\n"
    );
    assert_eq!(
        html("<body><table><tr><td>x</td></tr></table></body>"),
        "<body><table><tr><td>x</td></tr></table></body>\n"
    );
}

#[test]
fn formats_the_standard_vite_entry_document() {
    let output = html(
        r#"<!doctype html><html><head><title>x</title></head><body><div id="root"></div><script type="module" src="/src/main.tsx"></script></body></html>"#,
    );
    assert!(output.contains(r#"<div id="root"></div>"#), "{output}");
    assert!(output.contains("</body>"), "{output}");
    assert!(output.ends_with("</html>\n"), "{output}");
}

#[test]
fn still_repairs_omitted_optional_end_tags() {
    let output = html("<ul><li>a<li>b</ul>");
    assert_eq!(output, "<ul>\n  <li>a</li>\n  <li>b</li>\n</ul>\n");
    let output = html("<html><head><title>x</title><body><p>y</p></body></html>");
    assert!(output.contains("</head>"), "{output}");
}

#[test]
fn resolves_the_dialect_from_the_path_when_configured_as_auto() {
    let output = run(
        "component.vue",
        r#"<template><div class="p-4 flex">x</div></template>"#,
        &Config::default(),
        Dialect::Auto,
    );
    assert!(output.contains(r#"<div class="p-4 flex">x</div>"#));
    assert_eq!(
        run(
            "component.svelte",
            "<script>let a=1</script><p>{a}</p>",
            &Config::default(),
            Dialect::Auto,
        ),
        "<script>\n  let a = 1;\n</script><p>{a}</p>\n"
    );

    let angular = "@if (show) {<p>{{value}}</p>}\n";
    let detected = run(
        "app.component.html",
        angular,
        &Config::default(),
        Dialect::Auto,
    );
    assert!(detected.contains("@if (show) {"), "{detected}");
    assert_eq!(
        detected,
        run(
            "component.html",
            angular,
            &Config::default(),
            Dialect::Angular
        )
    );
}

#[test]
fn formats_vue_svelte_and_astro_templates() {
    let vue = run(
        "component.vue",
        r#"<template><div :class="{ active: isActive }">x</div></template><script setup lang="ts">let a:number=1</script><style scoped lang="scss">.a{.b{color:red}}</style>"#,
        &Config::default(),
        Dialect::Vue,
    );
    assert!(vue.contains(r#":class="{ active: isActive }""#), "{vue}");
    assert!(vue.contains("let a: number = 1;"), "{vue}");
    assert!(vue.contains("color: red;"), "{vue}");

    let svelte = run(
        "component.svelte",
        r#"<script lang="ts">let a:number=1</script><div class="p-4 flex">{a}</div>"#,
        &Config::default(),
        Dialect::Svelte,
    );
    assert!(svelte.contains("let a: number = 1;"), "{svelte}");
    assert!(
        svelte.contains(r#"<div class="p-4 flex">{a}</div>"#),
        "{svelte}"
    );

    let astro = run(
        "component.astro",
        "---\nconst a:number=1\n---\n<div class=\"p-4 flex\">{a}</div>\n<style lang=\"scss\">.a{.b{color:red}}</style>",
        &Config::default(),
        Dialect::Astro,
    );
    assert!(astro.contains("const a: number = 1;"), "{astro}");
    assert!(
        astro.contains("<div class=\"p-4 flex\">{a}</div>"),
        "{astro}"
    );
    assert!(astro.contains("color: red;"), "{astro}");
}

#[test]
fn embedded_style_and_script_receive_language_hints() {
    let css = html(r#"<style>.a{color:red}</style>"#);
    assert_eq!(css, "<style>\n  .a {\n    color: red;\n  }\n</style>\n");

    let scss = html(r#"<style lang="scss">.a{.b{color:red}}</style>"#);
    assert!(
        scss.contains("<style lang=\"scss\">\n  .a {\n    .b {\n      color: red;"),
        "{scss}"
    );

    let typescript = html(r#"<script lang="ts">let a:number=1</script>"#);
    assert!(typescript.contains("let a: number = 1;"), "{typescript}");

    let tsx = html(r#"<script lang="tsx">const a=<div className="x"/></script>"#);
    assert!(tsx.contains("const a = <div className=\"x\" />;"), "{tsx}");

    let javascript = html("<script>let a=1</script>");
    assert!(javascript.contains("let a = 1;"), "{javascript}");

    let module = html(r#"<script type="module" lang="ts">export const a:number=1</script>"#);
    assert!(module.contains("export const a: number = 1;"), "{module}");

    let typescript_alias = html(r#"<script lang="typescript">let a:number=1</script>"#);
    assert!(
        typescript_alias.contains("let a: number = 1;"),
        "{typescript_alias}"
    );

    let mime_style = html(r#"<style lang="text/scss">.a{.b{color:red}}</style>"#);
    assert!(mime_style.contains("color: red;"), "{mime_style}");

    let empty_style = html(r#"<style lang="">.a{color:red}</style>"#);
    assert!(empty_style.contains("color: red;"), "{empty_style}");

    let empty_script = html(r#"<script lang="">let a=1</script>"#);
    assert!(empty_script.contains("let a = 1;"), "{empty_script}");

    let unknown_style = html(r#"<style lang="unknown">.a{color:red}</style>"#);
    assert!(unknown_style.contains("color: red;"), "{unknown_style}");

    let unknown_script = html(r#"<script lang="unknown">let a=1</script>"#);
    assert!(unknown_script.contains("let a = 1;"), "{unknown_script}");

    let json = html(r#"<script type="application/json">[1,2,3]</script>"#);
    assert!(json.contains("[1,2,3]"), "{json}");
}

#[test]
fn invalid_embedded_snippets_are_preserved_instead_of_failing_the_document() {
    let input = "<div>\n<script>let = ;</script>\n<span>x</span>\n</div>";
    let output = html(input);
    assert!(output.contains("let = ;"), "{output}");
    assert!(output.contains("  <span>x</span>"), "{output}");
    assert!(output.ends_with("</div>\n"), "{output}");

    let input = "<div>\n<style>.a { color: </style>\n<span>x</span>\n</div>";
    let output = html(input);
    assert!(output.contains("<span>x</span>"), "{output}");
    assert!(output.ends_with("</div>\n"), "{output}");
}

#[test]
fn sorts_static_class_attributes_and_multiple_attributes() {
    let config = tailwind(&[]);
    assert_eq!(
        run(
            "x.html",
            r#"<div class="p-4 flex m-4"></div>"#,
            &config,
            Dialect::Html,
        ),
        "<div class=\"m-4 flex p-4\"></div>\n"
    );
    assert_eq!(
        run(
            "x.html",
            r#"<div class="p-4 flex" className="block m-4"></div>"#,
            &config,
            Dialect::Html,
        ),
        "<div class=\"flex p-4\" className=\"m-4 block\"></div>\n"
    );
}

#[test]
fn tailwind_sorting_requires_both_enabled_mode_and_a_sorter() {
    let disabled = Config {
        tailwind: dotfmt_tailwind::Config {
            mode: dotfmt_tailwind::Mode::Off,
            ..dotfmt_tailwind::Config::default()
        },
        tailwind_sorter: Some(ordered_sorter()),
        ..Config::default()
    };
    assert_eq!(
        run(
            "x.html",
            r#"<div class="p-4 flex"></div>"#,
            &disabled,
            Dialect::Html,
        ),
        "<div class=\"p-4 flex\"></div>\n"
    );

    let auto = Config {
        tailwind_sorter: Some(ordered_sorter()),
        ..Config::default()
    };
    assert_eq!(
        run(
            "x.html",
            r#"<div class="p-4 flex"></div>"#,
            &auto,
            Dialect::Html,
        ),
        "<div class=\"flex p-4\"></div>\n"
    );
}

#[test]
fn sorts_custom_tailwind_attributes_without_dropping_class() {
    let config = tailwind(&["tw"]);
    assert_eq!(
        run(
            "x.html",
            r#"<div class="p-4 flex" tw="block m-4"></div>"#,
            &config,
            Dialect::Html,
        ),
        "<div class=\"flex p-4\" tw=\"m-4 block\"></div>\n"
    );
}

#[test]
fn handles_quotes_inside_attribute_values_and_arbitrary_tailwind_values() {
    let config = tailwind(&[]);
    assert_eq!(
        run(
            "x.html",
            r#"<div data-note="a > b" class="p-4 flex"></div>"#,
            &config,
            Dialect::Html,
        ),
        "<div data-note=\"a > b\" class=\"flex p-4\"></div>\n"
    );
    assert_eq!(
        run(
            "x.html",
            r#"<div class="p-4 content-['x y'] flex"></div>"#,
            &config,
            Dialect::Html,
        ),
        "<div class=\"content-['x y'] flex p-4\"></div>\n"
    );
}

#[test]
fn skips_dynamic_template_and_svelte_class_values() {
    let config = tailwind(&[]);
    for input in [
        r#"<div class="p-4 {{ variant }} flex"></div>"#,
        r#"<div class="p-4 {active ? 'flex' : ''}"></div>"#,
        r#"<div class="{% if active %}p-4 flex{% endif %}"></div>"#,
        r#"<div class="p-4 <% variant %> flex"></div>"#,
    ] {
        assert_eq!(
            run("x.html", input, &config, Dialect::Svelte),
            format!("{input}\n")
        );
    }
}

#[test]
fn skips_comments_and_raw_script_style_text() {
    let config = tailwind(&[]);
    assert_eq!(
        run(
            "x.html",
            r#"<!-- <div class="p-4 flex"> --><div class="p-4 flex"></div>"#,
            &config,
            Dialect::Html,
        ),
        "<!-- <div class=\"p-4 flex\"> --><div class=\"flex p-4\"></div>\n"
    );

    let script = run(
        "x.html",
        r#"<script>const x = '<div class="p-4 flex">';</script>"#,
        &config,
        Dialect::Html,
    );
    assert!(
        script.contains(r#"const x = '<div class="p-4 flex">';"#),
        "{script}"
    );

    let style = run(
        "x.html",
        r#"<style>.x::after { content: '<div class="p-4 flex">'; }</style>"#,
        &config,
        Dialect::Html,
    );
    assert!(style.contains("p-4 flex"), "{style}");

    let pre = run(
        "x.html",
        r#"<pre><code><div class="p-4 flex"></div></code></pre>"#,
        &config,
        Dialect::Html,
    );
    assert!(pre.contains(r#"<div class="p-4 flex"></div>"#), "{pre}");
}

#[test]
fn sorts_vue_class_array_and_object_string_literals_only() {
    let config = tailwind(&[]);

    let literal = run(
        "x.vue",
        r#"<div :class="'p-4 flex'"></div>"#,
        &config,
        Dialect::Vue,
    );
    assert!(literal.contains(r#""flex p-4""#), "{literal}");

    let array = run(
        "x.vue",
        r#"<div :class="['p-4 flex', dynamic, 'block m-4']"></div>"#,
        &config,
        Dialect::Vue,
    );
    assert!(
        array.contains(r#"["flex p-4", dynamic, "m-4 block"]"#),
        "{array}"
    );

    let ternary = run(
        "x.vue",
        r#"<div :class="['p-4 flex', cond ? 'block m-4' : 'bg-red-500']"></div>"#,
        &config,
        Dialect::Vue,
    );
    assert!(
        ternary.contains(r#"cond ? "block m-4" : "bg-red-500""#),
        "{ternary}"
    );

    let object = run(
        "x.vue",
        r#"<div :class="{ 'p-4': active, flex: shown }"></div>"#,
        &config,
        Dialect::Vue,
    );
    assert!(
        object.contains(r#"{ "p-4": active, flex: shown }"#),
        "{object}"
    );

    let bound = run(
        "x.vue",
        r#"<div v-bind:class="['p-4 flex']"></div>"#,
        &config,
        Dialect::Vue,
    );
    assert!(bound.contains(r#"["flex p-4"]"#), "{bound}");
}

#[test]
fn respects_quotes_entities_and_ignore_directives() {
    let config = Config {
        quotes: Quotes::Single,
        ..tailwind(&[])
    };
    assert_eq!(
        run(
            "x.html",
            r#"<div class="p-4 flex"></div>"#,
            &config,
            Dialect::Html,
        ),
        "<div class='flex p-4'></div>\n"
    );

    let config = tailwind(&[]);
    assert_eq!(
        run(
            "x.html",
            r#"<div class="p-4&#32;flex m-4"></div>"#,
            &config,
            Dialect::Html,
        ),
        "<div class=\"p-4&#32;flex m-4\"></div>\n"
    );

    let file = "<!-- prettier-ignore -->\n<div class=\"p-4 flex\"><span class=\"block m-4\">x</span></div>\n";
    assert_eq!(run("x.html", file, &config, Dialect::Html), file);
    let file_directive = "<!-- prettier-ignore-file -->\n<div class=\"p-4 flex\"><span class=\"block m-4\">x</span></div>\n";
    assert_eq!(
        run("x.html", file_directive, &config, Dialect::Html),
        file_directive
    );

    let element = "<!-- prettier-ignore -->\n<div class=\"p-4 flex\"><span class=\"block m-4\">x</span></div>";
    assert_eq!(
        run("x.html", element, &config, Dialect::Html),
        format!("{element}\n")
    );
}

#[test]
fn inserts_omitted_html_end_tags_instead_of_rejecting_valid_html() {
    let config = Config::default();
    let output = run(
        "a.html",
        "<ul><li>one<li>two</ul>\n<p>one<p>two\n<table><tr><td>a<td>b<tr><td>c</table>\n",
        &config,
        Dialect::Html,
    );
    assert!(output.contains("<li>one</li>"), "{output}");
    assert!(output.contains("<li>two</li>"), "{output}");
    assert!(output.contains("<td>a</td>"), "{output}");
    assert!(output.contains("<tr>"), "{output}");
    assert!(!output.contains("<li>one<li>"), "{output}");
}

#[test]
fn tailwind_ignore_directives_are_scoped_to_their_element() {
    let config = tailwind(&[]);
    let leading = "<!-- prettier-ignore -->\n<div class=\"p-4 flex\">a</div>\n<div class=\"p-4 flex\">b</div>\n";
    assert_eq!(
        run("x.html", leading, &config, Dialect::Html),
        "<!-- prettier-ignore -->\n<div class=\"p-4 flex\">a</div>\n<div class=\"flex p-4\">b</div>\n"
    );

    let file = "<!-- prettier-ignore-file -->\n<div class=\"p-4 flex\">a</div>\n<div class=\"p-4 flex\">b</div>\n";
    assert_eq!(run("x.html", file, &config, Dialect::Html), file);

    let middle = "<div class=\"p-4 flex\">a</div>\n<!-- prettier-ignore -->\n<div class=\"p-4 flex\">b</div>\n<div class=\"p-4 flex\">c</div>\n";
    assert_eq!(
        run("x.html", middle, &config, Dialect::Html),
        "<div class=\"flex p-4\">a</div>\n<!-- prettier-ignore -->\n<div class=\"p-4 flex\">b</div>\n<div class=\"flex p-4\">c</div>\n"
    );
}

#[test]
fn tailwind_sorting_reflows_and_settles_class_attributes() {
    let config = tailwind(&[]);
    let wrapped = "<div\n  class=\"p-4\n flex   m-2\n text-sm\">x</div>\n";
    assert_eq!(
        run("x.html", wrapped, &config, Dialect::Html),
        "<div class=\"m-2 text-sm flex p-4\">\n  x\n</div>\n"
    );

    let siblings = "<section>\n  <div\n    class=\"p-4\n flex\">a</div>\n  <span\n    class=\"m-4\n block\">b</span>\n</section>\n";
    let output = run("x.html", siblings, &config, Dialect::Html);
    assert!(output.contains("<div class=\"flex p-4\">"), "{output}");
    assert!(
        output.contains("<span class=\"m-4 block\">b</span>"),
        "{output}"
    );

    for (path, dialect) in [
        ("x.vue", Dialect::Vue),
        ("x.svelte", Dialect::Svelte),
        ("x.astro", Dialect::Astro),
    ] {
        assert_eq!(
            run(path, wrapped, &config, dialect),
            "<div class=\"m-2 text-sm flex p-4\">\n  x\n</div>\n",
            "{path}"
        );
    }
}

#[test]
fn deeply_nested_markup_is_rejected_without_overflowing_the_stack() {
    let deep = format!("{}x{}", "<div>".repeat(600), "</div>".repeat(600));
    let error = format(Path::new("deep.html"), &deep, &Config::default(), Dialect::Html)
        .expect_err("deep nesting must be a diagnostic");
    assert!(
        error.to_string().contains("nesting exceeds"),
        "{error}"
    );

    let allowed = format!("{}x{}", "<div>".repeat(100), "</div>".repeat(100));
    assert!(format(Path::new("deep.html"), &allowed, &Config::default(), Dialect::Html).is_ok());

    // Text and raw script/style content are not markup nesting.
    let script = format!(
        "<script>const s = \"{}\";</script>",
        "<div>".repeat(500)
    );
    assert!(format(Path::new("deep.html"), &script, &Config::default(), Dialect::Html).is_ok());
}

#[test]
fn preserved_snippets_do_not_collide_with_user_sentinel_text() {
    let sentinel = "\u{e000}0\u{e001}";
    let input = format!(
        "<p>before {sentinel} after</p>\n<script type=\"text/x-template\"><li a>\n</script>\n"
    );
    let output = format(Path::new("x.html"), &input, &Config::default(), Dialect::Html).unwrap();
    assert!(output.contains(sentinel), "{output}");
    assert!(output.contains("text/x-template"), "{output}");
    assert!(output.contains("<li a>"), "{output}");
    assert!(
        !output.contains("before <script"),
        "the preserved script leaked into the paragraph: {output}"
    );
}

#[test]
fn commented_out_markup_does_not_count_toward_the_nesting_limit() {
    let comment = format!(
        "<!-- {}x{} -->",
        "<div>".repeat(600),
        "</div>".repeat(600)
    );
    let output = format(
        Path::new("x.html"),
        &comment,
        &Config::default(),
        Dialect::Html,
    )
    .unwrap();
    assert!(output.contains("<!--"), "{output}");
}

#[test]
fn adjacent_json_ld_scripts_stay_on_their_own_lines() {
    let input = "<script type=\"application/ld+json\">{\"a\":1}</script>\n<script type=\"application/ld+json\">{\"b\":2}</script>\n";
    let output = format(Path::new("x.html"), input, &Config::default(), Dialect::Html).unwrap();
    assert_eq!(output.matches("<script").count(), 2, "{output}");
    assert!(!output.contains("</script> <script"), "{output}");
    assert!(output.contains("{\"a\":1}"), "{output}");
    assert!(output.contains("{\"b\":2}"), "{output}");
}

#[test]
fn adjacent_import_maps_stay_on_their_own_lines() {
    let input = "<script type=\"importmap\">{\"imports\":{\"a\":\"/a.js\"}}</script>\n<script type=\"importmap\">{\"imports\":{\"b\":\"/b.js\"}}</script>\n";
    let output = format(Path::new("x.html"), input, &Config::default(), Dialect::Html).unwrap();
    assert_eq!(output.matches("<script").count(), 2, "{output}");
    assert!(!output.contains("</script> <script"), "{output}");
}
