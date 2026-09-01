use std::ffi::OsString;

use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Resolution {
    #[default]
    Skip,
    Repo,
    Live,
}

#[derive(Clone, Debug, Parser)]
#[command(
    name = "dotfile sync",
    version,
    about = "Reconcile the repository, generated metadata, and this workstation"
)]
pub struct SyncCli {
    #[arg(
        value_name = "PROFILE",
        help = "Profile to reconcile; the saved profile by default"
    )]
    pub profile: Option<String>,

    #[arg(
        short = 'n',
        long = "dry-run",
        help = "Plan without changing files or contacting the peer"
    )]
    pub dry_run: bool,

    #[arg(
        long = "override",
        value_name = "GROUP=NAME",
        help = "Select a machine override with GROUP=NAME|none"
    )]
    pub overrides: Vec<String>,

    #[arg(
        long,
        conflicts_with = "resolve",
        help = "Resolve local edits from the repository; discard remote edits with --push"
    )]
    pub force: bool,

    #[arg(
        long,
        value_enum,
        default_value_t,
        help = "Choose how locally edited merged configs are settled"
    )]
    pub resolve: Resolution,

    #[arg(short = 'p', long, help = "Push commits, then pull and sync the peer")]
    pub push: bool,

    #[arg(long, value_name = "HOST", help = "Select the peer; implies --push")]
    pub to: Option<String>,

    #[arg(
        short = 'v',
        long,
        help = "Show every link, merge, generated file, and remote action"
    )]
    pub verbose: bool,
}

impl SyncCli {
    pub fn parse_tail(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, clap::Error> {
        let values = std::iter::once(OsString::from("dotfile sync")).chain(arguments);
        Self::try_parse_from(values)
    }
}
