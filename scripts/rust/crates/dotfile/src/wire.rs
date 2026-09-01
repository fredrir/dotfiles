use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::process::ExitCode;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::Receiver;

use crate::cli::SyncCli;
use crate::decision::{Choice, Prompt, Request, Server};
use crate::event::{Event, Summary};
use crate::push::protocol::{self, Message};

enum Invocation {
    Probe(u32),
    Run(u32, Vec<OsString>),
}

pub fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    let invocation = match invocation(arguments)? {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("dotfile: {error}");
            return Some(ExitCode::from(2));
        }
    };
    match invocation {
        Invocation::Probe(version) => Some(
            if version == protocol::VERSION && crate::tooling::native_current().unwrap_or(false) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            },
        ),
        Invocation::Run(version, remaining) => {
            crate::cancel::reset();
            let mut output = std::io::stdout().lock();
            if version != protocol::VERSION {
                let error = format!(
                    "wire protocol {version} is incompatible with {}",
                    protocol::VERSION
                );
                let _ = write_message(
                    &mut output,
                    &Message::Error {
                        operation: "protocol".to_string(),
                        value: error.clone(),
                        code: Some(2),
                    },
                );
                eprintln!("dotfile: {error}");
                return Some(ExitCode::from(2));
            }
            let cli = match SyncCli::parse_tail(remaining) {
                Ok(cli) if !cli.push && cli.to.is_none() => cli,
                Ok(_) => {
                    let error = "wire sync cannot start another remote push".to_string();
                    let _ = write_message(
                        &mut output,
                        &Message::Error {
                            operation: "protocol".to_string(),
                            value: error.clone(),
                            code: Some(2),
                        },
                    );
                    eprintln!("dotfile: {error}");
                    return Some(ExitCode::from(2));
                }
                Err(error) => {
                    let message = error.to_string();
                    let _ = write_message(
                        &mut output,
                        &Message::Error {
                            operation: "arguments".to_string(),
                            value: message.clone(),
                            code: Some(2),
                        },
                    );
                    eprintln!("{message}");
                    return Some(ExitCode::from(2));
                }
            };
            let mut input = BufReader::new(std::io::stdin().lock());
            match run_session(cli, &mut input, &mut output) {
                Ok(()) => Some(ExitCode::SUCCESS),
                Err(error) => {
                    let _ = write_message(
                        &mut output,
                        &Message::Error {
                            operation: "sync".to_string(),
                            value: error.clone(),
                            code: Some(1),
                        },
                    );
                    eprintln!("dotfile: {error}");
                    Some(if error == "cancelled" {
                        ExitCode::from(130)
                    } else {
                        ExitCode::FAILURE
                    })
                }
            }
        }
    }
}

fn invocation(arguments: &[OsString]) -> Option<Result<Invocation, String>> {
    let mut mode = None::<(&str, usize, Option<String>)>;
    for (index, argument) in arguments.iter().enumerate() {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        let candidate = if argument == "--wire" || argument == "--wire-probe" {
            Some((argument, None))
        } else if let Some(value) = argument.strip_prefix("--wire=") {
            Some(("--wire", Some(value.to_string())))
        } else {
            argument
                .strip_prefix("--wire-probe=")
                .map(|value| ("--wire-probe", Some(value.to_string())))
        };
        if let Some((name, value)) = candidate {
            if mode.is_some() {
                return Some(Err("only one wire mode may be selected".to_string()));
            }
            mode = Some((name, index, value));
        }
    }
    let (name, index, inline) = mode?;
    let (version, consumed) = match inline {
        Some(value) => (value, 1),
        None => {
            let Some(value) = arguments.get(index + 1).and_then(|value| value.to_str()) else {
                return Some(Err(format!("{name} requires a protocol version")));
            };
            (value.to_string(), 2)
        }
    };
    let version = match version.parse::<u32>() {
        Ok(version) => version,
        Err(_) => return Some(Err(format!("invalid wire protocol version '{version}'"))),
    };
    if name == "--wire-probe" {
        if arguments.len() != consumed {
            return Some(Err(
                "--wire-probe accepts only a protocol version".to_string()
            ));
        }
        return Some(Ok(Invocation::Probe(version)));
    }
    let mut remaining = arguments.to_vec();
    remaining.drain(index..index + consumed);
    Some(Ok(Invocation::Run(version, remaining)))
}

