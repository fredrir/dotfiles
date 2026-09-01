use std::io::{self, IsTerminal};

use clap::{Parser, ValueEnum};
use workstation::{Completions, Style};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Agent {
    Codex,
    Claude,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Codex => "codex",
            Agent::Claude => "claude",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn enabled(self, terminal: bool) -> bool {
        match self {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => auto_enabled(
                terminal,
                std::env::var_os("NO_COLOR").is_some(),
                std::env::var("CLICOLOR").ok().as_deref(),
                std::env::var("TERM").ok().as_deref(),
            ),
        }
    }

    pub fn style(self) -> Style {
        self.style_for(io::stdout().is_terminal())
    }

    pub fn style_for(self, terminal: bool) -> Style {
        Style::for_stdout_with_color(self.enabled(terminal))
    }
}

fn auto_enabled(
    terminal: bool,
    no_color: bool,
    clicolor: Option<&str>,
    term: Option<&str>,
) -> bool {
    terminal
        && !no_color
        && clicolor != Some("0")
        && term.is_none_or(|value| !value.eq_ignore_ascii_case("dumb"))
}

#[derive(Parser)]
#[command(
    name = "agent-hop",
    version,
    about = "Move an agent session another other workstation",
    after_long_help = "
  agent-hop codex                 Move this directory's latest Codex session
  agent-hop claude                Move this directory's latest Claude Code session
  agent-hop codex SESSION_ID      Move one specific session
  agent-hop --dry-run claude      Show what would be copied and started
  agent-hop --no-connect codex    Copy the session without opening the peer"
)]
pub struct Cli {
    #[arg(
        short = 'n',
        long = "dry-run",
        help = "Show what would be copied and started without changing anything"
    )]
    pub dry_run: bool,

    #[arg(
        long = "no-connect",
        help = "Copy the session without opening it on the other workstation"
    )]
    pub no_connect: bool,

    #[arg(
        long = "color",
        value_name = "WHEN",
        default_value = "auto",
        help = "Control colored output"
    )]
    pub color: ColorMode,

    #[arg(
        value_enum,
        value_name = "AGENT",
        required_unless_present = "shell",
        help = "Select the coding agent whose session will move"
    )]
    pub agent: Option<Agent>,

    #[arg(
        value_name = "SESSION_ID",
        requires = "agent",
        help = "Move this session instead of the newest one for the working directory"
    )]
    pub session_id: Option<String>,

    #[command(flatten)]
    pub completions: Completions,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_parser_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn an_agent_is_required_for_a_regular_run() {
        assert!(Cli::try_parse_from(["agent-hop"]).is_err());
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
    fn explicit_color_modes_override_terminal_detection() {
        assert!(ColorMode::Always.enabled(false));
        assert!(!ColorMode::Never.enabled(true));
        assert!(
            ColorMode::Always
                .style_for(false)
                .bold("agent-hop")
                .contains("\x1b[")
        );
        assert_eq!(
            ColorMode::Never.style_for(true).bold("agent-hop"),
            "agent-hop"
        );
    }

    #[test]
    fn automatic_color_obeys_the_stream_and_environment_contract() {
        assert!(auto_enabled(true, false, None, None));
        assert!(auto_enabled(true, false, Some("1"), Some("xterm-256color")));
        assert!(!auto_enabled(false, false, None, None));
        assert!(!auto_enabled(true, true, None, None));
        assert!(!auto_enabled(true, false, Some("0"), None));
        assert!(!auto_enabled(true, false, None, Some("dumb")));
        assert!(!auto_enabled(true, false, None, Some("DUMB")));
    }
}
