use std::fs;
use std::path::Path;

use super::*;
use testkit::{executable, tree};

fn install() -> testkit::TempDir {
    let root = tree(&["bin/", "help/"]);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shadcn");
    let mut script = format!(
        "#!/bin/sh\nprintf 'call\\n' >> '{}/calls'\ncase \"$*\" in\n",
        root.path().display()
    );
    for entry in fs::read_dir(fixtures).unwrap() {
        let entry = entry.unwrap();
        let path = root.path().join("help").join(entry.file_name());
        fs::copy(entry.path(), &path).unwrap();
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let command = if stem == "main" {
            "--help".to_string()
        } else {
            format!("{} --help", stem.replace('-', " "))
        };
        script.push_str(&format!("'{command}') /bin/cat '{}' ;;\n", path.display()));
    }
    script.push_str("*) exit 1 ;;\nesac\n");
    executable(&root.path().join("bin/shadcn"), &script);
    root
}

fn completed(root: &Path, input: &str) -> Reply {
    let path = root.join("bin").to_string_lossy().to_string();
    let ctx = Context::testing(root, root, &[("PATH", &path)]);
    let words: Vec<String> = input.split(' ').map(String::from).collect();
    let current = words.len();
    crate::complete(&ctx, "shadcn", &Line::new(words, current, None))
}

#[test]
fn commands_and_aliases_come_from_installed_help() {
    let root = install();
    let reply = completed(root.path(), "shadcn ");
    for command in [
        "init", "create", "search", "list", "add", "registry", "preset", "mcp",
    ] {
        assert!(reply.values().contains(&command), "{command}: {reply:?}");
    }
    assert_eq!(
        completed(root.path(), "shadcn init --").plain(),
        completed(root.path(), "shadcn create --").plain()
    );
    assert_eq!(
        completed(root.path(), "shadcn search --").plain(),
        completed(root.path(), "shadcn list --").plain()
    );
}

#[test]
fn nested_commands_and_aliases_select_their_own_flags() {
    let root = install();
    assert_eq!(completed(root.path(), "shadcn mcp ").values(), ["init"]);
    assert_eq!(
        completed(root.path(), "shadcn preset ").values(),
        ["decode", "resolve", "info", "url", "open"]
    );
    let reply = completed(root.path(), "shadcn preset info --");
    assert!(reply.values().contains(&"--json"));
    assert!(reply.values().contains(&"--cwd="));
    assert!(!reply.values().contains(&"--client="));
    assert_eq!(
        completed(root.path(), "shadcn mcp --cwd project init --client ").values(),
        ["claude", "cursor", "vscode", "codex", "opencode"]
    );
}

#[test]
fn component_arguments_repeat_and_options_do_not_consume_them() {
    let root = install();
    for input in [
        "shadcn add ",
        "shadcn add button ",
        "shadcn add --cwd project ",
        "shadcn docs ",
    ] {
        let reply = completed(root.path(), input);
        assert!(reply.values().contains(&"button"), "{input}: {reply:?}");
        assert!(reply.values().contains(&"dialog"), "{input}: {reply:?}");
        assert!(reply.plain().ends_with("files\n"));
    }
}

#[test]
fn option_values_support_separate_equals_and_attached_short_forms() {
    let root = install();
    for input in [
        "shadcn create --base ",
        "shadcn docs --base=",
        "shadcn docs -b ",
        "shadcn docs -br",
    ] {
        assert_eq!(
            completed(root.path(), input).values(),
            ["base", "radix", "aria"],
            "{input}"
        );
    }
    assert!(
        completed(root.path(), "shadcn docs --base=")
            .plain()
            .starts_with("skip\t--base=\n")
    );
    assert!(
        completed(root.path(), "shadcn docs -br")
            .plain()
            .starts_with("skip\t-b\n")
    );
    assert!(
        completed(root.path(), "shadcn create --template ")
            .values()
            .contains(&"astro")
    );
}

#[test]
fn optional_values_allow_another_option_and_do_not_become_positionals() {
    let root = install();
    assert_eq!(
        completed(root.path(), "shadcn add --diff ").plain(),
        "files\n"
    );
    assert!(
        completed(root.path(), "shadcn add --diff --")
            .values()
            .contains(&"--dry-run")
    );
    assert!(
        completed(root.path(), "shadcn init --preset base-nova ")
            .values()
            .contains(&"button")
    );
    assert!(
        completed(root.path(), "shadcn init --preset --base ")
            .values()
            .contains(&"radix")
    );
}

#[test]
fn comma_separated_values_keep_the_option_prefix_and_exclude_chosen_values() {
    let root = install();
    let reply = completed(root.path(), "shadcn apply --only=theme,");
    assert!(reply.plain().starts_with("skip\t--only=theme,\n"));
    assert_eq!(reply.values(), ["font"]);
    let reply = completed(root.path(), "shadcn list --type ui,block,");
    assert!(reply.plain().starts_with("skip\tui,block,\n"));
    assert!(!reply.values().contains(&"ui"));
    assert!(!reply.values().contains(&"block"));
    assert!(reply.values().contains(&"hook"));
}

#[test]
fn paths_use_the_shells_directory_or_file_completion() {
    let root = install();
    for input in [
        "shadcn add -c ",
        "shadcn add --path ",
        "shadcn build --output ",
    ] {
        assert_eq!(completed(root.path(), input).plain(), "dirs\n", "{input}");
    }
    for input in [
        "shadcn build ",
        "shadcn registry validate ",
        "shadcn migrate icons ",
    ] {
        assert_eq!(completed(root.path(), input).plain(), "files\n", "{input}");
    }
}

#[test]
fn help_and_option_separator_respect_argument_context() {
    let root = install();
    assert!(
        completed(root.path(), "shadcn help ")
            .values()
            .contains(&"add")
    );
    assert_eq!(
        completed(root.path(), "shadcn help mcp ").values(),
        ["init"]
    );
    assert_eq!(
        completed(root.path(), "shadcn registry help ").values(),
        ["add", "validate", "help"]
    );
    let reply = completed(root.path(), "shadcn add -- --");
    assert!(!reply.values().contains(&"--dry-run"));
    assert!(reply.values().contains(&"button"));
    assert!(
        completed(root.path(), "shadcn unknown ")
            .values()
            .is_empty()
    );
}

#[test]
fn cached_help_avoids_starting_shadcn_again_and_refreshes_after_an_update() {
    let root = install();
    completed(root.path(), "shadcn info --");
    let calls = fs::read_to_string(root.path().join("calls")).unwrap();
    completed(root.path(), "shadcn info --");
    assert_eq!(
        fs::read_to_string(root.path().join("calls")).unwrap(),
        calls
    );
    let help = root.path().join("help/info.txt");
    let updated = format!(
        "{}\n  --future  a new option\n",
        fs::read_to_string(&help).unwrap()
    );
    fs::write(help, updated).unwrap();
    let binary = root.path().join("bin/shadcn");
    let updated = format!("{}\n# upgraded\n", fs::read_to_string(&binary).unwrap());
    fs::write(binary, updated).unwrap();
    assert!(
        completed(root.path(), "shadcn info --")
            .values()
            .contains(&"--future")
    );
}

#[test]
fn missing_or_broken_shadcn_returns_no_candidates() {
    let root = tree(&["bin/"]);
    assert!(completed(root.path(), "shadcn ").values().is_empty());
    executable(&root.path().join("bin/shadcn"), "#!/bin/sh\nexit 1\n");
    assert!(completed(root.path(), "shadcn ").values().is_empty());
}
