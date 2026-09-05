use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use workstation::{Completable, Completions};

pub use workstation::ColorMode;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, ValueEnum)]
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

#[derive(Parser)]
#[command(
    name = "agent-hop",
    version,
    about = "Move a Codex or Claude Code CLI session between workstations",
    after_long_help = "
  agent-hop                       Browse, preview, and move your sessions
  agent-hop codex                 Move this directory's latest Codex session
  agent-hop claude                Move this directory's latest Claude Code session
  agent-hop codex SESSION_ID      Move one specific session
  agent-hop --list                List local and remote sessions without a TUI
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
        long,
        conflicts_with_all = ["agent", "session_id", "dry_run", "no_connect"],
        help = "List available local and remote sessions without opening the picker"
    )]
    pub list: bool,

    #[arg(
        value_enum,
        value_name = "AGENT",
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

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

#[derive(Subcommand)]
pub enum Command {
    #[command(name = "__machine", hide = true)]
    Machine(Machine),
}

#[derive(Args)]
pub struct Machine {
    #[command(subcommand)]
    pub request: MachineRequest,
}

#[derive(Subcommand)]
pub enum MachineRequest {
    Catalog {
        #[arg(long, default_value_t = crate::remote::MACHINE_PROTOCOL_VERSION)]
        protocol: u64,
        #[arg(long)]
        workspace: Option<PathBuf>,
        #[arg(long, default_value_t = 1_000)]
        limit: usize,
    },
    Preview {
        #[arg(long, default_value_t = crate::remote::MACHINE_PROTOCOL_VERSION)]
        protocol: u64,
        #[arg(long)]
        agent: Agent,
        #[arg(long)]
        session: String,
        #[arg(long, default_value_t = 12_000)]
        max_chars: usize,
    },
    Export {
        #[arg(long, default_value_t = crate::remote::MACHINE_PROTOCOL_VERSION)]
        protocol: u64,
        #[arg(long)]
        agent: Agent,
        #[arg(long)]
        session: String,
        #[arg(long)]
        sha256: String,
        #[arg(long)]
        bytes: u64,
    },
    Lineage {
        #[arg(long, default_value_t = crate::remote::MACHINE_PROTOCOL_VERSION)]
        protocol: u64,
        #[arg(long)]
        agent: Agent,
        #[arg(long)]
        session: String,
    },
    Import {
        #[arg(long, default_value_t = crate::remote::MACHINE_PROTOCOL_VERSION)]
        protocol: u64,
        #[arg(long)]
        agent: Agent,
        #[arg(long)]
        session: String,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        sha256: String,
        #[arg(long)]
        bytes: u64,
    },
    RecordManifest {
        #[arg(long, default_value_t = crate::remote::MACHINE_PROTOCOL_VERSION)]
        protocol: u64,
    },
    ExportCompanion {
        #[arg(long, default_value_t = crate::remote::MACHINE_PROTOCOL_VERSION)]
        protocol: u64,
        #[arg(long)]
        agent: Agent,
        #[arg(long)]
        session: String,
        #[arg(long)]
        workspace: PathBuf,
    },
}

#[cfg(test)]
#[path = "../tests/unit/cli_tests.rs"]
mod tests;
