use std::io::{self, Stdout, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as TerminalEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
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
        .terminal
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
            TerminalEvent::Key(key) => {
                redraw = true;
                model.apply(UiEvent::Key(key))
            }
            TerminalEvent::Resize(width, height) => {
                redraw = true;
                model.apply(UiEvent::Resize(width, height))
            }
            TerminalEvent::Mouse(_)
            | TerminalEvent::FocusGained
            | TerminalEvent::FocusLost
            | TerminalEvent::Paste(_) => Effect::None,
        };
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
    terminal: Terminal<CrosstermBackend<Stdout>>,
    raw: bool,
    alternate: bool,
}

impl PickerTerminal {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut stdout = io::stdout();
                let _ = execute!(stdout, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(error);
            }
        };
        if let Err(error) = terminal.hide_cursor() {
            let _ = terminal.show_cursor();
            let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self {
            terminal,
            raw: true,
            alternate: true,
        })
    }

    fn draw(&mut self, model: &Model, options: PickerOptions) -> Result<(), String> {
        self.terminal
            .draw(|frame| render(frame, model, options))
            .map(|_| ())
            .map_err(|error| format!("could not render the session picker: {error}"))
    }
}

impl Drop for PickerTerminal {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        if self.alternate {
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            self.alternate = false;
        }
        if self.raw {
            let _ = disable_raw_mode();
            self.raw = false;
        }
    }
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
        .map_err(|error| format!("could not send the session description to dclip: {error}"))?;
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
mod tests {
    use std::sync::{Arc, Condvar, Mutex};

    use super::*;

    #[test]
    fn animation_scheduler_redraws_only_on_due_active_frames() {
        let start = Instant::now();
        let mut model = Model::new();
        let mut next_frame = None;

        assert!(!advance_animation_if_due(
            &mut model,
            &mut next_frame,
            start
        ));
        assert!(next_frame.is_none(), "idle UI must stay event-driven");

        model.pane = super::super::model::Pane::List;
        model.preview_transition = 500;
        assert!(!advance_animation_if_due(
            &mut model,
            &mut next_frame,
            start
        ));
        let deadline = next_frame.expect("active animation schedules a frame");
        assert!(!advance_animation_if_due(
            &mut model,
            &mut next_frame,
            deadline - Duration::from_millis(1)
        ));
        assert!(advance_animation_if_due(
            &mut model,
            &mut next_frame,
            deadline
        ));
        assert_eq!(model.preview_transition, 400);
    }

    #[derive(Clone)]
    struct Source;

    impl CatalogSource for Source {
        fn refresh_local(&mut self) -> Result<CatalogSnapshot, String> {
            Ok(CatalogSnapshot::default())
        }

        fn refresh_remote(&mut self) -> Result<CatalogSnapshot, String> {
            Ok(CatalogSnapshot::default())
        }

        fn preview(&mut self, _key: &str) -> Result<Preview, String> {
            Ok(Preview::default())
        }
    }

    #[test]
    fn worker_returns_catalog_results_without_blocking_the_ui_thread() {
        let worker = SourceWorker::new(Source);
        worker.send(WorkerRequest::Refresh).unwrap();
        let response = loop {
            if let Some(response) = worker.try_response().unwrap() {
                break response;
            }
            thread::yield_now();
        };
        assert!(matches!(
            response,
            WorkerResponse::Refreshed {
                complete: false,
                result: Ok(_)
            }
        ));
    }

    #[derive(Clone)]
    struct RemoteBlockingSource {
        remote_gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl CatalogSource for RemoteBlockingSource {
        fn refresh_local(&mut self) -> Result<CatalogSnapshot, String> {
            Ok(CatalogSnapshot::default())
        }

        fn refresh_remote(&mut self) -> Result<CatalogSnapshot, String> {
            let (lock, ready) = &*self.remote_gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = ready.wait(released).unwrap();
            }
            Ok(CatalogSnapshot::default())
        }

        fn preview(&mut self, _key: &str) -> Result<Preview, String> {
            Ok(Preview::default())
        }
    }

    fn release(gate: &Arc<(Mutex<bool>, Condvar)>) {
        let (lock, ready) = &**gate;
        *lock.lock().unwrap() = true;
        ready.notify_all();
    }

