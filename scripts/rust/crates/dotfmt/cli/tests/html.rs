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
