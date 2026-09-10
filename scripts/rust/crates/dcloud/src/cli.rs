use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use workstation::{Completable, Completions};

#[derive(Parser)]
#[command(
    name = "dcloud",
    version,
    about = "Back up, transfer, browse and synchronize configured storage"
)]
pub struct Cli {
    #[arg(long, global = true, help = "Configuration file")]
    pub config: Option<PathBuf>,
    #[arg(long, global = true, help = "Print structured JSON")]
    pub json: bool,
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
    #[command(about = "Create configuration and local recovery credentials")]
    Init {
        #[arg(long)]
        host: String,
        #[arg(long)]
        secrets_dir: Option<PathBuf>,
    },
    #[command(about = "Import a SOPS-encrypted Google client and authorize Drive")]
    AuthDrive {
        #[arg(long)]
        client: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with = "client",
            help = "Encrypt downloaded client JSON from outside the repository"
        )]
        import_client: Option<PathBuf>,
        #[arg(long, default_value = "dcloud-drive")]
        remote: String,
        #[arg(long)]
        authorize: bool,
    },
    #[command(about = "Validate or print example configuration")]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(about = "Check dependencies, credentials and destinations")]
    Doctor {
        #[arg(long)]
        remote: bool,
    },
    #[command(about = "Create and replicate an incremental snapshot")]
    Backup {
        job: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
    },
    #[command(about = "Preview source selection, routing and backup changes")]
    Plan { job: String },
    #[command(about = "Retry unfinished replicas of the original snapshot")]
    Retry { run: Option<String> },
    #[command(about = "Run due jobs on their source computer")]
    RunDue {
        #[arg(long)]
        expect_host: Option<String>,
    },
    #[command(about = "Dispatch due work to configured source computers")]
    Dispatch {
        #[arg(value_delimiter = ',')]
        hosts: Vec<String>,
    },
    #[command(about = "Create a compressed portable archive and upload it")]
    Upload {
        path: PathBuf,
        #[arg(long, value_delimiter = ',', required = true)]
        to: Vec<String>,
        #[arg(long, default_value = "uploads")]
        category: String,
        #[arg(long = "label")]
        labels: Vec<String>,
        #[arg(long)]
        expires_days: Option<u32>,
        #[arg(
            long,
            help = "Quarantine unchanged source after full remote verification"
        )]
        move_source: bool,
    },
    #[command(about = "Download, decrypt and restore a portable archive")]
    Download {
        id: String,
        #[arg(long)]
        from: String,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        to: PathBuf,
        #[arg(long = "path")]
        paths: Vec<PathBuf>,
    },
    #[command(
        visible_alias = "ls",
        about = "Browse snapshots, archives and file trees"
    )]
    Browse {
        #[command(flatten)]
        filter: Filter,
        #[arg(long)]
        offline: bool,
        #[arg(long)]
        tui: bool,
        #[arg(long, requires = "job", requires = "from")]
        snapshot: Option<String>,
    },
    #[command(about = "Restore snapshot files into a new directory")]
    Restore {
        #[command(flatten)]
        repository: Repository,
        snapshot: String,
        #[arg(long)]
        to: PathBuf,
        #[arg(long = "path")]
        paths: Vec<PathBuf>,
    },
    #[command(about = "Verify repository structure or stored data")]
    Verify {
        #[command(flatten)]
        repository: Repository,
        #[arg(long)]
        full: bool,
        #[arg(long, conflicts_with = "full")]
        subset: Option<String>,
    },
    #[command(about = "Restore a recovery point and verify its content")]
    RestoreTest {
        #[command(flatten)]
        repository: Repository,
        #[arg(long)]
        snapshot: Option<String>,
    },
    #[command(about = "Preview or apply retention to verified recovery points")]
    Retention {
        #[command(flatten)]
        repository: Repository,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        prune: bool,
    },
    #[command(about = "Protect a snapshot from automatic retention")]
    Pin {
        #[command(flatten)]
        repository: Repository,
        snapshot: String,
        #[arg(long)]
        remove: bool,
    },
    #[command(about = "Update snapshot categories and labels")]
    Label {
        #[command(flatten)]
        repository: Repository,
        snapshot: String,
        #[arg(long = "add")]
        add: Vec<String>,
        #[arg(long = "remove")]
        remove: Vec<String>,
        #[arg(long)]
        category: Option<String>,
    },
    #[command(about = "Update a portable archive's category and labels")]
    LabelUpload {
        id: String,
        #[arg(long)]
        from: String,
        #[arg(long)]
        host: Option<String>,
        #[arg(long = "add")]
        add: Vec<String>,
        #[arg(long = "remove")]
        remove: Vec<String>,
        #[arg(long)]
        category: Option<String>,
    },
    #[command(about = "Compare two snapshots")]
    Diff {
        #[command(flatten)]
        repository: Repository,
        older: String,
        newer: String,
    },
    #[command(about = "Show storage use and a capacity projection")]
    Stats {
        #[command(flatten)]
        repository: Repository,
        #[arg(long)]
        forecast: bool,
    },
    #[command(about = "Repair a replica from a verified repository")]
    Repair {
        #[command(flatten)]
        repository: Repository,
        snapshot: String,
        #[arg(long)]
        to: String,
    },
    #[command(about = "Rebuild the local catalog from remote metadata")]
    Catalog {
        #[arg(long)]
        from: Option<String>,
    },
    #[command(about = "Preview or run bidirectional sync; preserve conflicts")]
    Sync {
        pair: String,
        #[arg(long)]
        init: bool,
        #[arg(long)]
        apply: bool,
    },
    #[command(about = "Show recovery points, pending copies and overdue jobs")]
    Status {
        #[arg(long)]
        overdue: bool,
    },
    #[command(about = "Render or install a local scheduler")]
    Schedule {
        #[arg(long)]
        install: bool,
        #[arg(long)]
        dispatch: bool,
    },
    #[command(about = "Preview or purge expired unchanged quarantine entries")]
    Cleanup {
        #[arg(long)]
        apply: bool,
    },
    #[command(about = "Export recovery credentials into a new private directory")]
    RecoveryExport {
        #[arg(long)]
        to: PathBuf,
    },
}

#[derive(Args)]
pub struct Repository {
    #[arg(long)]
    pub job: String,
    #[arg(long, help = "Destination name, or spool")]
    pub from: String,
    #[arg(long)]
    pub host: Option<String>,
}

#[derive(Args, Default)]
pub struct Filter {
    #[arg(long)]
    pub host: Option<String>,
    #[arg(long)]
    pub job: Option<String>,
    #[arg(long)]
    pub from: Option<String>,
    #[arg(long)]
    pub category: Option<String>,
    #[arg(long)]
    pub label: Option<String>,
    #[arg(long)]
    pub search: Option<String>,
    #[arg(long)]
    pub saved: Option<String>,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    Check,
    Example,
}
