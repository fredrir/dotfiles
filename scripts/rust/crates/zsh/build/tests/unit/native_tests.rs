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

#[test]
fn only_atuin_uuid_has_a_native_substitute() {
    assert!(substitute(&names(&["atuin", "uuid"])).is_some());
    assert!(substitute(&names(&["/opt/homebrew/bin/atuin", "uuid"])).is_some());
    assert!(substitute(&names(&["atuin", "uuid", "--help"])).is_none());
    assert!(substitute(&names(&["atuin"])).is_none());
    assert!(substitute(&names(&["uuidgen"])).is_none());
    assert!(substitute(&[]).is_none());
}

fn at(millis: u64) -> SystemTime {
    UNIX_EPOCH + std::time::Duration::from_millis(millis)
}

#[test]
fn atuin_uuid_output_must_have_the_form_the_helper_makes() {
    let made = 0x01a1_20c9_8fed;
    let id = "01a120c98fed74a3beeadec383fcc3b8\n";
    assert!(is_uuid7(id, at(made)));
    assert!(is_uuid7(id, at(made + 59_000)));
    assert!(!is_uuid7(id, at(made + 61_000)));
    for other in [
        "01a120c98fed74a3beeadec383fcc3b8",
        "01a120c98fed74a3beeadec383fcc3b8\n\n",
        "01A120C98FED74A3BEEADEC383FCC3B8\n",
        "01a120c9-8fed-74a3-beea-dec383fcc3b8\n",
        "01a120c98fed44a3beeadec383fcc3b8\n",
        "01a120c98fed78a3beeadec383fcc3b8\n",
        "01a120c98fed74a3ceeadec383fcc3b8\n",
        "01a120c98fed74a3beeadec383fcc3b\n",
    ] {
        assert!(!is_uuid7(other, at(made)), "{other}");
    }
}

#[test]
fn the_expansion_calls_the_helper_and_keeps_the_original_as_fallback() {
    let atuin = substitute(&names(&["atuin", "uuid"])).unwrap();
    let expansion = atuin.expansion("$(atuin uuid)");
    assert_eq!(
        expansion,
        "${${:-$((_zsh_build_uuid7()))}:+${_zsh_build_uuid:-$(atuin uuid)}}"
    );
    let code = format!("export ATUIN_SESSION={expansion}\nids+=(\"{expansion}\")\n");
    assert!(crate::script::parse(&code).is_ok(), "{code}");
}

#[test]
fn the_helper_defines_the_math_function_from_a_proper_random_source() {
    let atuin = substitute(&names(&["atuin", "uuid"])).unwrap();
    let program = crate::script::parse(atuin.helper).unwrap();
    assert_eq!(
        crate::compile::walk::function_names(&program.lists),
        names(&[atuin.function])
    );
    for part in [
        format!("functions -M {} 0 0\n", atuin.function),
        format!("typeset -g {}=\n", atuin.value),
        "now=($epochtime)\n".to_string(),
        "sysread -s 10 random </dev/urandom || return\n".to_string(),
    ] {
        assert!(atuin.helper.contains(&part), "{part}");
    }
    assert!(!atuin.helper.contains("RANDOM"));
}
