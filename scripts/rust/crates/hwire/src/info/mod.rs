mod collect;
mod detect;
mod model;
mod render;
mod ssh_info;
mod tui;

use std::io::{self, IsTerminal, Write};
use std::thread;
use std::time::Duration;

use workstation::ColorMode;

#[derive(Clone, Debug)]
pub struct Options {
    pub verbose: bool,
    pub json: bool,
    pub watch: bool,
    pub interval: Duration,
    pub notify: bool,
    pub color: ColorMode,
    pub targets: Vec<String>,
    pub diagnostics: bool,
}

pub fn run(options: Options) -> Result<(), String> {
    if options.verbose && !options.json && tui::capable() {
        return tui::run(options);
    }
    plain(options)
}

fn plain(options: Options) -> Result<(), String> {
    let terminal = io::stdout().is_terminal();
    let iterations = std::env::var("HWIRE_WATCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let mut previous = None;
    let mut previous_preferred: Option<Option<hostkit::Route>> = None;
    let mut completed = 0usize;
    loop {
        let snapshot = collect::snapshot(&options)?;
        let fingerprint = snapshot.fingerprint();
        let changed = previous
            .as_ref()
            .is_none_or(|previous| previous != &fingerprint);
        let primary_route = snapshot.primary_route();
        let route_changed = previous_preferred.is_some_and(|previous| previous != primary_route);
        if changed {
            if route_changed && options.notify && terminal {
                print!("\x07");
            }
            if options.json {
                println!("{}", render::json_document(&snapshot));
            } else if options.verbose {
                if previous.is_some() {
                    println!();
                }
                println!("{}", render::verbose(&snapshot, options.color, terminal));
            } else {
                println!("{}", render::compact(&snapshot, options.color, terminal));
            }
            io::stdout()
                .flush()
                .map_err(|error| format!("stdout: {error}"))?;
        }
        let failure = snapshot.failure();
        previous_preferred = Some(primary_route);
        previous = Some(fingerprint);
        completed += 1;
        if !options.watch || iterations.is_some_and(|limit| completed >= limit) {
            return match failure {
                Some(error) => Err(error),
                None => Ok(()),
            };
        }
        thread::sleep(options.interval);
    }
}
