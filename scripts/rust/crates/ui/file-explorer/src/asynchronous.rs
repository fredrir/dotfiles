use crate::state::State;
use crate::{
    Directory, Explorer, ExplorerError, ExplorerView, FileSource, InputKind, Key, Outcome,
    SystemExplorerResult, SystemTerminal, Terminal, TerminalExplorerResult,
};
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

pub struct AsyncExplorer<'a, S: FileSource, V = crate::DefaultView> {
    explorer: Explorer<'a, S, V>,
}

impl<'a, S: FileSource, V> AsyncExplorer<'a, S, V> {
    pub fn new(explorer: Explorer<'a, S, V>) -> Self {
        Self { explorer }
    }
}

struct Request<L> {
    generation: u64,
    location: L,
    refresh: bool,
}
struct Response<L, E> {
    generation: u64,
    result: Result<Directory<L>, E>,
}

struct Worker<L, E> {
    requests: SyncSender<Request<L>>,
    responses: Receiver<Response<L, E>>,
    cancellation: Cancellation,
}

impl<L, E> Drop for Worker<L, E> {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl<S, V> AsyncExplorer<'_, S, V>
where
    S: FileSource + Clone + Send + 'static,
    S::Location: Send + 'static,
    S::Error: Send + fmt::Display + 'static,
    V: ExplorerView<S::Location>,
{
    pub fn run(&self) -> SystemExplorerResult<S::Location, S::Error> {
        let Some(mut terminal) = SystemTerminal::open().map_err(ExplorerError::Terminal)? else {
            return Ok(Outcome::Unavailable);
        };
        self.run_in(&mut terminal)
    }

    pub fn run_in<T: Terminal>(
        &self,
        terminal: &mut T,
    ) -> TerminalExplorerResult<S::Location, S::Error, T::Error> {
        let result = self.interact(terminal);
        let cleared = terminal.clear().map_err(ExplorerError::Terminal);
        match (result, cleared) {
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            (Ok(outcome), Ok(())) => Ok(outcome),
        }
    }

    fn worker(&self) -> Worker<S::Location, S::Error> {
        let (requests, receiver) = mpsc::sync_channel::<Request<S::Location>>(1);
        let (sender, responses) = mpsc::sync_channel(1);
        let cancellation = Cancellation::default();
        let cancel = cancellation.clone();
        let source = self.explorer.source.clone();
        std::thread::spawn(move || {
            while let Ok(mut request) = receiver.recv() {
                while let Ok(newer) = receiver.try_recv() {
                    request = newer;
                }
                if cancel.is_cancelled() {
                    break;
                }
                let result = if request.refresh {
                    source.refresh_directory_cancellable(&request.location, &cancel)
                } else {
                    source.read_directory_cancellable(&request.location, &cancel)
                };
                if cancel.is_cancelled()
                    || sender
                        .send(Response {
                            generation: request.generation,
                            result,
                        })
                        .is_err()
                {
                    break;
                }
            }
        });
        Worker {
            requests,
            responses,
            cancellation,
        }
    }

    fn interact<T: Terminal>(
        &self,
        terminal: &mut T,
    ) -> TerminalExplorerResult<S::Location, S::Error, T::Error> {
        let worker = self.worker();
        let mut generation = 0u64;
        let mut pending = Some(Request {
            generation,
            location: self.explorer.start.clone(),
            refresh: false,
        });
        let mut restore = self.explorer.initial_focus.clone();
        let mut state: Option<State<S::Location>> = None;
        let mut loading = true;
        let mut dirty = true;
        let mut help = false;
        let mut prefetched = None;
        let mut viewport_rows = 1;
        let mut style = ui_theme::LiveStyle::new(self.explorer.style);
        loop {
            dirty |= style.poll();
            if let Some(request) = pending.take() {
                match worker.requests.try_send(request) {
                    Ok(()) => {}
                    Err(TrySendError::Full(request)) => pending = Some(request),
                    Err(TrySendError::Disconnected(_)) => return Ok(Outcome::Interrupted),
                }
            }
            loop {
                match worker.responses.try_recv() {
                    Ok(response) if response.generation == generation => {
                        loading = false;
                        dirty = true;
                        match response.result {
                            Ok(directory) => {
                                if let Some(state) = &mut state {
                                    state.replace_directory(directory, restore.as_ref());
                                } else {
                                    let mut initial =
                                        State::new(directory, self.explorer.config.selection);
                                    if let Some(location) = &restore {
                                        initial.focus_location(location);
                                    }
                                    state = Some(initial);
                                }
                            }
                            Err(error) => {
                                if let Some(state) = &mut state {
                                    state.set_error(error.to_string());
                                } else {
                                    return Err(ExplorerError::Source(error));
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(Outcome::Interrupted),
                }
            }
            if dirty {
                let mut lines = if let Some(state) = &mut state {
                    self.explorer.prefetch_focused(state, &mut prefetched);
                    let mut frame = self.explorer.frame(state, terminal, help, style.style());
                    if state.settle(frame.viewport_rows) {
                        frame = self.explorer.frame(state, terminal, help, style.style());
                    }
                    viewport_rows = frame.viewport_rows.max(1);
                    frame.lines
                } else {
                    Vec::new()
                };
                if loading {
                    let size = terminal.size();
                    if size.height > 0 {
                        if lines.len() == size.height {
                            lines.pop();
                        }
                        lines.push(
                            ui_widgets::Line::styled(
                                "Loading…   esc cancel",
                                ui_theme::Role::Muted,
                            )
                            .paint(style.style(), size.width),
                        );
                    }
                }
                terminal.draw(&lines).map_err(ExplorerError::Terminal)?;
                dirty = false;
            }
            let timeout = if loading {
                Duration::from_millis(25)
            } else {
                Duration::from_secs(1)
            };
            let Some(event) = terminal
                .poll_event(timeout)
                .map_err(ExplorerError::Terminal)?
            else {
                continue;
            };
            dirty = true;
            let ui_terminal::Event::Key(key) = event else {
                continue;
            };
            if key == Key::Interrupt {
                return Ok(Outcome::Interrupted);
            }
            if key == Key::Escape
                && (loading || state.as_ref().is_none_or(|state| state.prompt().is_none()))
            {
                return Ok(Outcome::Cancelled);
            }
            if key == Key::Char('q') && state.as_ref().is_none_or(|state| state.prompt().is_none())
            {
                return Ok(Outcome::Cancelled);
            }
            let Some(current) = &mut state else {
                continue;
            };
            let location = match (current.input_kind(), key) {
                (None, Key::Right | Key::Tab | Key::Char('l'))
                | (Some(InputKind::Search), Key::Right | Key::Tab) => current
                    .focused()
                    .filter(|entry| entry.kind.is_directory())
                    .map(|entry| (entry.location.clone(), None, false)),
                (None, Key::Left | Key::Char('h')) => current
                    .directory()
                    .parent
                    .clone()
                    .map(|parent| (parent, Some(current.directory().location.clone()), false)),
                (None, Key::Char('r')) => Some((
                    current.directory().location.clone(),
                    current.focused().map(|entry| entry.location.clone()),
                    true,
                )),
                (Some(InputKind::Location), Key::Enter) => {
                    match self.explorer.source.resolve_input(
                        &current.directory().location,
                        current.prompt().unwrap_or(""),
                    ) {
                        Ok(location) => Some((location, None, false)),
                        Err(error) => {
                            current.set_error(error.to_string());
                            None
                        }
                    }
                }
                _ => None,
            };
            if let Some((location, focus, refresh)) = location {
                generation = generation.wrapping_add(1);
                restore = focus;
                pending = Some(Request {
                    generation,
                    location,
                    refresh,
                });
                loading = true;
            } else if !matches!(
                (current.input_kind(), key),
                (
                    None,
                    Key::Right
                        | Key::Tab
                        | Key::Char('l')
                        | Key::Left
                        | Key::Char('h')
                        | Key::Char('r')
                ) | (Some(InputKind::Search), Key::Right | Key::Tab)
                    | (Some(InputKind::Location), Key::Enter)
            ) && !(loading && key == Key::Enter)
                && let Some(outcome) =
                    self.explorer
                        .apply_key(current, key, viewport_rows, &mut help)
            {
                return Ok(outcome);
            }
        }
    }
}
