use super::*;

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn names(help: &Help) -> Vec<&str> {
    help.flags
        .iter()
        .flat_map(|flag| flag.names.iter().map(String::as_str))
        .collect()
}

fn choices<'a>(help: &'a Help, name: &str) -> Vec<&'a str> {
    help.flag(name)
        .and_then(|flag| flag.value.as_ref())
        .map(|value| value.choices.iter().map(String::as_str).collect())
        .unwrap_or_default()
}

#[test]
fn commander_pipe_aliases_resolve_to_the_canonical_command() {
    let help = Help::parse(&fixture("shadcn/main"), &["shadcn"]);
    let init = help.command("create").expect("create aliases init");
    assert_eq!(init.name, "init");
    assert_eq!(init.aliases, ["create"]);
    assert_eq!(help.command("list").unwrap().name, "search");
    let preset = Help::parse(&fixture("shadcn/preset"), &["shadcn", "preset"]);
    assert_eq!(preset.command("info").unwrap().name, "resolve");
}

#[test]
fn npm_block_flags_pair_short_and_long_names() {
    let help = Help::parse(&fixture("npm-install"), &["npm", "install"]);
    let save = help.flag("-S").expect("-S is listed");
    assert!(save.has("--save"), "{save:?}");
    assert!(
        save.description.starts_with("Save installed packages"),
        "{save:?}"
    );
}

#[test]
fn npm_usage_brackets_add_flags_the_blocks_leave_out() {
    let help = Help::parse(&fixture("npm-install"), &["npm", "install"]);
    let names = names(&help);
    for expected in ["--save-dev", "--no-save", "--save-peer", "--dry-run"] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
    let save_dev = help.flag("--save-dev").expect("--save-dev");
    assert!(
        !save_dev.has("-S"),
        "distinct usage alternatives stay distinct: {save_dev:?}"
    );
}

#[test]
fn npm_placeholder_alternatives_become_choices() {
    let help = Help::parse(&fixture("npm-install"), &["npm", "install"]);
    assert_eq!(choices(&help, "--omit"), ["dev", "optional", "peer"]);
    assert_eq!(
        choices(&help, "--install-strategy"),
        ["hoisted", "nested", "shallow", "linked"]
    );
    assert!(choices(&help, "--min-release-age-exclude").is_empty());
}

#[test]
fn npm_aliases_come_from_the_aliases_line() {
    let help = Help::parse(&fixture("npm-install"), &["npm", "install"]);
    assert!(
        help.aliases.iter().any(|alias| alias == "i"),
        "{:?}",
        help.aliases
    );
    assert!(
        help.aliases.iter().any(|alias| alias == "isntall"),
        "{:?}",
        help.aliases
    );
    let uninstall = Help::parse(&fixture("npm-uninstall"), &["npm", "uninstall"]);
    assert!(
        uninstall.aliases.iter().any(|alias| alias == "rm"),
        "{:?}",
        uninstall.aliases
    );
}

#[test]
fn npm_top_level_lists_every_command() {
    let help = Help::parse(&fixture("npm"), &["npm"]);
    for expected in ["access", "install", "uninstall", "whoami", "exec"] {
        assert!(help.command(expected).is_some(), "{expected} missing");
    }
}

#[test]
fn bun_flags_carry_their_inline_values() {
    let help = Help::parse(&fixture("bun-add"), &["bun", "add"]);
    let config = help.flag("--config").expect("--config");
    assert!(config.has("-c"));
    assert_eq!(
        config
            .value
            .as_ref()
            .map(|value| value.placeholder.as_str()),
        Some("val")
    );
    assert!(help.flag("--no-save").is_some());
    assert_eq!(
        choices(&help, "--backend"),
        ["hardlink", "symlink", "copyfile"]
    );
    assert_eq!(choices(&help, "--linker"), ["isolated", "hoisted"]);
    assert_eq!(help.aliases, ["a"]);
}

#[test]
fn bun_commands_take_aliases_from_their_description() {
    let help = Help::parse(&fixture("bun"), &["bun"]);
    let add = help.command("add").expect("add");
    assert_eq!(add.aliases, ["a"]);
    assert_eq!(add.description, "Add a dependency to package.json");
    assert!(
        help.command("rm")
            .is_some_and(|command| command.name == "remove")
    );
    assert!(
        help.command("lint").is_none(),
        "an example column is not a command"
    );
}

