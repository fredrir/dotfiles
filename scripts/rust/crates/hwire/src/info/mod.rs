mod collect;
mod detect;
mod model;
mod render;
mod ssh_info;
mod tui;

use std::io::{self, IsTerminal, Write};
use std::thread;
use std::time::Duration;

use clap::ValueEnum;

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
            ColorMode::Auto => {
                terminal
                    && std::env::var_os("NO_COLOR").is_none()
                    && std::env::var("CLICOLOR").ok().as_deref() != Some("0")
                    && std::env::var("TERM")
                        .ok()
                        .is_none_or(|term| !term.eq_ignore_ascii_case("dumb"))
            }
        }
    }
}

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

pub fn measurement_color(mode: ColorMode) -> bool {
    mode.enabled(io::stdout().is_terminal())
}

#[cfg(test)]
#[path = "../../tests/unit/info/mod_tests.rs"]
mod tests;
