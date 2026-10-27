use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use testkit::{executable, tree};

const SLUG: &str = "fredrik-hansteens-projects";

const ROOT_HELP: &str = r#"
  ▲ vercel [options] <command | path>

  Commands:

    Basic

      build            Build the project
      deploy           Deploy your project to Vercel (default)
      dev              Starts the `vercel dev` server
      env              Interact with Environment Variables for a Project
      help             Show help for a command
      link             Link a local directory to a Vercel project
      pull             Pull Environment Variables and project settings into .vercel
      switch           Switch to a different team

    Advanced

      alias            Interact with deployment aliases
      domains          Manage domains
      list             List deployments
      project          Manage your Vercel projects
      teams            Manage Teams under your Vercel account

  Global Options:

    -h, --help                     Output usage information
    --cwd                          Current working directory
    -A FILE, --local-config=FILE   Path to the local `vercel.json` file
    -Q DIR, --global-config=DIR    Path to the global `.vercel` directory
    -S, --scope                    Set a custom scope
    -t TOKEN, --token=TOKEN        Login token
"#;

fn install(fail_project: bool) -> testkit::TempDir {
    let root = tree(&["bin/"]);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vercel");
    let help = root.path().join("root.txt");
    fs::write(&help, ROOT_HELP).unwrap();
    let mut script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}/calls'\ncase \"$*\" in\n  '--help') /bin/cat '{}' ;;\n",
        root.path().display(),
        help.display()
    );
    let mut mapping = vec![
        ("build --help", "build.txt"),
        ("deploy --help", "deploy.txt"),
        ("dev --help", "dev.txt"),
        ("env --help", "env.txt"),
        ("alias --help", "alias.txt"),
        ("domains --help", "domains.txt"),
        ("teams --help", "teams.txt"),
        ("switch --help", "switch.txt"),
        ("link --help", "link.txt"),
        ("list --help", "list.txt"),
        ("pull --help", "pull.txt"),
        ("alias list --help", "alias-list.txt"),
        ("alias remove --help", "alias-remove.txt"),
        ("alias set --help", "alias-set.txt"),
        ("domains add --help", "domains-add.txt"),
        ("domains inspect --help", "domains-inspect.txt"),
        ("domains list --help", "domains-list.txt"),
        ("domains move --help", "domains-move.txt"),
        ("env add --help", "env-add.txt"),
        ("env list --help", "env-list.txt"),
        ("env pull --help", "env-pull.txt"),
        ("env remove --help", "env-remove.txt"),
        ("env run --help", "env-run.txt"),
        ("env update --help", "env-update.txt"),
        ("link add --help", "link-add.txt"),
        ("project inspect --help", "project-inspect.txt"),
        ("project list --help", "project-list.txt"),
        ("project rename --help", "project-rename.txt"),
        ("teams list --help", "teams-list.txt"),
        ("teams switch --help", "teams-switch.txt"),
    ];
    if !fail_project {
        mapping.push(("project --help", "project.txt"));
    }
    for (args, file) in mapping {
        script.push_str(&format!(
            "  '{args}') /bin/cat '{}' ;;\n",
            fixtures.join(file).display()
        ));
    }
    script.push_str(&format!(
        "  'teams ls --json --limit 100') /bin/cat '{}' ;;\n  *) exit 1 ;;\nesac\n",
        fixtures.join("json/teams.json").display()
    ));
    executable(&root.path().join("bin/vercel"), &script);
    let auth = root.path().join(".local/share/com.vercel.cli");
    fs::create_dir_all(&auth).unwrap();
    fs::write(auth.join("auth.json"), "{}").unwrap();
    fs::write(auth.join("config.json"), "{}").unwrap();
    root
}

fn context(root: &Path, offline: bool) -> Context {
    let path = root.join("bin").to_string_lossy().to_string();
    Context::testing(
        root,
        root,
        &[
            ("PATH", &path),
            ("ZCOMP_OFFLINE", if offline { "1" } else { "0" }),
        ],
    )
}

fn completed(root: &Path, input: &str) -> Reply {
    let ctx = context(root, false);
    let words: Vec<String> = input.split(' ').map(String::from).collect();
    let current = words.len();
    crate::complete(&ctx, "vercel", &Line::new(words, current, None))
}