#[test]
fn pnpm_clap_help_joins_aliases_and_descriptions() {
    let help = Help::parse(&fixture("pnpm-add"), &["pnpm", "add"]);
    let prod = help.flag("--prod").expect("--prod");
    assert!(prod.has("--production"), "{prod:?}");
    let dev = help.flag("-D").expect("-D");
    assert!(dev.has("--save-dev"));
    assert!(dev.description.contains("devDependencies"), "{dev:?}");
    let top = Help::parse(&fixture("pnpm"), &["pnpm"]);
    let remove = top.command("uni").expect("remove answers to uni");
    assert_eq!(remove.name, "remove");
    assert!(!remove.description.contains("[aliases"), "{remove:?}");
}

#[test]
fn yarn_lists_commands_as_dashed_items() {
    let help = Help::parse(&fixture("yarn"), &["yarn"]);
    assert!(help.command("add").is_some());
    assert!(help.command("upgrade").is_some());
    let add = Help::parse(&fixture("yarn-add"), &["yarn", "add"]);
    assert!(
        add.flag("--pnp")
            .is_some_and(|flag| flag.has("--enable-pnp"))
    );
}

#[test]
fn pi_main_help_describes_commands_and_short_and_long_flags() {
    let help = Help::parse(&fixture("pi"), &["pi"]);
    let names: Vec<&str> = help
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "install",
            "remove",
            "uninstall",
            "update",
            "list",
            "config",
            "auth"
        ]
    );
    let name = help.flag("-n").expect("-n");
    assert!(name.has("--name"));
    assert!(name.takes_separate_value());
    assert!(help.flag("-nt").is_some_and(|flag| flag.has("--no-tools")));
    assert!(
        help.flag("--mcp-config").is_some(),
        "extension flags count too"
    );
    assert!(help.flag("--").is_none());
}

#[test]
fn pi_value_lists_in_descriptions_become_choices() {
    let help = Help::parse(&fixture("pi"), &["pi"]);
    assert_eq!(
        choices(&help, "--thinking"),
        ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(choices(&help, "--mode"), ["text", "json", "rpc"]);
    assert_eq!(choices(&help, "--tui-mode"), ["regular", "fullscreen"]);
    assert!(choices(&help, "--provider").is_empty());
    assert!(
        choices(&help, "--session").is_empty(),
        "path|id are metavariables"
    );
}

#[test]
fn an_optional_value_does_not_consume_the_next_word() {
    let help = Help::parse(&fixture("pi"), &["pi"]);
    let list = help.flag("--list-models").expect("--list-models");
    assert!(list.value.as_ref().is_some_and(|value| value.optional));
    assert!(!list.takes_separate_value());
}

#[test]
fn pi_subcommand_usage_yields_nested_commands_and_flags() {
    let help = Help::parse(&fixture("pi-auth"), &["pi", "auth"]);
    let names: Vec<&str> = help
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert_eq!(names, ["print-api-key", "print-bearer-token", "check"]);
    for flag in [
        "--provider",
        "--model",
        "--min-expiry",
        "--json",
        "--no-refresh",
    ] {
        assert!(help.flag(flag).is_some(), "{flag} missing");
    }
    assert!(
        help.flag("--provider")
            .is_some_and(Flag::takes_separate_value)
    );
}

#[test]
fn usage_word_alternatives_are_positional_choices() {
    let update = Help::parse(&fixture("pi-update"), &["pi", "update"]);
    assert_eq!(update.positionals, ["self", "pi"]);
    let remove = Help::parse(&fixture("pi-remove"), &["pi", "remove"]);
    assert_eq!(remove.aliases, ["uninstall"]);
    let install = Help::parse(&fixture("pi-install"), &["pi", "install"]);
    assert!(
        install
            .flag("-na")
            .is_some_and(|flag| flag.has("--no-approve"))
    );
    assert_eq!(
        install.flag("-na").map(|flag| flag.description.as_str()),
        Some("Ignore project-local files for this command")
    );
}

#[test]
fn a_flag_found_twice_merges_into_one() {
    let mut help = Help::default();
    help.add_flag(Flag {
        names: vec!["-w".into()],
        ..Flag::default()
    });
    help.add_flag(Flag {
        names: vec!["-w".into(), "--workspace".into()],
        value: Some(Value::new("workspace-name", false)),
        description: "Workspace".into(),
    });
    assert_eq!(help.flags.len(), 1);
    assert_eq!(help.flags[0].names, ["-w", "--workspace"]);
    assert!(help.flags[0].takes_separate_value());
    assert_eq!(
        help.flag("--workspace=web").map(|flag| flag.names.len()),
        Some(2)
    );
}

#[test]
fn path_placeholders_are_recognised() {
    assert!(Value::new("dir", false).names_a_directory());
    assert!(Value::new("path", false).names_a_path());
    assert!(!Value::new("name", false).names_a_path());
}
