use std::time::Duration;

use crate::cli::SyncCli;
use crate::context::Context;
use crate::decision::Client;
use crate::event::{EventSink, Summary};

use super::config::Configuration;

pub fn reconcile(
    context: &Context,
    profile: &str,
    cli: &SyncCli,
    decisions: &Client,
    events: &dyn EventSink,
) -> Result<Summary, String> {
    crate::cancel::check()?;
    let configuration = Configuration::load(context, profile, &cli.overrides, events)?;
    let (merge_entries, merge_paths) = super::merge::discover(context, &configuration)?;
    crate::cancel::check()?;
    let links =
        super::links::synchronize(context, &configuration, &merge_paths, cli.dry_run, events)?;
    crate::cancel::check()?;
    let secrets =
        super::secrets::synchronize(context, &configuration, cli.dry_run, cli.force, events)?;
    crate::cancel::check()?;
    let merges = super::merge::synchronize(
        context,
        &merge_entries,
        cli.dry_run,
        cli.force,
        cli.resolve,
        decisions,
        events,
    )?;
    crate::cancel::check()?;
    let integrations =
        super::integrations::synchronize(context, &configuration, cli.dry_run, events)?;
    context.save_profile(profile, cli.dry_run)?;
    configuration.save_overrides(context, cli.dry_run)?;
    if secrets.blocked > 0 || merges.blocked > 0 {
        let mut problems = Vec::new();
        if secrets.blocked > 0 {
            problems.push(format!(
                "{} secret{}",
                secrets.blocked,
                if secrets.blocked == 1 { "" } else { "s" }
            ));
        }
        if merges.blocked > 0 {
            problems.push(format!(
                "{} merged file{}",
                merges.blocked,
                if merges.blocked == 1 { "" } else { "s" }
            ));
        }
        return Err(format!("{} need a decision", problems.join(" and ")));
    }
    super::links::save_index(context, &links.managed, cli.dry_run)?;
    Ok(Summary {
        profile: profile.to_string(),
        peer: None,
        remote_changed: None,
        checked: links.checked + secrets.checked + merges.checked + integrations.checked,
        changed: links.changed + secrets.changed + merges.changed + integrations.changed,
        links: links.links,
        merges: merges.merges,
        secrets: secrets.secrets,
        generated: integrations.generated,
        dry_run: cli.dry_run,
        elapsed: Duration::ZERO,
    })
}
