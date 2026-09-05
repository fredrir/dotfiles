use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event as TerminalEvent, KeyCode, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tui_kit::{Alternate, MouseCapture};
use unicode_width::UnicodeWidthStr;
use workstation::screen::{SignalGuard, termination_requested};

use super::{
    CatalogSnapshot, CatalogSource, Effect, FavoriteStore, Model, PickedSession, PickerOptions,
    PickerOutcome, Preview, UiEvent, render,
};

const INPUT_POLL: Duration = Duration::from_millis(25);
const ANIMATION_FRAME: Duration = Duration::from_millis(16);
const MAX_PREVIEW_JOBS: usize = 2;

/// Run the picker with discovery and preview reads on a dedicated worker.
///
/// Taking ownership is intentional: the worker may still be blocked in SSH
/// when the user cancels. The UI can restore the terminal immediately and
/// detach that worker instead of waiting for a borrowed source to return.
pub(crate) fn run(
    source: impl CatalogSource + Clone + Send + 'static,
    mut favorites: impl FavoriteStore,
    options: PickerOptions,
) -> Result<PickerOutcome, String> {
    if !super::capable() {
        return Err("the interactive session picker needs a terminal".into());
    }

    // Drop order matters: the terminal must restore before a pending signal is
    // re-raised with the caller's original disposition.
    let _signals = SignalGuard::new()
        .map_err(|error| format!("could not watch for terminal signals: {error}"))?;
    let mut terminal = PickerTerminal::new()
        .map_err(|error| format!("could not open the session picker: {error}"))?;
    let worker = SourceWorker::new(source);
    let mut model = Model::new();
    let area = terminal
        .screen
        .terminal()
        .size()
        .map_err(|error| format!("could not inspect the terminal: {error}"))?;
    model.area = area.into();
    model.set_reduced_motion(options.reduced_motion);
    model.set_initial_action(options.initial_action);
    model.set_view(options.initial_view);
    terminal.draw(&model, options)?;
    worker.send(WorkerRequest::Refresh)?;
    let mut effect = Effect::None;
    let mut redraw = false;
    let mut next_animation_frame: Option<Instant> = None;

    loop {
        if termination_requested() {
            return Ok(PickerOutcome::Cancelled(model.view()));
        }

        if effect != Effect::None {
            redraw = true;
            match handle_effect(effect, &worker, &mut favorites, &mut model)? {
                LoopState::Continue(next) => {
                    effect = next;
                    if effect != Effect::None {
                        continue;
                    }
                }
                LoopState::Finish(outcome) => return Ok(outcome),
            }
        }

        while let Some(response) = worker.try_response()? {
            redraw = true;
            let response_effect = handle_response(response, &mut model);
            if response_effect != Effect::None {
                effect = response_effect;
                break;
            }
        }

        redraw |= advance_animation_if_due(&mut model, &mut next_animation_frame, Instant::now());

        if redraw {
            terminal.draw(&model, options)?;
            redraw = false;
        }
        if effect != Effect::None {
            continue;
        }

        let poll_timeout = next_animation_frame
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(INPUT_POLL)
            })
            .unwrap_or(INPUT_POLL);
        if !event::poll(poll_timeout)
            .map_err(|error| format!("could not poll terminal input: {error}"))?
        {
            continue;
        }
        let terminal_event =
            event::read().map_err(|error| format!("could not read terminal input: {error}"))?;
        effect = match terminal_event {
            TerminalEvent::Key(key)
                if key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                    && model
                        .text_selection
                        .is_some_and(|selection| selection.dragged()) =>
            {
                redraw = true;
                copy_selected_preview(&terminal, &mut model);
                Effect::None
            }
            TerminalEvent::Key(key) => {
                redraw = true;
                model.apply(UiEvent::Key(key))
            }
            TerminalEvent::Resize(width, height) => {
                redraw = true;
                model.apply(UiEvent::Resize(width, height))
            }
            TerminalEvent::Mouse(mouse) => {
                redraw = true;
                let effect = model.apply(UiEvent::Mouse {
                    kind: mouse.kind,
                    column: mouse.column,
                    row: mouse.row,
                });
                if mouse.kind == MouseEventKind::Up(MouseButton::Left)
                    && model
                        .text_selection
                        .is_some_and(|selection| selection.dragged())
                {
                    copy_selected_preview(&terminal, &mut model);
                }
                effect
            }
            TerminalEvent::FocusGained | TerminalEvent::FocusLost | TerminalEvent::Paste(_) => {
                Effect::None
            }
        };
    }
}

