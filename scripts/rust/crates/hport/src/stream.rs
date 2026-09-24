use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Stdio};
use std::thread;

use crossbeam_channel::Sender;

use crate::listener::Listener;
use crate::master::Master;

pub const REMOTE: &str =
    r#"exec "${DOTFILES_COMPILED:-$HOME/dotfiles/.bin}/hport" listeners --watch"#;

pub enum Event {
    Snapshot(Vec<Listener>),
    Ended(String),
}

// The peer's watcher exits on stdin EOF: when this child dies or the link drops
pub struct Stream {
    child: Child,
}

impl Stream {
    pub fn spawn(master: &Master, events: Sender<Event>) -> Result<Stream, String> {
        let mut child = master
            .session(REMOTE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(hostkit::ssh::command_error)?;
        let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err("ssh session has no pipes".into());
        };
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let event = match serde_json::from_str::<Vec<Listener>>(&line) {
                    Ok(listeners) => Event::Snapshot(listeners),
                    Err(error) => Event::Ended(format!("peer listeners: {error}")),
                };
                let ended = matches!(event, Event::Ended(_));
                if events.send(event).is_err() || ended {
                    return;
                }
            }
            let mut reason = String::new();
            let _ = stderr.read_to_string(&mut reason);
            let _ = events.send(Event::Ended(hostkit::ssh::stderr_reason(
                reason.as_bytes(),
                "peer listener stream ended",
            )));
        });
        Ok(Stream { child })
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
