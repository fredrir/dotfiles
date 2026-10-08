use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

type Answer = Result<String, String>;

struct Job {
    deadline: Instant,
    answer: Sender<Answer>,
}

pub struct Reader {
    jobs: SyncSender<Job>,
    busy: Arc<Mutex<Option<Instant>>>,
}

impl Reader {
    pub fn spawn(read: impl Fn() -> Answer + Send + 'static, queue: usize) -> Reader {
        let (jobs, inbox) = mpsc::sync_channel(queue);
        let busy = Arc::new(Mutex::new(None));
        let marker = Arc::clone(&busy);
        let _ = thread::Builder::new()
            .name("pasteboard".into())
            .spawn(move || serve(&read, &inbox, &marker));
        Reader { jobs, busy }
    }

    pub fn ask(&self, timeout: Duration) -> Answer {
        let stuck = self
            .busy
            .lock()
            .map(|since| since.is_some_and(|since| since.elapsed() >= timeout))
            .unwrap_or(true);
        if stuck {
            return Err("clipboard busy".into());
        }
        let (answer, reply) = mpsc::channel();
        let deadline = Instant::now() + timeout;
        self.jobs
            .try_send(Job { deadline, answer })
            .map_err(|_| "clipboard busy".to_string())?;
        reply
            .recv_timeout(timeout)
            .unwrap_or_else(|_| Err("clipboard timed out".into()))
    }
}

fn serve(read: &dyn Fn() -> Answer, inbox: &Receiver<Job>, busy: &Mutex<Option<Instant>>) {
    for job in inbox {
        if Instant::now() >= job.deadline {
            continue;
        }
        mark(busy, Some(Instant::now()));
        let answer = panic::catch_unwind(AssertUnwindSafe(read))
            .unwrap_or_else(|_| Err("clipboard failed".into()));
        mark(busy, None);
        let _ = job.answer.send(answer);
    }
}

fn mark(busy: &Mutex<Option<Instant>>, value: Option<Instant>) {
    if let Ok(mut since) = busy.lock() {
        *since = value;
    }
}

#[cfg(test)]
#[path = "../../tests/unit/native/reader_tests.rs"]
mod tests;
