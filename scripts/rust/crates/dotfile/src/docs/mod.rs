mod benchmarks;
mod cli;
pub mod keybinds;
mod markdown;
pub(crate) mod packages;
pub mod plan;
mod readme;
pub mod reference;

use crate::context::Context;
use crate::event::{Action, Event, EventSink, Phase};
pub use cli::{Args, Target, run};

pub fn synchronize(
    context: &Context,
    dry_run: bool,
    events: &dyn EventSink,
) -> Result<usize, String> {
    if !dry_run {
        crate::fs::transaction::recover(context)?;
    }
    events.emit(Event::PhaseStarted {
        phase: Phase::Artifacts,
        total: None,
    });
    let (plan, missing) = prepare(
        context,
        &[Target::Cli, Target::Keybinds, Target::Packages],
        true,
    )?;
    for program in missing {
        events.emit(Event::Warning {
            message: format!("{program}: command metadata unavailable; documentation retained"),
            hint: None,
        });
    }
    let paths = plan.paths();
    if !dry_run {
        plan.apply(&context.root)?;
    }
    for path in &paths {
        events.emit(Event::Item {
            action: Action::Generate,
            path: path.clone(),
            detail: if dry_run { "would update" } else { "updated" }.into(),
            changed: true,
        });
    }
    Ok(paths.len())
}

fn prepare(
    context: &Context,
    targets: &[Target],
    inventory: bool,
) -> Result<(plan::Plan, Vec<String>), String> {
    let mut outputs = Vec::new();
    let mut missing = Vec::new();
    for target in targets {
        match target {
            Target::Cli => {
                let (pages, unavailable) = reference::outputs(context)?;
                outputs.extend(pages);
                missing.extend(unavailable);
            }
            Target::Keybinds => outputs.extend(keybinds::outputs(&context.root)?),
            Target::Packages => outputs.extend(packages::outputs(context, inventory)?),
            Target::Readme => outputs.push(readme::output_file(context)?),
            Target::Benchmarks => outputs.push(benchmarks::output(context)?),
        }
    }
    Ok((plan::Plan::new(&context.root, outputs)?, missing))
}
