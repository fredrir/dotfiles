use clap::{Args as ClapArgs, Subcommand};
use std::path::PathBuf;

#[derive(Debug, ClapArgs)]
#[command(about = "Keep private material out of the repository")]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scan leaked tokens, private values, and encryption invariants.
    Scan {
        paths: Vec<PathBuf>,
        #[arg(long, conflicts_with = "commits")]
        staged: bool,
        #[arg(long)]
        commits: Option<String>,
        #[arg(long)]
        no_canaries: bool,
        #[arg(long)]
        all: bool,
    },
    /// Create this machine's age identity and print its public key.
    Init,
    /// Add a recipient; without a key, enroll this machine.
    Enroll {
        label: String,
        key: Option<String>,
        #[arg(long)]
        using: Option<PathBuf>,
    },
    /// Remove a recipient and rotate every encrypted data key.
    Revoke {
        label: String,
        #[arg(long)]
        using: Option<PathBuf>,
    },
    /// Replace a recipient key, preserving its label.
    Roll {
        label: String,
        key: Option<String>,
        #[arg(long)]
        using: Option<PathBuf>,
    },
    /// Give every encrypted file a new data key.
    Rekey {
        #[arg(long)]
        using: Option<PathBuf>,
    },
    /// List enrolled recipients.
    Keys,
    /// Regenerate .sops.yaml from config/keys.dotfile.
    Sync {
        #[arg(long)]
        rewrap: bool,
        #[arg(long)]
        using: Option<PathBuf>,
    },
    /// Check identities, recipients, hooks, and encrypted files.
    Doctor {
        #[arg(long)]
        all: bool,
    },
    /// Encrypt a live file into the repository and keep it in place.
    Add(AddArgs),
    /// Edit a tracked secret with SOPS, then apply it.
    Edit { path: PathBuf },
    /// Decrypt tracked secrets to their destinations.
    Apply {
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
    },
    /// Show each tracked secret's local state.
    Status,
    /// List variable names without revealing their values.
    Vars {
        #[arg(long)]
        unused: bool,
    },
    /// Remove materialized secrets while preserving local edits.
    Clean {
        #[arg(short = 'n', long)]
        dry_run: bool,
    },
    #[command(name = "__redact", hide = true)]
    Redact,
}

#[derive(Debug, ClapArgs)]
pub struct AddArgs {
    pub path: PathBuf,
    #[arg(long, default_value = "")]
    pub pkg: String,
    #[arg(long)]
    pub shared: bool,
    #[arg(long)]
    pub linux: bool,
    #[arg(long)]
    pub arch: bool,
    #[arg(long)]
    pub ubuntu: bool,
    #[arg(long)]
    pub kde: bool,
    #[arg(long)]
    pub hyprland: bool,
    #[arg(long)]
    pub macos: bool,
    #[arg(long, conflicts_with = "no_marker")]
    pub marker: bool,
    #[arg(long)]
    pub no_marker: bool,
}

impl Command {
    pub fn mutates(&self) -> bool {
        !matches!(
            self,
            Self::Scan { .. }
                | Self::Keys
                | Self::Doctor { .. }
                | Self::Status
                | Self::Vars { .. }
                | Self::Redact
                | Self::Apply { dry_run: true, .. }
                | Self::Clean { dry_run: true }
        )
    }
}
