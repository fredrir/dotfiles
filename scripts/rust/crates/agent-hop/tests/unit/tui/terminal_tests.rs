use std::sync::{Arc, Condvar, Mutex};

use super::*;

#[test]
fn preview_selection_extracts_only_pane_cells_and_handles_wide_text() {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 24, 2));
    buffer.set_string(0, 0, "LIST   preview one", ratatui::style::Style::default());
    buffer.set_string(0, 1, "OTHER  日本 test", ratatui::style::Style::default());
    let area = Rect::new(7, 0, 12, 2);
    let selection = super::super::model::TextSelection::between(
        ratatui::layout::Position::new(7, 0),
        ratatui::layout::Position::new(17, 1),
    );

    let text = selected_text_from_buffer(&buffer, selection, area).unwrap();
    assert_eq!(text, "preview one\n日本 test");
    assert!(!text.contains("LIST"));
    assert!(!text.contains("OTHER"));
}

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
