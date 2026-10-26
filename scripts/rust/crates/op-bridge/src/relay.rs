use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use zeroize::Zeroizing;

use crate::broker::{Policy, Tier};
use crate::daemon::{self, POISONED, REQUEST_TIMEOUT};
use crate::paths;
use crate::protocol::{self, ANSWER_TIMEOUT, Request, Response};

pub fn run(policy: Policy) -> Result<(), String> {
    let socket = paths::client()?;
    let tunnel = paths::tunnel();
    let listener = daemon::bind(&socket)?;
    eprintln!(
        "op-bridge: listening on {}; {}",
        socket.display(),
        policy.describe()
    );
    let memory = Arc::new(Mutex::new(Memory::new(policy)));
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("op-bridge: accept: {error}");
                continue;
            }
        };
        let (memory, tunnel) = (Arc::clone(&memory), tunnel.clone());
        thread::spawn(move || {
            if let Err(error) = serve(&stream, &memory, &tunnel) {
                eprintln!("op-bridge: {error}");
            }
        });
    }
    Ok(())
}

// Silent values kept until the daemon stops; everything else is macie's to answer
struct Memory {
    policy: Policy,
    kept: HashMap<String, Zeroizing<String>>,
    generation: u64,
}

impl Memory {
    fn new(policy: Policy) -> Memory {
        Memory {
            policy,
            kept: HashMap::new(),
            generation: 0,
        }
    }

    fn forget(&mut self) {
        self.kept.clear();
        self.generation += 1;
    }

    // A fetch that straddled a forget may carry the value it was meant to drop
    fn keep(&mut self, reference: &str, value: &Zeroizing<String>, generation: u64) {
        if generation == self.generation && self.policy.tier(reference) == Ok(Tier::Silent) {
            self.kept.insert(reference.to_string(), value.clone());
        }
    }
}

fn serve(stream: &UnixStream, memory: &Mutex<Memory>, tunnel: &Path) -> Result<(), String> {
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|error| error.to_string())?;
    let request = protocol::receive(stream)?;
    let response = answer(memory, &request, |request| forward(tunnel, request))?;
    protocol::send(stream, &response)
}

fn forward(tunnel: &Path, request: &Request) -> Result<Response, String> {
    let stream =
        UnixStream::connect(tunnel).map_err(|error| format!("macie unreachable: {error}"))?;
    protocol::ask(&stream, request, ANSWER_TIMEOUT)
        .map_err(|error| format!("macie did not answer: {error}"))
}

// Holds no lock while macie answers, so a Touch ID prompt never stalls a kept read
fn answer(
    memory: &Mutex<Memory>,
    request: &Request,
    forward: impl FnOnce(&Request) -> Result<Response, String>,
) -> Result<Response, String> {
    match request {
        Request::Read(reference) => {
            let generation = {
                let memory = memory.lock().map_err(|_| POISONED)?;
                if let Some(value) = memory.kept.get(reference) {
                    eprintln!("op-bridge: {reference}: sent from memory");
                    return Ok(Response::Value(value.clone()));
                }
                memory.generation
            };
            let response = forward(request).unwrap_or_else(Response::Refused);
            if let Response::Value(value) = &response {
                memory
                    .lock()
                    .map_err(|_| POISONED)?
                    .keep(reference, value, generation);
            }
            eprintln!(
                "op-bridge: {reference}: {}",
                daemon::outcome(&response, false)
            );
            Ok(response)
        }
        Request::Reload => {
            memory.lock().map_err(|_| POISONED)?.forget();
            eprintln!("op-bridge: reload: cache cleared");
            Ok(forward(request)
                .unwrap_or_else(|reason| Response::Refused(format!("{reason}; cleared archie only"))))
        }
        Request::Forget => {
            memory.lock().map_err(|_| POISONED)?.forget();
            eprintln!("op-bridge: macie reload: cache cleared");
            Ok(Response::Forgotten)
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/relay_tests.rs"]
mod tests;
