use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use crate::listener::{self, Listener};

const POLL: Duration = Duration::from_millis(500);

pub fn run(watch: bool) -> Result<(), String> {
    if !watch {
        return emit(&mut io::stdout().lock(), &snapshot()?);
    }
    thread::spawn(|| {
        let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
        std::process::exit(0);
    });
    let mut last = None;
    loop {
        let current = snapshot()?;
        if last.as_ref() != Some(&current) {
            emit(&mut io::stdout().lock(), &current)?;
            last = Some(current);
        }
        thread::sleep(POLL);
    }
}

fn snapshot() -> Result<Vec<Listener>, String> {
    listener::scan().map(listener::exportable)
}

fn emit(out: &mut impl Write, listeners: &[Listener]) -> Result<(), String> {
    let line = serde_json::to_string(listeners).map_err(|error| error.to_string())?;
    writeln!(out, "{line}")
        .and_then(|()| out.flush())
        .map_err(|error| format!("stdout: {error}"))
}