fn copy_selected_preview(terminal: &PickerTerminal, model: &mut Model) {
    let Some(text) = terminal.selected_text(model) else {
        return;
    };
    match copy_with_dclip(&text) {
        Ok(()) => model.selected_text_copied(),
        Err(error) => model.selected_text_copy_failed(error),
    }
}

fn advance_animation_if_due(
    model: &mut Model,
    next_frame: &mut Option<Instant>,
    now: Instant,
) -> bool {
    if !model.is_animating() {
        *next_frame = None;
        return false;
    }
    let deadline = next_frame.get_or_insert_with(|| now + ANIMATION_FRAME);
    if now < *deadline {
        return false;
    }
    let changed = model.tick_animation();
    *deadline = now + ANIMATION_FRAME;
    changed
}

enum LoopState {
    Continue(Effect),
    Finish(PickerOutcome),
}

fn handle_effect(
    effect: Effect,
    worker: &SourceWorker,
    favorites: &mut impl FavoriteStore,
    model: &mut Model,
) -> Result<LoopState, String> {
    match effect {
        Effect::None => Ok(LoopState::Continue(Effect::None)),
        Effect::Cancel => Ok(LoopState::Finish(PickerOutcome::Cancelled(model.view()))),
        Effect::Pick(action) => {
            let session = model
                .preview_entry()
                .cloned()
                .ok_or_else(|| "the selected session disappeared".to_string())?;
            if let Some(reason) = &session.disabled_reason {
                model.status = Some(format!("Unavailable: {reason}"));
                return Ok(LoopState::Continue(Effect::None));
            }
            Ok(LoopState::Finish(PickerOutcome::Picked(Box::new(
                PickedSession {
                    session,
                    action,
                    view: model.view(),
                },
            ))))
        }
        Effect::Refresh => {
            model.begin_refresh();
            worker.send(WorkerRequest::Refresh)?;
            Ok(LoopState::Continue(Effect::None))
        }
        Effect::LoadPreview(key) => {
            worker.send(WorkerRequest::Preview(key))?;
            Ok(LoopState::Continue(Effect::None))
        }
        Effect::SetFavorite { key, favorite } => {
            if let Err(error) = favorites.set_favorite(&key, favorite) {
                model.favorite_failed(&key, !favorite, error);
            } else if model.status.as_deref()
                != Some("Session no longer matches the active filters")
            {
                model.status = Some(if favorite {
                    "Added to favorites".into()
                } else {
                    "Removed from favorites".into()
                });
            }
            Ok(LoopState::Continue(Effect::None))
        }
        Effect::CopySessionDescription(description) => {
            match copy_with_dclip(&description) {
                Ok(()) => model.copied(),
                Err(error) => model.copy_failed(error),
            }
            Ok(LoopState::Continue(Effect::None))
        }
    }
}

fn handle_response(response: WorkerResponse, model: &mut Model) -> Effect {
    match response {
        WorkerResponse::Refreshed { complete, result } => match result {
            Ok(snapshot) => model.load(snapshot, complete),
            Err(error) => {
                model.load_failed(error, complete);
                Effect::None
            }
        },
        WorkerResponse::Previewed { key, result } => {
            model.preview_loaded(&key, result);
            Effect::None
        }
        WorkerResponse::PreviewSkipped(key) => model.preview_skipped(&key),
    }
}

enum WorkerRequest {
    Refresh,
    Preview(String),
    #[cfg(test)]
    Barrier(Sender<()>),
    Shutdown,
}

enum WorkerResponse {
    Refreshed {
        complete: bool,
        result: Result<CatalogSnapshot, String>,
    },
    Previewed {
        key: String,
        result: Result<Preview, String>,
    },
    PreviewSkipped(String),
}

struct SourceWorker {
    requests: Sender<WorkerRequest>,
    responses: Receiver<WorkerResponse>,
    /// Deliberately not joined: cancellation must not wait for stuck SSH.
    thread: Option<JoinHandle<()>>,
}

struct RemoteJob {
    local: CatalogSnapshot,
    receiver: Receiver<Result<CatalogSnapshot, String>>,
}

