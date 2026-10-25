use super::*;
use testkit::tree;

#[test]
fn modules_and_data_files_are_named_by_their_path_without_extension() {
    let root = tree(&[
        "lib/util.jq=def greet(x): x; def bye: 1;",
        "lib/conf.json={}",
        "lib/sub/sub.jq=def s: 1;",
        "lib/.hidden/h.jq=",
        "lib/notes.txt=",
    ]);
    let names: Vec<(String, bool)> = list(&[root.path().join("lib")])
        .into_iter()
        .map(|module| (module.name, module.data))
        .collect();
    assert_eq!(
        names,
        [
            ("conf".into(), true),
            ("sub/sub".into(), false),
            ("sub".into(), false),
            ("util".into(), false),
        ]
    );
}

#[test]
fn modules_resolve_like_jq_and_list_their_functions() {
    let root = tree(&[
        "first/util.jq=def a: 1;",
        "second/util.jq=def b: 1;",
        "second/sub/sub.jq=def greet(x; y): x;",
    ]);
    let dirs = [root.path().join("first"), root.path().join("second")];
    assert_eq!(
        resolve(&dirs, "util", false),
        Some(root.path().join("first/util.jq"))
    );
    let sub = resolve(&dirs, "sub", false).unwrap();
    assert_eq!(functions(&sub), [("greet".into(), 2)]);
    assert_eq!(resolve(&dirs, "util", true), None);
}

#[test]
fn the_search_path_starts_with_library_dirs_and_skips_missing_ones() {
    let root = tree(&["home/.jq/", "work/lib/"]);
    let ctx = Context::testing(&root.path().join("home"), &root.path().join("work"), &[]);
    let dirs = search_path(&ctx, None, &["lib".into(), "missing".into()]);
    assert_eq!(
        dirs,
        [root.path().join("work/lib"), root.path().join("home/.jq")]
    );
}
