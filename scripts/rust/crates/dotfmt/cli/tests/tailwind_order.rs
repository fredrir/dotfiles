#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;

const SOURCE: &str = "export const A=()=> <div className=\"p-4 flex m-2 text-sm\"/>;\n";

fn project(order: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("project directory");
    fs::write(
        root.path().join("package.json"),
        "{\"name\":\"noproj\",\"private\":true}\n",
    )
    .expect("package.json");
    fs::write(
        root.path().join("dotfmt.dotfile"),
        format!(
            "typescript {{\n tailwind = on\n tailwind_order = {order}\n type_aware = false\n}}\n"
        ),
    )
    .expect("dotfmt.dotfile");
    fs::create_dir_all(root.path().join("src")).expect("src");
    fs::write(root.path().join("src/a.tsx"), SOURCE).expect("source");
    root
}

#[test]
fn project_order_without_a_local_tailwind_is_a_configuration_error() {
    let root = project("project");
    let output = dotfmt(root.path())
        .args(["--stdin", "src/a.tsx"])
        .stdin(SOURCE)
        .run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.contains("tailwind order:"), "{output:?}");
    assert!(
        output
            .stderr
            .contains("tailwindcss is not installed in this project"),
        "{output:?}"
    );
}

#[test]
fn auto_order_falls_back_to_the_rust_sorter_without_a_local_tailwind() {
    let root = project("auto");
    let output = dotfmt(root.path())
        .args(["--stdin", "src/a.tsx"])
        .stdin(SOURCE)
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stderr.contains("using the Rust order"), "{output:?}");
    assert!(output.stdout.contains("className="), "{output:?}");
}
