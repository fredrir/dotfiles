#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn formats_html_with_embedded_css_and_typescript() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "html {\n width = 80\n}\ncss {}\ntypescript {}\n",
        ),
        (
            "page.html",
            "<div><style>.a{color:red}</style><script>const x={a:1}</script><span class=\"p-4 flex\">x</span></div>",
        ),
    ]);
    let output = dotfmt(root.path()).arg("page.html").run();
    assert!(output.success(), "{output:?}");
    let html = fs::read_to_string(root.path().join("page.html")).unwrap();
    assert!(html.contains(".a {\n      color: red;\n    }"), "{html}");
    assert!(html.contains("const x = { a: 1 };"), "{html}");
}

const EMBEDDED_CONFIGURATION: &str = "\
{
  width = 40
  quote_style = single
}
css {
  quotes = single
  hex_case = upper
  width = 200
}
typescript {
  quote_style = single
  semicolons = as-needed
}
html {}
";

const SCRIPT_ARRAY: &str = "const arr = [\"aaaa\",\"bbbb\",\"cccc\",\"dddd\",\"eeee\",\"ffff\"]";

#[test]
fn embedded_script_and_style_use_their_language_settings() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", EMBEDDED_CONFIGURATION),
        (
            "page.html",
            &format!(
                "<script lang=\"ts\">{SCRIPT_ARRAY}</script>\n<style>.a{{content:\"x y\";color:#abcdef}}</style>\n"
            ),
        ),
    ]);
    let output = dotfmt(root.path()).arg("page.html").run();
    assert!(output.success(), "{output:?}");
    let html = fs::read_to_string(root.path().join("page.html")).unwrap();
    assert!(
        html.contains("<script lang='ts'>\n  const arr = [\n    'aaaa',"),
        "{html}"
    );
    // Semicolons are as-needed and lines wrap at the TypeScript width.
    assert!(html.contains("    'ffff',\n  ]\n</script>"), "{html}");
    assert!(
        html.contains(".a {\n    content: 'x y';\n    color: #ABCDEF;\n  }"),
        "{html}"
    );
}

#[test]
fn javascript_scripts_without_a_language_fall_back_to_typescript() {
    for config in [
        // The opt-in javascript block is absent, so the typescript block applies.
        EMBEDDED_CONFIGURATION.to_owned(),
        // A disabled javascript block keeps the embedded script on typescript options.
        format!("{EMBEDDED_CONFIGURATION}javascript {{\n  enabled = false\n}}\n"),
    ] {
        let root = tree_pairs(&[
            ("dotfmt.dotfile", config.as_str()),
            ("page.html", &format!("<script>{SCRIPT_ARRAY}</script>\n")),
        ]);
        let output = dotfmt(root.path()).arg("page.html").run();
        assert!(output.success(), "{output:?}");
        let html = fs::read_to_string(root.path().join("page.html")).unwrap();
        assert!(
            html.contains("const arr = [\n    'aaaa',"),
            "{config:?}: {html}"
        );
        assert!(!html.contains(";\n</script>"), "{config:?}: {html}");
    }
}

#[test]
fn an_enabled_javascript_block_overrides_typescript_for_plain_scripts() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            &format!(
                "{EMBEDDED_CONFIGURATION}javascript {{\n  quote_style = double\n  semicolons = always\n}}\n"
            ),
        ),
        (
            "page.html",
            &format!(
                "<script>{SCRIPT_ARRAY}</script>\n<script lang=\"ts\">{SCRIPT_ARRAY}</script>\n"
            ),
        ),
    ]);
    let output = dotfmt(root.path()).arg("page.html").run();
    assert!(output.success(), "{output:?}");
    let html = fs::read_to_string(root.path().join("page.html")).unwrap();
    let (plain, typed) = html.split_once("<script lang='ts'>").unwrap();
    assert!(plain.contains("\"aaaa\""), "{html}");
    assert!(plain.contains(";\n</script>"), "{html}");
    assert!(typed.contains("'aaaa'"), "{html}");
}

#[test]
fn framework_components_format_embedded_blocks_with_their_language_settings() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", EMBEDDED_CONFIGURATION),
        (
            "App.vue",
            "<template><div>{{ msg }}</div></template>\n<script setup lang=\"ts\">const msg:string=\"hi\"</script>\n<style scoped lang=\"scss\">.a{.b{content:\"x\"}}</style>\n",
        ),
        (
            "App.svelte",
            "<script>let msg = \"hi\"</script>\n<style>.a{content:\"x\"}</style>\n<h1>{msg}</h1>\n",
        ),
        (
            "page.astro",
            "---\nconst title:string=\"hi\"\n---\n<h1>{title}</h1>\n<style>.a{content:\"x\"}</style>\n",
        ),
    ]);
    let output = dotfmt(root.path())
        .arg("App.vue")
        .arg("App.svelte")
        .arg("page.astro")
        .run();
    assert!(output.success(), "{output:?}");
    for path in ["App.vue", "App.svelte", "page.astro"] {
        let source = fs::read_to_string(root.path().join(path)).unwrap();
        assert!(source.contains("content: 'x';"), "{path}: {source}");
    }
    let vue = fs::read_to_string(root.path().join("App.vue")).unwrap();
    assert!(vue.contains("const msg: string = 'hi'"), "{vue}");
    assert!(vue.contains(".b {\n      content: 'x';\n    }"), "{vue}");
}

#[test]
fn html_only_configuration_keeps_embedded_defaults() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "html {}\n"),
        (
            "page.html",
            "<script>const a = [\"x\"]</script><style>.a{content:\"x\";color:#ABCDEF}</style>\n",
        ),
    ]);
    let output = dotfmt(root.path()).arg("page.html").run();
    assert!(output.success(), "{output:?}");
    let html = fs::read_to_string(root.path().join("page.html")).unwrap();
    assert!(html.contains("const a = [\"x\"];"), "{html}");
    assert!(html.contains("content: \"x\";"), "{html}");
    assert!(html.contains("color: #abcdef;"), "{html}");
}