fn calls(root: &Path) -> String {
    fs::read_to_string(root.join("calls")).unwrap_or_default()
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vercel")
        .join(name)
}

#[test]
fn root_lists_commands_aliases_and_directories() {
    let root = install(false);
    let reply = completed(root.path(), "vercel ");
    for expected in [
        "deploy", "project", "projects", "teams", "switch", "alias", "aliases", "domains",
    ] {
        assert!(reply.values().contains(&expected), "{expected}: {reply:?}");
    }
    assert!(reply.plain().ends_with("dirs\n"), "{reply:?}");
}

#[test]
fn nested_commands_load_below_their_parent() {
    let root = install(false);
    for input in ["vercel project ", "vercel projects "] {
        let reply = completed(root.path(), input);
        for expected in ["list", "inspect", "rename", "create"] {
            assert!(reply.values().contains(&expected), "{input}: {reply:?}");
        }
    }
    assert_eq!(
        completed(root.path(), "vercel alias ").values(),
        ["list", "remove", "set"]
    );
}

#[test]
fn flags_come_from_the_root_and_the_selected_command() {
    let root = install(false);
    let project = completed(root.path(), "vercel project list --");
    for expected in [
        "--filter=",
        "--json",
        "--scope=",
        "--cwd=",
        "--local-config=",
    ] {
        assert!(
            project.values().contains(&expected),
            "{expected}: {project:?}"
        );
    }
    let deploy = completed(root.path(), "vercel deploy --");
    for expected in ["--prod", "--prebuilt", "--archive=", "--env=", "--scope="] {
        assert!(
            deploy.values().contains(&expected),
            "{expected}: {deploy:?}"
        );
    }
}

#[test]
fn static_choices_apply_to_the_commands_that_have_them() {
    let root = install(false);
    assert_eq!(
        completed(root.path(), "vercel list --status ").values(),
        [
            "BUILDING",
            "ERROR",
            "INITIALIZING",
            "QUEUED",
            "READY",
            "CANCELED",
            "BLOCKED",
        ]
    );
    assert_eq!(
        completed(root.path(), "vercel deploy --environment ").values(),
        ["production", "preview"]
    );
    assert_eq!(
        completed(root.path(), "vercel env add --environment ").values(),
        ["production", "preview", "development"]
    );
    assert_eq!(
        completed(root.path(), "vercel env add --type ").values(),
        ["config", "secret"]
    );
    assert_eq!(
        completed(root.path(), "vercel deploy --archive ").values(),
        ["tgz"]
    );
}

#[test]
fn comma_separated_choices_keep_the_prefix_and_drop_chosen_values() {
    let root = install(false);
    let status = completed(root.path(), "vercel list --status=READY,");
    assert!(
        status.plain().starts_with("skip\t--status=READY,\n"),
        "{status:?}"
    );
    assert!(!status.values().contains(&"READY"));
    assert!(status.values().contains(&"BUILDING"));

    let environment = completed(root.path(), "vercel env add --environment=production,");
    assert!(
        environment
            .plain()
            .starts_with("skip\t--environment=production,\n"),
        "{environment:?}"
    );
    assert!(!environment.values().contains(&"production"));
    assert_eq!(environment.values(), ["preview", "development"]);
}

#[test]
fn inline_values_complete_without_repeating_the_flag() {
    let root = install(false);
    let scope = completed(root.path(), "vercel --scope=");
    assert!(scope.plain().starts_with("skip\t--scope=\n"), "{scope:?}");
    assert!(scope.values().contains(&SLUG));
    assert_eq!(
        completed(root.path(), "vercel deploy --environment=").values(),
        ["production", "preview"]
    );
    assert_eq!(
        completed(root.path(), "vercel --cwd=").plain(),
        "skip\t--cwd=\ndirs\n"
    );
}

#[test]
fn a_separator_stops_flags_and_env_run_takes_no_completion() {
    let root = install(false);
    assert_eq!(completed(root.path(), "vercel env run -- ").plain(), "");
    assert!(
        completed(root.path(), "vercel deploy -- ")
            .plain()
            .contains("dirs\n")
    );
    let deploy = completed(root.path(), "vercel deploy -- --prod");
    assert!(!deploy.values().contains(&"--prod"), "{deploy:?}");
}