struct PreviewJob {
    generation: u64,
    key: String,
    receiver: Receiver<Result<Preview, String>>,
}

fn spawn_remote_job(
    mut source: impl CatalogSource + Send + 'static,
    local: CatalogSnapshot,
) -> RemoteJob {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(source.refresh_remote());
    });
    RemoteJob { local, receiver }
}

fn spawn_preview_job(
    mut source: impl CatalogSource + Send + 'static,
    generation: u64,
    key: String,
) -> PreviewJob {
    let (sender, receiver) = mpsc::channel();
    let job_key = key.clone();
    thread::spawn(move || {
        let _ = sender.send(source.preview(&job_key));
    });
    PreviewJob {
        generation,
        key,
        receiver,
    }
}

impl SourceWorker {
    fn new(mut source: impl CatalogSource + Clone + Send + 'static) -> Self {
        let (request_sender, request_receiver) = mpsc::channel();
        let (response_sender, response_receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut remote_job: Option<RemoteJob> = None;
            let mut pending_remote: Option<CatalogSnapshot> = None;
            let mut preview_jobs: Vec<PreviewJob> = Vec::new();
            let mut preview_generation = 0_u64;
            let mut desired_preview: Option<(u64, String)> = None;
            'worker: loop {
                if let Some(job) = remote_job.take() {
                    match job.receiver.try_recv() {
                        Ok(result) => {
                            if let Some(local) = pending_remote.take() {
                                remote_job = Some(spawn_remote_job(source.clone(), local));
                            } else {
                                let result = result.map(|remote| job.local.merge(remote));
                                if response_sender
                                    .send(WorkerResponse::Refreshed {
                                        complete: true,
                                        result,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            continue;
                        }
                        Err(TryRecvError::Disconnected) => {
                            if let Some(local) = pending_remote.take() {
                                remote_job = Some(spawn_remote_job(source.clone(), local));
                            } else if response_sender
                                    .send(WorkerResponse::Refreshed {
                                        complete: true,
                                        result: Err(
                                            "remote catalog worker stopped unexpectedly".into()
                                        ),
                                    })
                                    .is_err()
                            {
                                break;
                            }
                        }
                        Err(TryRecvError::Empty) => remote_job = Some(job),
                    }
                }

                let mut completed = Vec::new();
                for (index, job) in preview_jobs.iter().enumerate() {
                    match job.receiver.try_recv() {
                        Ok(result) => completed.push((index, result)),
                        Err(TryRecvError::Disconnected) => completed.push((
                            index,
                            Err("preview worker stopped unexpectedly".to_string()),
                        )),
                        Err(TryRecvError::Empty) => {}
                    }
                }
                for (index, result) in completed.into_iter().rev() {
                    let job = preview_jobs.swap_remove(index);
                    if desired_preview
                        .as_ref()
                        .is_some_and(|(generation, _)| *generation == job.generation)
                    {
                        desired_preview = None;
                        if response_sender
                            .send(WorkerResponse::Previewed {
                                key: job.key,
                                result,
                            })
                            .is_err()
                        {
                            break 'worker;
                        }
                    }
                }
                if let Some((generation, key)) = desired_preview.as_ref()
                    && preview_jobs.len() < MAX_PREVIEW_JOBS
                    && !preview_jobs.iter().any(|job| job.generation == *generation)
                {
                    preview_jobs.push(spawn_preview_job(source.clone(), *generation, key.clone()));
                }

                let request = if remote_job.is_some() || !preview_jobs.is_empty() {
                    match request_receiver.recv_timeout(Duration::from_millis(10)) {
                        Ok(request) => request,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                } else {
                    match request_receiver.recv() {
                        Ok(request) => request,
                        Err(_) => break,
                    }
                };
                match request {
                    WorkerRequest::Refresh => {
                        // One active remote lookup and one pending generation is
                        // enough. Further refresh keys collapse into the latter.
                        if pending_remote.is_none() {
                            match source.refresh_local() {
                                Ok(local) => {
                                    if response_sender
                                        .send(WorkerResponse::Refreshed {
                                            complete: false,
                                            result: Ok(local.clone()),
                                        })
                                        .is_err()
                                    {
                                        break;
                                    }
                                    if remote_job.is_some() {
                                        pending_remote = Some(local);
                                    } else {
                                        remote_job = Some(spawn_remote_job(source.clone(), local));
                                    }
                                }
                                Err(error) => {
                                    if response_sender
                                        .send(WorkerResponse::Refreshed {
                                            complete: true,
                                            result: Err(error),
                                        })
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    WorkerRequest::Preview(key) => {
                        if desired_preview
                            .as_ref()
                            .is_some_and(|(_, desired)| desired == &key)
                        {
                            continue;
                        }
                        if let Some((_, previous)) = desired_preview.take()
                            && previous != key
                            && response_sender
                                .send(WorkerResponse::PreviewSkipped(previous))
                                .is_err()
                        {
                            break;
                        }
                        if let Some(job) = preview_jobs.iter().find(|job| job.key == key) {
                            desired_preview = Some((job.generation, key));
                        } else {
                            preview_generation = preview_generation.wrapping_add(1);
                            desired_preview = Some((preview_generation, key));
                        }
                    }
                    #[cfg(test)]
                    WorkerRequest::Barrier(sender) => {
                        let _ = sender.send(());
                    }
                    WorkerRequest::Shutdown => break,
                }
            }
        });
        Self {
            requests: request_sender,
            responses: response_receiver,
            thread: Some(thread),
        }
    }

    fn send(&self, request: WorkerRequest) -> Result<(), String> {
        self.requests
            .send(request)
            .map_err(|_| "the session catalog worker stopped unexpectedly".into())
    }

    fn try_response(&self) -> Result<Option<WorkerResponse>, String> {
        match self.responses.try_recv() {
            Ok(response) => Ok(Some(response)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err("the session catalog worker stopped unexpectedly".into())
            }
        }
    }
}

impl Drop for SourceWorker {
    fn drop(&mut self) {
        let _ = self.requests.send(WorkerRequest::Shutdown);
        // Dropping JoinHandle detaches.
        let _ = self.thread.take();
    }
}

struct PickerTerminal {
    screen: Alternate,
    last_frame: Buffer,
}

impl PickerTerminal {
    fn new() -> io::Result<Self> {
        Ok(Self {
            screen: Alternate::new(MouseCapture::Enabled)?,
            last_frame: Buffer::empty(Rect::default()),
        })
    }

    fn draw(&mut self, model: &Model, options: PickerOptions) -> Result<(), String> {
        let frame = self
            .screen
            .terminal()
            .draw(|frame| render(frame, model, options))
            .map_err(|error| format!("could not render the session picker: {error}"))?;
        self.last_frame = frame.buffer.clone();
        Ok(())
    }

    fn selected_text(&self, model: &Model) -> Option<String> {
        let selection = model
            .text_selection
            .filter(|selection| selection.dragged())?;
        let area = super::view::preview_text_area(model);
        selected_text_from_buffer(&self.last_frame, selection, area)
    }
}

fn selected_text_from_buffer(
    buffer: &Buffer,
    selection: super::model::TextSelection,
    area: Rect,
) -> Option<String> {
    if area.is_empty() {
        return None;
    }
    let (start, end) = selection.ordered();
    let mut rows = Vec::with_capacity(usize::from(end.y.saturating_sub(start.y)) + 1);
    for y in start.y..=end.y {
        let mut row = String::new();
        let mut x = area.x;
        while x < area.right() {
            let point = ratatui::layout::Position::new(x, y);
            let Some(cell) = buffer.cell(point) else {
                break;
            };
            let symbol = cell.symbol();
            if selection.contains(point, area) {
                row.push_str(symbol);
            }
            x = x.saturating_add(UnicodeWidthStr::width(symbol).max(1) as u16);
        }
        rows.push(row.trim_end_matches(' ').to_owned());
    }
    let text = rows.join("\n");
    let text = text.trim_matches([' ', '\n']);
    (!text.is_empty()).then(|| text.to_owned())
}

fn copy_with_dclip(value: &str) -> Result<(), String> {
    let mut child = Command::new("dclip")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start dclip: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "dclip did not accept input".to_string())?
        .write_all(value.as_bytes())
        .map_err(|error| format!("could not send clipboard text to dclip: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not wait for dclip: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if detail.is_empty() {
            format!("dclip exited with {}", output.status)
        } else {
            format!("dclip failed: {detail}")
        })
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tui/terminal_tests.rs"]
mod tests;
