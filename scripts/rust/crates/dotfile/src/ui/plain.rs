use std::collections::HashSet;
use std::io::{self, Write};
use std::thread::JoinHandle;

use crossbeam_channel::Receiver;

use crate::decision::{Request, Server};
use crate::event::{Event, Summary};

pub fn run(
    receiver: Receiver<Event>,
    decisions: Server,
    worker: JoinHandle<Result<Summary, String>>,
    verbose: bool,
) -> Result<Summary, String> {
    run_with_initial(receiver, decisions, worker, verbose, std::iter::empty())
}

pub(crate) fn run_with_initial(
    receiver: Receiver<Event>,
    decisions: Server,
    worker: JoinHandle<Result<Summary, String>>,
    verbose: bool,
    initial: impl IntoIterator<Item = Event>,
) -> Result<Summary, String> {
    let mut seen_items = HashSet::new();
    let mut seen_warnings = HashSet::new();
    let mut failure = None;
    let mut output = PlainOutput {
        stdout: io::stdout(),
        stderr: io::stderr(),
    };
    let mut initial_error = None;
    for event in initial {
        if let Err(error) = render_event(
            event,
            verbose,
            &mut seen_items,
            &mut seen_warnings,
            &mut failure,
            &mut output,
        ) {
            initial_error = Some((error, None));
            break;
        }
    }
    let ui_error = initial_error.or_else(|| {
        let mut decisions_open = true;
        loop {
            let incoming = if decisions_open {
                crossbeam_channel::select! {
                    recv(receiver) -> event => PlainIncoming::Event(event),
                    recv(decisions.requests()) -> request => PlainIncoming::Decision(request),
                }
            } else {
                PlainIncoming::Event(receiver.recv())
            };
            match incoming {
                PlainIncoming::Event(Ok(event)) => {
                    if let Err(error) = render_event(
                        event,
                        verbose,
                        &mut seen_items,
                        &mut seen_warnings,
                        &mut failure,
                        &mut output,
                    ) {
                        break Some((error, None));
                    }
                }
                PlainIncoming::Event(Err(_)) => {
                    break drain_decisions(&decisions)
                        .err()
                        .map(|failure| (failure.error, Some(*failure.request)));
                }
                PlainIncoming::Decision(Ok(request)) => {
                    if let Err(error) = decisions.respond(&request, request.prompt.safe_default()) {
                        break Some((error, Some(request)));
                    }
                }
                PlainIncoming::Decision(Err(_)) => decisions_open = false,
            }
        }
    });
    if let Some((error, request)) = ui_error {
        super::settle_worker_after_ui_error(&receiver, &decisions, worker, request);
        return Err(error);
    }
    super::finish_worker(worker, failure)
}

enum PlainIncoming {
    Event(Result<Event, crossbeam_channel::RecvError>),
    Decision(Result<Request, crossbeam_channel::RecvError>),
}

fn render_event(
    event: Event,
    verbose: bool,
    seen_items: &mut HashSet<String>,
    seen_warnings: &mut HashSet<(String, Option<String>)>,
    failure: &mut Option<(String, Option<String>)>,
    output: &mut PlainOutput,
) -> Result<(), String> {
    match event {
        Event::Started {
            profile,
            dry_run,
            peer,
        } if verbose => {
            let operation = if peer.is_some() {
                "push"
            } else if dry_run {
                "plan"
            } else {
                "sync"
            };
            match peer {
                Some(peer) => output.stdout_line(&format!(
                    "{operation} {} -> {}",
                    super::sanitize_text(&profile),
                    super::sanitize_text(&peer)
                ))?,
                None => output
                    .stdout_line(&format!("{operation} {}", super::sanitize_text(&profile)))?,
            }
        }
        Event::PhaseStarted { phase, .. } if verbose => {
            output.stdout_line(super::phase_name(phase))?;
        }
        Event::Item {
            action,
            path,
            detail,
            changed,
        } if verbose && (action != crate::event::Action::Check || changed) => {
            let line = super::item_line(action, &path, &detail);
            if seen_items.insert(line.clone()) {
                output.stdout_line(&line)?;
            }
        }
        Event::Warning { message, hint } => {
            let key = (message.clone(), hint.clone());
            if seen_warnings.insert(key) {
                output.stderr_line(&format!("warning: {}", super::sanitize_text(&message)))?;
                if let Some(hint) = hint {
                    output.stderr_line(&format!("  hint: {}", super::sanitize_text(&hint)))?;
                }
            }
        }
        Event::Failed { message, hint, .. } => {
            *failure = Some((
                super::sanitize_text(&message),
                hint.map(|hint| super::sanitize_text(&hint)),
            ));
        }
        _ => {}
    }
    Ok(())
}

struct PlainOutput {
    stdout: io::Stdout,
    stderr: io::Stderr,
}

impl PlainOutput {
    fn stdout_line(&mut self, line: &str) -> Result<(), String> {
        writeln!(self.stdout, "{line}")
            .map_err(|error| format!("unable to write sync activity: {error}"))
    }

    fn stderr_line(&mut self, line: &str) -> Result<(), String> {
        writeln!(self.stderr, "{line}")
            .map_err(|error| format!("unable to write sync warning: {error}"))
    }
}

struct DecisionDrainError {
    error: String,
    request: Box<Request>,
}

fn drain_decisions(decisions: &Server) -> Result<(), DecisionDrainError> {
    while let Some(request) = decisions.try_recv() {
        if let Err(error) = decisions.respond(&request, request.prompt.safe_default()) {
            return Err(DecisionDrainError {
                error,
                request: Box::new(request),
            });
        }
    }
    Ok(())
}