#[test]
fn help_mode_lists_commands_and_never_data() {
    let root = install(false);
    assert!(
        completed(root.path(), "vercel help ")
            .values()
            .contains(&"deploy")
    );
    let nested = completed(root.path(), "vercel help project ");
    for expected in ["list", "inspect", "rename"] {
        assert!(
            nested.values().contains(&expected),
            "{expected}: {nested:?}"
        );
    }
    assert!(
        completed(root.path(), "vercel help domains move ")
            .values()
            .is_empty()
    );
    completed(root.path(), "vercel help --scope ");
    assert!(
        !calls(root.path()).contains("teams ls"),
        "{}",
        calls(root.path())
    );
}

#[test]
fn the_default_deploy_command_supplies_flags_and_paths() {
    let root = install(false);
    let flags = completed(root.path(), "vercel ./dist --");
    for expected in ["--prod", "--prebuilt", "--archive="] {
        assert!(flags.values().contains(&expected), "{expected}: {flags:?}");
    }
    let path = completed(root.path(), "vercel deploy ");
    assert!(path.plain().contains("delegate\n"), "{path:?}");
    assert!(path.plain().ends_with("dirs\n"), "{path:?}");
    assert_eq!(
        completed(root.path(), "vercel --env ").plain(),
        "message\tKEY=VALUE\n"
    );
}

#[test]
fn teams_fill_scope_switch_and_domain_destination_values() {
    let root = install(false);
    for input in [
        "vercel --scope ",
        "vercel -S ",
        "vercel link --team ",
        "vercel teams switch ",
        "vercel switch ",
        "vercel domains move acme ",
    ] {
        let reply = completed(root.path(), input);
        assert!(reply.values().contains(&SLUG), "{input}: {reply:?}");
    }
}

#[test]
fn the_current_team_is_marked() {
    let mut reply = Reply::new();
    teams_group(
        vec![
            data::Team {
                slug: "acme".into(),
                name: "Acme".into(),
                current: true,
            },
            data::Team {
                slug: "other".into(),
                name: "Other".into(),
                current: false,
            },
        ],
        &mut reply,
    );
    let plain = reply.plain();
    assert!(
        plain.starts_with("group\tteams\tteam\tlines\tunsorted\n"),
        "{plain}"
    );
    assert!(
        plain
            .lines()
            .any(|line| line.starts_with("item\tacme\t") && line.ends_with("current")),
        "{plain}"
    );
    assert!(
        plain
            .lines()
            .any(|line| line.starts_with("item\tother\t") && !line.ends_with("current")),
        "{plain}"
    );
}

#[test]
fn root_switch_and_install_use_their_canonical_paths() {
    let root = spec::Spec::default();
    assert_eq!(
        effective_path(&["switch".into()], 0, "", &root),
        ["teams", "switch"]
    );
    assert_eq!(
        effective_path(&["install".into()], 0, "", &root),
        ["integration", "add"]
    );
    assert_eq!(
        effective_path(&["alias".into()], 1, "", &root),
        ["alias", "set"]
    );
    assert_eq!(
        effective_path(&["alias".into()], 0, "dep", &root),
        ["alias", "set"]
    );
}

#[test]
fn offline_teams_are_empty_without_running_the_cli() {
    let root = install(false);
    let ctx = context(root.path(), true);
    assert!(data::teams(&ctx).is_empty());
    assert!(!calls(root.path()).contains("teams ls"));
}

#[test]
fn teams_are_cached_between_completions() {
    let root = install(false);
    let ctx = context(root.path(), false);
    assert_eq!(data::teams(&ctx).len(), 1);
    assert_eq!(
        calls(root.path())
            .matches("teams ls --json --limit 100")
            .count(),
        1
    );
    assert_eq!(data::teams(&ctx).len(), 1);
    assert_eq!(
        calls(root.path())
            .matches("teams ls --json --limit 100")
            .count(),
        1
    );
}

#[test]
fn missing_auth_caches_an_empty_team_list() {
    let root = install(false);
    fs::remove_file(root.path().join(".local/share/com.vercel.cli/auth.json")).unwrap();
    let ctx = context(root.path(), false);
    assert!(data::teams(&ctx).is_empty());
    assert!(!calls(root.path()).contains("teams ls"));
    let stored: Option<Vec<data::Team>> = crate::cache::peek(&ctx, "vercel-teams");
    assert!(stored.is_some_and(|teams| teams.is_empty()));
}

