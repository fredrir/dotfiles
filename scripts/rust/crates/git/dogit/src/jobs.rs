use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};

type Work<T> = Box<dyn FnOnce(Arc<AtomicBool>) -> T + Send>;

struct Job<T> {
    generation: u64,
    cancel: Arc<AtomicBool>,
    work: Work<T>,
    wake: Option<ui_terminal::InputWake>,
}

pub struct Worker<T> {
    sender: Sender<Job<T>>,
    waiting: Receiver<Job<T>>,
    output: Receiver<(u64, T)>,
    cancel: Arc<AtomicBool>,
    generation: u64,
    wake: Option<ui_terminal::InputWake>,
}

impl<T: Send + 'static> Default for Worker<T> {
    fn default() -> Self {
        let (sender, waiting) = bounded::<Job<T>>(1);
        let (results, output) = unbounded();
        let incoming = waiting.clone();
        std::thread::spawn(move || {
            while let Ok(job) = incoming.recv() {
                if job.cancel.load(Ordering::Acquire) {
                    continue;
                }
                let value = (job.work)(Arc::clone(&job.cancel));
                if !job.cancel.load(Ordering::Acquire) {
                    if results.send((job.generation, value)).is_err() {
                        break;
                    }
                    if let Some(wake) = job.wake {
                        wake.wake();
                    }
                }
            }
        });
        Self {
            sender,
            waiting,
            output,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
            wake: None,
        }
    }
}

impl<T: Send + 'static> Worker<T> {
    pub fn set_waker(&mut self, wake: ui_terminal::InputWake) {
        self.wake = Some(wake);
    }

    pub fn submit(&mut self, work: impl FnOnce(Arc<AtomicBool>) -> T + Send + 'static) {
        self.invalidate();
        let _ = self.waiting.try_recv();
        let job = Job {
            generation: self.generation,
            cancel: Arc::clone(&self.cancel),
            work: Box::new(work),
            wake: self.wake.clone(),
        };
        let _ = self.sender.try_send(job);
    }

    pub fn invalidate(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn take(&self) -> Option<T> {
        let mut current = None;
        while let Ok((generation, value)) = self.output.try_recv() {
            if generation == self.generation {
                current = Some(value);
            }
        }
        current
    }
}

impl<T> Drop for Worker<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn late_result_cannot_replace_new_selection() {
        let mut worker = Worker::default();
        let (started, observed) = bounded(1);
        let (release, blocked) = bounded(1);
        worker.submit(move |_| {
            started.send(()).unwrap();
            blocked.recv().unwrap();
            "old"
        });
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.submit(|_| "new");
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = worker.take() {
                assert_eq!(result, "new");
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
