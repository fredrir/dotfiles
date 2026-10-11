use super::*;

fn dump(records: &[(&str, &str, &str)]) -> Dump {
    let mut bytes = Vec::new();
    for (table, name, value) in records.iter().chain([&("end", "", "")]) {
        for field in [table, name, value] {
            bytes.extend_from_slice(field.as_bytes());
            bytes.push(0);
        }
    }
    Dump::parse(&bytes).unwrap()
}

fn compare(bundle: &[(&str, &str, &str)], sources: &[(&str, &str, &str)]) -> Vec<String> {
    let expected = Expected {
        completions: vec!["/build/zshrc.completions".to_string()],
    };
    differences(&dump(bundle), &dump(sources), &expected)
        .into_iter()
        .map(|difference| format!("{} {}", difference.table, difference.name))
        .collect()
}

#[test]
fn a_dump_is_records_up_to_its_end() {
    let parsed = dump(&[
        ("aliases", "ll", "ls -l"),
        ("functions", "f", "\tprint\n\tx"),
    ]);
    assert_eq!(parsed.0["aliases"]["ll"], "ls -l");
    assert_eq!(parsed.function("f").unwrap(), "\tprint\n\tx");
    assert!(Dump::parse(b"aliases\0ll\0ls -l\0").is_err());
    assert!(Dump::parse(b"aliases\0ll\0").is_err());
}