#[test]
fn command_completion_does_not_start_a_data_subprocess() {
    let root = install(false);
    assert!(!completed(root.path(), "vercel ").values().is_empty());
    assert!(
        !calls(root.path()).contains("teams ls"),
        "{}",
        calls(root.path())
    );
}

#[test]
fn help_is_never_fetched_and_a_cached_spec_is_reused() {
    let root = install(false);
    completed(root.path(), "vercel deploy --");
    let first = calls(root.path());
    assert!(
        !first
            .lines()
            .any(|line| line == "help --help" || line.starts_with("help ")),
        "{first}"
    );
    completed(root.path(), "vercel deploy --");
    assert_eq!(calls(root.path()), first);
}

#[test]
fn a_failed_child_is_omitted_without_losing_the_rest() {
    let root = install(true);
    let spec = spec::load(&context(root.path(), false));
    assert!(
        !spec.children.contains_key("project"),
        "{:?}",
        spec.children
    );
    assert!(spec.children.contains_key("teams"), "{:?}", spec.children);
    assert!(spec.help.command("project").is_some());
    assert!(
        completed(root.path(), "vercel teams switch ")
            .values()
            .contains(&SLUG)
    );
}

#[test]
fn replacing_the_binary_invalidates_the_spec() {
    let root = install(false);
    completed(root.path(), "vercel deploy --");
    let first = calls(root.path());
    let binary = root.path().join("bin/vercel");
    let mut script = fs::read_to_string(&binary).unwrap();
    script.push_str("\n# upgraded\n");
    fs::write(&binary, script).unwrap();
    completed(root.path(), "vercel deploy --");
    assert_ne!(calls(root.path()), first);
}

#[test]
fn the_team_payload_ignores_fields_other_than_slug_name_and_current() {
    let payload = fs::read_to_string(fixture_path("json/teams.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert!(parsed["teams"][0].get("plan").is_some());
    let teams: Vec<data::Team> = serde_json::from_value(parsed["teams"].clone()).unwrap();
    assert_eq!(teams[0].slug, SLUG);
    assert_eq!(teams[0].name, "Fredrik Hansteen's projects");
}

#[test]
fn vc_dispatches_and_falls_back_when_vercel_is_missing() {
    let root = install(false);
    let complete_vc = || {
        let ctx = context(root.path(), false);
        let line = Line::new(vec!["vc".into(), String::new()], 2, None);
        crate::complete(&ctx, "vc", &line)
    };
    assert!(complete_vc().values().contains(&"deploy"));
    fs::rename(root.path().join("bin/vercel"), root.path().join("bin/vc")).unwrap();
    assert!(complete_vc().values().contains(&"deploy"));
}

#[test]
fn value_flags_that_are_not_data_driven_report_their_value() {
    let root = install(false);
    assert_eq!(
        completed(root.path(), "vercel deploy --project ").plain(),
        "message\tproject name\n"
    );
    assert_eq!(
        completed(root.path(), "vercel --token ").plain(),
        "message\tTOKEN for -t, --token\n"
    );
    assert_eq!(completed(root.path(), "vercel -A ").plain(), "files\n");
    assert_eq!(completed(root.path(), "vercel -Q ").plain(), "dirs\n");
    assert_eq!(completed(root.path(), "vercel --cwd ").plain(), "dirs\n");
}

#[test]
fn changing_auth_files_invalidates_the_team_cache() {
    let root = install(false);
    let ctx = context(root.path(), false);
    assert_eq!(data::teams(&ctx).len(), 1);
    let config = root.path().join(".local/share/com.vercel.cli/config.json");
    fs::write(config, "{\"currentTeam\":\"team_other\"}").unwrap();
    assert_eq!(data::teams(&ctx).len(), 1);
    assert_eq!(
        calls(root.path())
            .matches("teams ls --json --limit 100")
            .count(),
        2
    );
}

#[test]
fn path_positionals_use_the_shells_directory_or_file_completion() {
    let root = install(false);
    for input in ["vercel build ", "vercel dev ", "vercel pull "] {
        assert!(
            completed(root.path(), input).plain().ends_with("dirs\n"),
            "{input}"
        );
    }
    assert!(
        completed(root.path(), "vercel env pull ")
            .plain()
            .ends_with("files\n")
    );
}
