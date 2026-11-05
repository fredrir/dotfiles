#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn extensionless_scss_alias_formats_variables_and_nesting() {
    let root = tree_pairs(&[("dotfmt.dotfile", "css {\n tailwind = off\n}\n")]);
    let input = "$brand:red;\n.a{color:$brand;&:hover{color:darken($brand,10%)}}\n";
    let output = dotfmt(root.path()).args(["-l", "scss"]).stdin(input).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        "$brand: red;\n.a {\n  color: $brand;\n  &:hover {\n    color: darken($brand, 10%);\n  }\n}\n"
    );
    assert!(output.stderr.is_empty(), "{output:?}");

    let output = dotfmt(root.path()).args(["-l", "css"]).stdin(input).run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.contains("syntax error"), "{output:?}");
}

#[test]
fn formats_css_scss_and_tailwind_apply() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "css {\n width = 80\n tailwind = on\n tailwind_order = rust\n}\n",
        ),
        (
            "app.css",
            "@import \"tailwindcss\";\n.card{display:grid;gap:1rem}\n.btn{@apply p-4 flex items-center gap-2}\n",
        ),
        (
            "style.scss",
            "$brand:red;\n.a{color:$brand;&:hover{color:darken($brand,10%)}}\n",
        ),
    ]);
    let output = dotfmt(root.path()).args(["app.css", "style.scss"]).run();
    assert!(output.success(), "{output:?}");
    let css = fs::read_to_string(root.path().join("app.css")).unwrap();
    assert!(css.contains(".card {\n  display: grid;\n  gap: 1rem;\n}"));
    assert!(css.contains("@apply flex items-center gap-2 p-4"), "{css}");
    let scss = fs::read_to_string(root.path().join("style.scss")).unwrap();
    assert!(scss.contains("&:hover"), "{scss}");
    assert!(scss.contains("color: darken($brand, 10%);"), "{scss}");
}
