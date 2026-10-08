use super::*;

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn npm_spec() -> Spec {
    let helps = ["npm-install", "npm-uninstall", "", "npm-run", "npm-exec"]
        .iter()
        .zip(["install", "uninstall", "update", "run", "exec"])
        .map(|(file, command)| {
            if file.is_empty() {
                Help::default()
            } else {
                Help::parse(&fixture(file), &["npm", command])
            }
        })
        .collect();
    Spec::assemble(Manager::Npm, Help::parse(&fixture("npm"), &["npm"]), helps)
}

#[test]
fn commands_map_to_their_manager_and_runners() {
    assert_eq!(Manager::from_command("pn"), Some((Manager::Pnpm, false)));
    assert_eq!(Manager::from_command("pnx"), Some((Manager::Pnpm, true)));
    assert_eq!(Manager::from_command("bunx"), Some((Manager::Bun, true)));
    assert_eq!(Manager::from_command("npx"), Some((Manager::Npm, true)));
    assert_eq!(Manager::from_command("deno"), None);
}

#[test]
fn a_role_answers_to_its_command_and_every_alias() {
    let spec = npm_spec();
    for word in ["install", "i", "add", "isntall"] {
        assert_eq!(
            spec.role_of(word).map(|role| role.role),
            Some(Role::Add),
            "{word}"
        );
    }
    assert_eq!(spec.role_of("rm").map(|role| role.role), Some(Role::Remove));
    assert_eq!(spec.role_of("x").map(|role| role.role), Some(Role::Dlx));
    assert!(spec.role_of("publish").is_none());
}

#[test]
fn a_role_carries_the_flags_of_its_help() {
    let spec = npm_spec();
    let add = spec.role(Role::Add).expect("add");
    assert!(add.help.flag("--save-dev").is_some());
    assert!(
        spec.role(Role::Update)
            .is_some_and(|role| role.help.flags.is_empty())
    );
}

#[test]
fn aliases_listed_only_in_the_top_level_help_are_picked_up() {
    let top = Help::parse(&fixture("pnpm"), &["pnpm"]);
    let spec = Spec::assemble(Manager::Pnpm, top, Vec::new());
    assert_eq!(
        spec.role_of("uni").map(|role| role.role),
        Some(Role::Remove)
    );
    assert_eq!(
        spec.role_of("upgrade").map(|role| role.role),
        Some(Role::Update)
    );
}

#[test]
fn without_any_help_the_built_in_aliases_still_apply() {
    let spec = Spec::fallback(Manager::Bun);
    assert_eq!(spec.role_of("a").map(|role| role.role), Some(Role::Add));
    assert_eq!(spec.role_of("x").map(|role| role.role), Some(Role::Dlx));
    assert!(spec.top.commands.is_empty());
}