    fn barrier(worker: &SourceWorker) {
        let (sender, receiver) = mpsc::channel();
        worker.send(WorkerRequest::Barrier(sender)).unwrap();
        receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn local_catalog_arrives_while_remote_discovery_is_still_blocked() {
        let remote_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = SourceWorker::new(RemoteBlockingSource {
            remote_gate: remote_gate.clone(),
        });
        worker.send(WorkerRequest::Refresh).unwrap();
        let response = loop {
            if let Some(response) = worker.try_response().unwrap() {
                break response;
            }
            thread::yield_now();
        };
        assert!(matches!(
            response,
            WorkerResponse::Refreshed {
                complete: false,
                result: Ok(_)
            }
        ));
        release(&remote_gate);
    }

    #[derive(Clone)]
    struct PreviewBlockingSource {
        started: Sender<String>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        blocked: Arc<Vec<String>>,
    }

    impl CatalogSource for PreviewBlockingSource {
        fn refresh_local(&mut self) -> Result<CatalogSnapshot, String> {
            Ok(CatalogSnapshot::default())
        }

        fn refresh_remote(&mut self) -> Result<CatalogSnapshot, String> {
            Ok(CatalogSnapshot::default())
        }

        fn preview(&mut self, key: &str) -> Result<Preview, String> {
            self.started.send(key.to_string()).unwrap();
            if self.blocked.iter().any(|blocked| blocked == key) {
                let (lock, ready) = &*self.gate;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = ready.wait(released).unwrap();
                }
            }
            Ok(Preview::default())
        }
    }

    #[test]
    fn a_new_preview_starts_while_the_previous_remote_read_is_blocked() {
        let (started_tx, started_rx) = mpsc::channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = SourceWorker::new(PreviewBlockingSource {
            started: started_tx,
            gate: gate.clone(),
            blocked: Arc::new(vec!["one".into()]),
        });
        worker.send(WorkerRequest::Preview("one".into())).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "one"
        );
        worker.send(WorkerRequest::Preview("two".into())).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "two"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(WorkerResponse::Previewed { key, .. }) = worker.try_response().unwrap()
                && key == "two"
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        release(&gate);
        let deadline = std::time::Instant::now() + Duration::from_millis(100);
        while std::time::Instant::now() < deadline {
            if let Some(WorkerResponse::Previewed { key, .. }) = worker.try_response().unwrap() {
                assert_ne!(key, "one", "the stale preview must be ignored");
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn preview_jobs_are_bounded_and_pending_requests_coalesce_to_the_newest() {
        let (started_tx, started_rx) = mpsc::channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = SourceWorker::new(PreviewBlockingSource {
            started: started_tx,
            gate: gate.clone(),
            blocked: Arc::new(vec!["one".into(), "two".into()]),
        });
        worker.send(WorkerRequest::Preview("one".into())).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "one"
        );
        worker.send(WorkerRequest::Preview("two".into())).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "two"
        );
        worker
            .send(WorkerRequest::Preview("intermediate".into()))
            .unwrap();
        worker
            .send(WorkerRequest::Preview("newest".into()))
            .unwrap();
        barrier(&worker);
        assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
        release(&gate);

        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "newest"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(WorkerResponse::Previewed { key, .. }) = worker.try_response().unwrap()
                && key == "newest"
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(started_rx.try_recv().is_err());
    }

    #[derive(Clone)]
    struct CountingRemoteSource {
        remote_started: Sender<()>,
        remote_gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl CatalogSource for CountingRemoteSource {
        fn refresh_local(&mut self) -> Result<CatalogSnapshot, String> {
            Ok(CatalogSnapshot::default())
        }

        fn refresh_remote(&mut self) -> Result<CatalogSnapshot, String> {
            self.remote_started.send(()).unwrap();
            let (lock, ready) = &*self.remote_gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = ready.wait(released).unwrap();
            }
            Ok(CatalogSnapshot::default())
        }

        fn preview(&mut self, _key: &str) -> Result<Preview, String> {
            Ok(Preview::default())
        }
    }

    #[test]
    fn repeated_refreshes_keep_one_remote_job_active_and_one_pending() {
        let (started_tx, started_rx) = mpsc::channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = SourceWorker::new(CountingRemoteSource {
            remote_started: started_tx,
            remote_gate: gate.clone(),
        });
        worker.send(WorkerRequest::Refresh).unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        for _ in 0..20 {
            worker.send(WorkerRequest::Refresh).unwrap();
        }
        barrier(&worker);
        assert!(started_rx.try_recv().is_err());

        release(&gate);
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