fn run_session(
    cli: SyncCli,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<(), String> {
    write_message(
        output,
        &Message::SyncReady {
            version: protocol::VERSION,
        },
    )?;
    let (event_sender, event_receiver) = crossbeam_channel::bounded(256);
    let (decision_client, decision_server) = crate::decision::channel();
    let worker =
        std::thread::spawn(move || crate::sync::run(&cli, &event_sender, &decision_client));
    let result = bridge_worker(input, output, &event_receiver, &decision_server, worker);
    if result.is_ok() {
        write_message(output, &Message::Completed)?;
    }
    result
}

fn bridge_worker(
    input: &mut impl BufRead,
    output: &mut impl Write,
    events: &Receiver<Event>,
    decisions: &Server,
    worker: JoinHandle<Result<Summary, String>>,
) -> Result<(), String> {
    let mut events_open = true;
    let mut decisions_open = true;
    let mut failure = None;
    while events_open || decisions_open {
        if events_open && decisions_open {
            crossbeam_channel::select! {
                recv(events) -> event => match event {
                    Ok(event) => {
                        if let Err(error) = write_message(output, &Message::Event { value: event }) {
                            failure = Some(error);
                            break;
                        }
                    }
                    Err(_) => events_open = false,
                },
                recv(decisions.requests()) -> request => match request {
                    Ok(request) => {
                        if let Err(error) = relay_decision(input, output, decisions, &request) {
                            settle_worker(events, decisions, worker, Some(request));
                            return Err(error);
                        }
                    }
                    Err(_) => decisions_open = false,
                },
            }
        } else if events_open {
            match events.recv() {
                Ok(event) => {
                    if let Err(error) = write_message(output, &Message::Event { value: event }) {
                        failure = Some(error);
                        break;
                    }
                }
                Err(_) => events_open = false,
            }
        } else if decisions_open {
            match decisions.requests().recv() {
                Ok(request) => {
                    if let Err(error) = relay_decision(input, output, decisions, &request) {
                        settle_worker(events, decisions, worker, Some(request));
                        return Err(error);
                    }
                }
                Err(_) => decisions_open = false,
            }
        }
    }
    if let Some(error) = failure {
        settle_worker(events, decisions, worker, None);
        return Err(error);
    }
    worker
        .join()
        .map_err(|_| "wire sync worker panicked".to_string())?
        .map(|_| ())
}

fn relay_decision(
    input: &mut impl BufRead,
    output: &mut impl Write,
    decisions: &Server,
    request: &Request,
) -> Result<(), String> {
    write_message(
        output,
        &Message::DecisionRequest {
            id: request.id,
            prompt: request.prompt.clone(),
        },
    )?;
    let mut line = String::new();
    if input
        .read_line(&mut line)
        .map_err(|error| error.to_string())?
        == 0
    {
        return Err("the remote decision stream ended unexpectedly".to_string());
    }
    let response = protocol::decode(line.trim_end())?;
    let Message::DecisionResponse { id, choice } = response else {
        return Err("expected a decision response from the remote controller".to_string());
    };
    if id != request.id {
        return Err(format!(
            "decision response {id} does not match request {}",
            request.id
        ));
    }
    if !valid_choice(&request.prompt, choice) {
        return Err(format!("invalid choice {choice:?} for this decision"));
    }
    decisions.respond(request, choice)
}

fn valid_choice(prompt: &Prompt, choice: Choice) -> bool {
    match prompt {
        Prompt::Merge { .. } => matches!(
            choice,
            Choice::Repo | Choice::Live | Choice::Ignore | Choice::Skip | Choice::Abort
        ),
        Prompt::MergeTarget { targets, .. } => match choice {
            Choice::Target(index) => index < targets.len(),
            Choice::Cancel => true,
            _ => false,
        },
        Prompt::RemoteChanges { .. } => matches!(choice, Choice::Discard | Choice::Cancel),
    }
}

fn settle_worker(
    events: &Receiver<Event>,
    decisions: &Server,
    worker: JoinHandle<Result<Summary, String>>,
    pending: Option<Request>,
) {
    crate::cancel::request();
    if let Some(request) = pending {
        let _ = decisions.respond(&request, cancellation_choice(&request.prompt));
    }
    while !worker.is_finished() {
        while events.try_recv().is_ok() {}
        while let Some(request) = decisions.try_recv() {
            let _ = decisions.respond(&request, cancellation_choice(&request.prompt));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    while events.try_recv().is_ok() {}
    let _ = worker.join();
}

fn cancellation_choice(prompt: &Prompt) -> Choice {
    match prompt {
        Prompt::Merge { .. } => Choice::Abort,
        Prompt::MergeTarget { .. } | Prompt::RemoteChanges { .. } => Choice::Cancel,
    }
}

fn write_message(output: &mut impl Write, message: &Message) -> Result<(), String> {
    let frame = protocol::encode(message)?;
    writeln!(output, "{frame}")
        .and_then(|()| output.flush())
        .map_err(|error| format!("cannot write wire response: {error}"))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;

    fn summary() -> Summary {
        Summary {
            profile: "test".to_string(),
            peer: None,
            remote_changed: None,
            checked: 1,
            changed: 0,
            links: 0,
            merges: 0,
            secrets: 0,
            generated: 0,
            dry_run: false,
            elapsed: Duration::ZERO,
        }
    }

    fn merge_prompt() -> Prompt {
        Prompt::Merge {
            path: PathBuf::from("/tmp/settings.json"),
            key: "font".to_string(),
            repo: "mono".to_string(),
            live: "sans".to_string(),
        }
    }

    #[test]
    fn sync_wire_round_trips_merge_and_target_with_exact_ids() {
        let (event_sender, event_receiver) = crossbeam_channel::bounded(8);
        let (client, server) = crate::decision::channel();
        let worker = std::thread::spawn(move || {
            event_sender
                .send(Event::Started {
                    profile: "test".to_string(),
                    dry_run: false,
                    peer: None,
                })
                .unwrap();
            assert_eq!(client.choose(merge_prompt()), Ok(Choice::Live));
            assert_eq!(
                client.choose(Prompt::MergeTarget {
                    path: PathBuf::from("/tmp/settings.json"),
                    key: "font".to_string(),
                    targets: vec!["shared".to_string(), "macos".to_string()],
                    default: 1,
                }),
                Ok(Choice::Target(1))
            );
            Ok(summary())
        });
        let responses = [
            Message::DecisionResponse {
                id: 1,
                choice: Choice::Live,
            },
            Message::DecisionResponse {
                id: 2,
                choice: Choice::Target(1),
            },
        ]
        .iter()
        .map(protocol::encode)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n")
            + "\n";
        let mut input = Cursor::new(responses);
        let mut output = Vec::new();

        assert_eq!(
            bridge_worker(&mut input, &mut output, &event_receiver, &server, worker),
            Ok(())
        );

        let messages = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(protocol::decode)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let requests = messages
            .iter()
            .filter(|message| matches!(message, Message::DecisionRequest { .. }))
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        assert!(matches!(
            requests[0],
            Message::DecisionRequest {
                id: 1,
                prompt: Prompt::Merge { .. }
            }
        ));
        assert!(matches!(
            requests[1],
            Message::DecisionRequest {
                id: 2,
                prompt: Prompt::MergeTarget { .. }
            }
        ));
    }

    #[test]
    fn sync_wire_rejects_eof_malformed_or_mismatched_response() {
        for response in [
            String::new(),
            "not-json\n".to_string(),
            protocol::encode(&Message::DecisionResponse {
                id: 99,
                choice: Choice::Repo,
            })
            .unwrap()
                + "\n",
        ] {
            let (_client, server) = crate::decision::channel();
            let request = Request {
                id: 1,
                prompt: merge_prompt(),
            };
            let mut input = Cursor::new(response);
            let mut output = Vec::new();

            assert!(relay_decision(&mut input, &mut output, &server, &request).is_err());
        }
    }
}
