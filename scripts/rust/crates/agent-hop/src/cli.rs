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
    args_conflicts_with_subcommands = true,
    about = "Move a Codex or Claude Code CLI session between workstations",
    after_long_help = "
  agent-hop                       Browse, preview, and move your sessions
  agent-hop codex                 Move this directory's latest Codex session
  agent-hop claude                Move this directory's latest Claude Code session
  agent-hop codex SESSION_ID      Move one specific session
  agent-hop --list                List local and remote sessions without a TUI
  agent-hop --dry-run claude      Show what would be copied and started
  agent-hop --no-connect codex    Copy the session without opening the peer

Managed execution (requires tmux; transfers at a safe turn boundary):
  agent-hop run codex             Start a managed native Codex UI
  agent-hop run claude            Start a managed native Claude UI
  agent-hop move --to macie       Queue execution transfer for this managed pane
  agent-hop status                Inspect this pane's durable ownership receipt
  agent-hop follow                Attach the verified destination owner
  agent-hop cancel                Cancel a queued move
  agent-hop recover --run RUN_ID  Recover only after proving no other owner

The original history commands copy conversation history; they do not stop an
already running agent. Managed moves transfer resumable agent execution and a
private Git workspace snapshot, not arbitrary process memory."
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
    /// Start a native agent UI with coordinated execution handoff.
    Run {
        #[arg(value_enum)]
        agent: Agent,
        #[arg(long)]
        resume: Option<String>,
    },
    /// Move this managed agent pane after its current turn finishes.
    Move {
        #[arg(long)]
        pane: Option<String>,
        #[arg(long, value_enum)]
        to: Option<hostkit::Host>,
    },
    /// Inspect a managed pane or a durable handoff receipt.
    Status {
        #[arg(long, conflicts_with = "run")]
        pane: Option<String>,
        #[arg(long)]
        run: Option<String>,
    },
    /// Cancel a queued move before ownership transfers.
    Cancel {
        #[arg(long)]
        pane: Option<String>,
    },
    /// Attach to the destination that owns this pane's moved agent.
    Follow {
        #[arg(long)]
        pane: Option<String>,
    },
    /// Resolve a failed transfer before resuming preserved source history.
    Recover {
        #[arg(long, conflicts_with = "run")]
        pane: Option<String>,
        #[arg(long)]
        run: Option<String>,
    },
    #[command(name = "__handoff", hide = true)]
    Handoff {
        #[arg(value_parser = ["preflight", "receive", "serve", "activate", "abort", "status", "hook"])]
        operation: String,
        #[arg(long)]
        id: String,
    },
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
