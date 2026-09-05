use super::*;
use clap::CommandFactory;
use workstation::Style;

#[test]
fn the_parser_is_well_formed() {
    Cli::command().debug_assert();
}

#[test]
fn legacy_flags_cannot_silently_modify_managed_execution() {
    for flag in ["--dry-run", "--no-connect", "--list"] {
        assert!(Cli::try_parse_from(["agent-hop", flag, "run", "codex"]).is_err());
    }
    assert!(Cli::try_parse_from(["agent-hop", "codex", "run", "codex"]).is_err());
    assert!(Cli::try_parse_from(["agent-hop", "run", "codex"]).is_ok());
}

#[test]
fn a_bare_invocation_selects_the_interactive_picker() {
    let cli = Cli::try_parse_from(["agent-hop"]).unwrap();
    assert!(cli.agent.is_none());
    assert!(!cli.list);
    assert!(cli.command.is_none());
}

#[test]
fn completion_and_command_dump_runs_need_no_agent() {
    let completions = Cli::try_parse_from(["agent-hop", "--completions", "zsh"]).unwrap();
    assert!(completions.agent.is_none());
    assert!(completions.completions.shell.is_some());

    let dump = Cli::try_parse_from(["agent-hop", "--command-dump"]).unwrap();
    assert!(dump.agent.is_none());
    assert!(dump.completions.dump);
}

#[test]
fn every_public_setting_reaches_the_request() {
    let cli = Cli::try_parse_from([
        "agent-hop",
        "-n",
        "--no-connect",
        "--color",
        "always",
        "codex",
        "01999999-1111-7222-8333-444444444444",
    ])
    .unwrap();
    assert!(cli.dry_run);
    assert!(cli.no_connect);
    assert_eq!(cli.color, ColorMode::Always);
    assert_eq!(cli.agent, Some(Agent::Codex));
    assert_eq!(
        cli.session_id.as_deref(),
        Some("01999999-1111-7222-8333-444444444444")
    );
}

#[test]
fn either_agent_can_select_the_latest_session() {
    for (argument, expected) in [("codex", Agent::Codex), ("claude", Agent::Claude)] {
        let cli = Cli::try_parse_from(["agent-hop", argument]).unwrap();
        assert_eq!(cli.agent, Some(expected));
        assert!(cli.session_id.is_none());
        assert_eq!(cli.color, ColorMode::Auto);
        assert!(!cli.list);
    }
}

#[test]
fn names_are_the_spellings_the_agent_programs_use() {
    assert_eq!(Agent::Codex.name(), "codex");
    assert_eq!(Agent::Claude.name(), "claude");
}

#[test]
fn invalid_agent_and_color_values_are_usage_errors() {
    assert!(Cli::try_parse_from(["agent-hop", "other"]).is_err());
    assert!(Cli::try_parse_from(["agent-hop", "--color", "sometimes", "codex"]).is_err());
}

#[test]
fn completion_output_is_exclusive() {
    assert!(Cli::try_parse_from(["agent-hop", "--completions", "zsh", "codex"]).is_err());
}

#[test]
fn plain_listing_is_an_explicit_noninteractive_mode() {
    let cli = Cli::try_parse_from(["agent-hop", "--list"]).unwrap();
    assert!(cli.list);
    assert!(cli.agent.is_none());
    assert!(Cli::try_parse_from(["agent-hop", "--list", "codex"]).is_err());
}

#[test]
fn hidden_machine_requests_are_structured_subcommands() {
    let cli = Cli::try_parse_from([
        "agent-hop",
        "__machine",
        "preview",
        "--agent",
        "codex",
        "--session",
        "safe-id",
        "--max-chars",
        "20",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Command::Machine(Machine {
            request: MachineRequest::Preview {
                agent: Agent::Codex,
                max_chars: 20,
                ..
            }
        }))
    ));

    let cli = Cli::try_parse_from([
        "agent-hop",
        "__machine",
        "export-companion",
        "--agent",
        "claude",
        "--session",
        "safe-id",
        "--workspace",
        "/home/fred/project",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Command::Machine(Machine {
            request: MachineRequest::ExportCompanion {
                agent: Agent::Claude,
                ..
            }
        }))
    ));
}

#[test]
fn explicit_color_modes_override_terminal_detection() {
    assert!(
        Style::for_mode(ColorMode::Always, false)
            .bold("agent-hop")
            .contains("\x1b[")
    );
    assert_eq!(
        Style::for_mode(ColorMode::Never, true).bold("agent-hop"),
        "agent-hop"
    );
}