#[test]
fn what_only_one_side_has_by_design_is_no_difference() {
    let found = compare(
        &[
            ("functions", "_zsh_build_whence", "\tbuiltin whence"),
            ("functions", "whence", "\t_zsh_build_whence \"$@\""),
            ("parameters", "__zb1", "git version 2.55.0"),
            ("parameters", "__zb0_1", "/x"),
            ("parameters", "__zbv_fd", "11"),
            (
                "math-functions",
                "_zsh_build_uuid7",
                "functions -M _zsh_build_uuid7 0 0",
            ),
            (
                "parameters",
                "fpath",
                "/build/zshrc.completions\n/usr/share/zsh",
            ),
            (
                "parameters",
                "FPATH",
                "/build/zshrc.completions:/usr/share/zsh",
            ),
            (
                "zstyles",
                "zle-line-init widgets",
                "0:user:zle-line-init\n2:_reset",
            ),
            ("parameters", "ATUIN_SESSION", "01a1"),
        ],
        &[
            ("parameters", "ZSH_BUILD_SKIP", "1"),
            ("types", "ZSH_BUILD_SKIP", "scalar-export"),
            ("parameters", "fpath", "/usr/share/zsh"),
            ("parameters", "FPATH", "/usr/share/zsh"),
            (
                "zstyles",
                "zle-line-init widgets",
                "0:user:zle-line-init\n1:_reset",
            ),
            ("parameters", "ATUIN_SESSION", "01a2"),
        ],
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn leftovers_and_changed_values_are_differences() {
    let found = compare(
        &[
            ("functions", "__zb_defer_3", "\tsource x"),
            ("functions", "whence", "\tbuiltin whence -v \"$@\""),
            ("parameters", "fpath", "/usr/share/zsh\n/extra"),
            ("zstyles", "zle-line-init widgets", "1:_b\n2:_a"),
            ("aliases", "ll", "ls -l"),
            ("parameters", "ATUIN_SESSION", "01a1"),
            ("types", "EDITOR", "scalar"),
        ],
        &[
            ("parameters", "fpath", "/usr/share/zsh"),
            ("zstyles", "zle-line-init widgets", "1:_a\n2:_b"),
            ("aliases", "ll", "ls -la"),
            ("types", "EDITOR", "scalar-export"),
        ],
    );
    assert_eq!(
        found,
        [
            "aliases ll",
            "functions __zb_defer_3",
            "functions whence",
            "parameters ATUIN_SESSION",
            "parameters fpath",
            "types EDITOR",
            "zstyles zle-line-init widgets",
        ]
    );
}

#[test]
fn the_queue_machinery_only_the_bundle_loaded_is_no_difference() {
    let residue = [
        ("functions", "add-zle-hook-widget", "\tlocal -a hooktypes"),
        ("functions", "azhw:zle-line-init", "\tlocal -a hook_widgets"),
        ("widgets", "_defer_drain", "user:_defer_drain"),
        ("widgets", "zle-line-init", "user:azhw:zle-line-init"),
        ("zstyles", "zle-hook types", "isearch-exit\nline-init"),
    ];
    assert!(compare(&residue, &[]).is_empty());
    assert_eq!(
        compare(&[("widgets", "zle-line-init", "user:_mine")], &[]),
        ["widgets zle-line-init"]
    );
    assert_eq!(
        compare(&[], &[("widgets", "_defer_drain", "user:_defer_drain")]),
        ["widgets _defer_drain"]
    );
}

#[test]
fn menu_select_wrapped_by_the_deferred_compinit_is_no_difference() {
    let wrapped = "completion:.menu-select:_main_complete";
    assert!(
        compare(
            &[("widgets", "autosuggest-orig-1-menu-select", wrapped)],
            &[
                (
                    "widgets",
                    "autosuggest-orig-1-menu-select",
                    "user:_zsh_autosuggest_orig_menu-select"
                ),
                (
                    "functions",
                    "_zsh_autosuggest_orig_menu-select",
                    "\tzle .menu-select"
                ),
            ],
        )
        .is_empty()
    );
    assert!(
        compare(
            &[("widgets", "menu-select", wrapped)],
            &[("widgets", "menu-select", "builtin")]
        )
        .is_empty()
    );
    assert_eq!(
        compare(
            &[
                ("widgets", "menu-complete", wrapped),
                ("widgets", "menu-select", wrapped)
            ],
            &[
                ("widgets", "menu-complete", "builtin"),
                ("widgets", "menu-select", "user:_mine")
            ],
        ),
        ["widgets menu-complete", "widgets menu-select"]
    );
    assert_eq!(
        compare(
            &[],
            &[(
                "functions",
                "_zsh_autosuggest_orig_menu-select",
                "\tzle .other"
            )]
        ),
        ["functions _zsh_autosuggest_orig_menu-select"]
    );
}

#[test]
fn a_type_without_its_parameter_shows_only_as_the_parameter() {
    let found = compare(&[("parameters", "X", "1"), ("types", "X", "scalar")], &[]);
    assert_eq!(found, ["parameters X"]);
}

#[test]
fn folded_output_stands_for_its_command_substitution() {
    assert!(rewritten(
        "\tlocal v=${\"${__zb3}\"#fzf } \n\teval \"${__zb1}\"",
        "\tlocal v=${\"$(fzf --version)\"#fzf } \n\teval \"$(init --frames)\""
    ));
    assert!(rewritten(
        "\tprint ${__zb2}/bin",
        "\tprint `brew --prefix`/bin"
    ));
    assert!(!rewritten(
        "\tprint ${__zb2}/bin",
        "\tprint $(brew --prefix)/sbin"
    ));
    assert!(!rewritten("\tprint ${__zb2}", "\tprint $HOMEBREW_PREFIX"));
}

#[test]
fn an_inlined_eval_is_a_block_where_the_eval_stood() {
    let sources =
        "\tlocal ready=1 \n\t[[ -n $x ]] && eval \"$(init --zsh)\" || print no\n\tdone=1 ";
    let bundle = "\tlocal ready=1 \n\t[[ -n $x ]] && {\n\t\tinit_fn () {\n\t\t\tprint -r -- initialized\n\t\t}\n\t\tinit_ready=1 \n\t} || print no\n\tdone=1 ";
    assert!(rewritten(bundle, sources));
    assert!(rewritten(
        "\t{\n\t\ta=1 \n\t}\n\t{\n\t\tb=1 \n\t}",
        "\teval \"$(one)\"\n\teval \"$(two)\""
    ));
    assert!(!rewritten(
        "\t[[ -n $x ]] && {\n\t\ta=1 \n\t} || print yes",
        "\t[[ -n $x ]] && eval \"$(init)\" || print no"
    ));
    assert!(!rewritten("\t{\n\t\ta=1 \n\t}", "\teval $(init)"));
}

#[test]
fn native_expansions_stand_for_the_command_they_replace() {
    assert!(rewritten(
        "\t[[ \"${${langinfo[CODESET]}:-$(locale LC_CTYPE)}\" == UTF-8 ]]",
        "\t[[ \"$(locale LC_CTYPE)\" == UTF-8 ]]"
    ));
    assert!(rewritten(
        "\tid=${${:-$((_zsh_build_uuid7()))}:+${_zsh_build_uuid:-$(atuin uuid)}} ",
        "\tid=$(atuin uuid) "
    ));
}

#[test]
fn a_backslash_the_build_puts_before_a_word_runs_the_same() {
    assert!(rewritten(
        "\t\\grep --color=auto x | \\ls -F -l",
        "\tgrep --color=auto x | ls -F -l"
    ));
    assert!(!rewritten("\t\\grep x", "\tgrep --color=auto x"));
    assert!(!rewritten("\tprint \\*", "\tprint *"));
}

#[test]
fn an_alias_zsh_expanded_in_the_sources_stays_a_difference() {
    assert!(!rewritten(
        "\tmkdir -p \"$dir\"\n\tgrep LISTEN",
        "\tmkdir -p -p \"$dir\"\n\tgrep --color=auto LISTEN"
    ));
}

#[test]
fn an_excerpt_shows_where_the_values_part() {
    let difference = |bundle: Option<&str>, sources: Option<&str>| Difference {
        table: "functions".to_string(),
        name: "f".to_string(),
        bundle: bundle.map(str::to_string),
        sources: sources.map(str::to_string),
    };
    assert_eq!(
        difference(
            Some("\ta\n\tgrep x\n\tz"),
            Some("\ta\n\tgrep --color=auto x")
        )
        .excerpt(60),
        ("grep x".to_string(), "grep --color=auto x".to_string())
    );
    assert_eq!(
        difference(None, Some("\tzle .menu-select\n\tx")).excerpt(60),
        ("(none)".to_string(), "zle .menu-select".to_string())
    );
    let long = "command git -c log.showSignature=false log -n 1 | grep -q wip";
    let expanded = long.replace("grep", "grep --color=auto");
    let (left, right) = difference(Some(long), Some(&expanded)).excerpt(30);
    assert_eq!(left, "...se log -n 1 | grep -q wip");
    assert_eq!(right, "...se log -n 1 | grep --color=aut...");
    assert_eq!(
        difference(Some("\u{1b}[0m"), Some("x")).excerpt(60).0,
        "\\u{1b}[0m"
    );
}
