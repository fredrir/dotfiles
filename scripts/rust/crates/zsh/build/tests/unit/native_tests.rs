use super::*;

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn only_path_helper_in_sh_style_is_emulated() {
    assert!(emulates(&names(&["/usr/libexec/path_helper", "-s"])));
    assert!(emulates(&names(&["path_helper", "-s"])));
    assert!(!emulates(&names(&["path_helper", "-c"])));
    assert!(!emulates(&names(&["path_helper"])));
    assert!(!emulates(&names(&["locale", "-s"])));
}

#[test]
fn names_sort_by_numeric_prefix_then_bytes() {
    let mut sorted = names(&[
        "homebrew",
        "100-rvictl",
        "10-pmk",
        "9-x",
        "10-cryptex",
        "Zed",
    ]);
    sort_names(&mut sorted).unwrap();
    assert_eq!(
        sorted,
        names(&[
            "9-x",
            "10-cryptex",
            "10-pmk",
            "100-rvictl",
            "Zed",
            "homebrew"
        ])
    );
}

#[test]
fn names_path_helper_orders_ambiguously_are_refused() {
    assert!(sort_names(&mut names(&["1a", "01a"])).is_err());
    assert!(sort_names(&mut names(&["+5", ","])).is_err());
    assert!(sort_names(&mut names(&[" 5"])).is_err());
}

#[test]
fn entries_are_read_in_order_once_each() {
    let root = testkit::tree_pairs(&[
        ("etc/paths", "/usr/bin\n\n/bin\n"),
        ("etc/paths.d/10-b", "/b\n/usr/bin\n"),
        ("etc/paths.d/9-a", "/a"),
        ("etc/paths.d/sub/ignored", "/sub\n"),
        ("etc/manpaths", "/usr/share/man\n"),
    ]);
    std::os::unix::fs::symlink("/etc/paths", root.path().join("etc/paths.d/link")).unwrap();
    let helper = PathHelper::load(root.path().to_str().unwrap()).unwrap();
    assert_eq!(helper.path, names(&["/usr/bin", "/bin", "/a", "/b"]));
    assert_eq!(helper.manpath, names(&["/usr/share/man"]));
    assert!(
        helper
            .sources
            .contains(&root.path().join("etc/paths.d/9-a"))
    );
    assert!(
        !helper
            .sources
            .contains(&root.path().join("etc/paths.d/link"))
    );
}

#[test]
fn entries_eval_would_reinterpret_are_refused() {
    for entry in ["/a b", "/a$b", "/a:b", "/a\"b"] {
        let root = testkit::tree_pairs(&[("etc/paths", entry)]);
        assert!(
            PathHelper::load(root.path().to_str().unwrap()).is_err(),
            "{entry}"
        );
    }
}

#[test]
fn the_emulation_parses_and_keeps_the_original_as_fallback() {
    let helper = PathHelper {
        path: names(&["/usr/bin"]),
        manpath: Vec::new(),
        sources: Vec::new(),
    };
    let code = helper.code("eval `/usr/libexec/path_helper -s`\n");
    assert!(crate::script::parse(&code).is_ok(), "{code}");
    assert!(
        code.contains("\neval `/usr/libexec/path_helper -s`\nelse\n"),
        "{code}"
    );
    assert!(code.contains("entries=(/usr/bin)\n"), "{code}");
}
