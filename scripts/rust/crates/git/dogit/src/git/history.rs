use std::io::{self, BufRead, Seek, Write};
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use crossbeam_channel::{Receiver, SendTimeoutError, Sender, bounded};

use super::{COMMIT_FORMAT, Commit, GitError, Repo, parse_commits};

impl Repo {
    pub fn count_history(&self, refs: &[(String, String)]) -> Result<usize, GitError> {
        if refs.is_empty() {
            return Ok(0);
        }
        let mut input = tempfile::tempfile()?;
        for (_, oid) in refs {
            writeln!(input, "{oid}")?;
        }
        input.rewind()?;
        let mut spec = super::CommandSpec::new("Count history", ["rev-list", "--count", "--stdin"]);
        spec.mutates = false;
        let mut command = self.command(&spec);
        command.stdin(Stdio::from(input));
        super::text(&super::capture(&mut command, self.cancellation.as_deref())?)
            .trim()
            .parse()
            .map_err(|_| GitError::new("Malformed commit count"))
    }
}

pub struct CommitStream {
    pages: Receiver<Result<Vec<Commit>, GitError>>,
    cancel: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    pub page_size: usize,
}

impl CommitStream {
    pub fn try_next(&self) -> Option<Result<Vec<Commit>, GitError>> {
        self.pages.try_recv().ok()
    }

    pub fn next_page(&self) -> Option<Result<Vec<Commit>, GitError>> {
        self.pages.recv().ok()
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire) && self.pages.is_empty()
    }

    pub(super) fn start(
        repo: &Repo,
        revisions: Vec<String>,
        ordered: bool,
        page_size: usize,
        wake: Option<ui_terminal::InputWake>,
    ) -> Self {
        let (sender, pages) = bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let stream = Self {
            pages,
            cancel: Arc::clone(&cancel),
            finished: Arc::clone(&finished),
            page_size: page_size.max(1),
        };
        let page_size = stream.page_size;
        let repo = repo.clone();
        std::thread::spawn(move || {
            let result = read(
                &repo,
                revisions,
                ordered,
                page_size,
                &cancel,
                &sender,
                wake.as_ref(),
            );
            if let Err(error) = result {
                let _ = send(&sender, Err(error), &cancel, wake.as_ref());
            }
            finished.store(true, Ordering::Release);
            if let Some(wake) = wake {
                wake.wake();
            }
        });
        stream
    }
}

impl Drop for CommitStream {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn send(
    sender: &Sender<Result<Vec<Commit>, GitError>>,
    mut page: Result<Vec<Commit>, GitError>,
    cancel: &AtomicBool,
    wake: Option<&ui_terminal::InputWake>,
) -> io::Result<()> {
    loop {
        crate::process::check_cancel(Some(cancel))?;
        match sender.send_timeout(page, Duration::from_millis(20)) {
            Ok(()) => {
                if let Some(wake) = wake {
                    wake.wake();
                }
                return Ok(());
            }
            Err(SendTimeoutError::Timeout(value)) => page = value,
            Err(SendTimeoutError::Disconnected(_)) => {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "History closed"));
            }
        }
    }
}

fn read(
    repo: &Repo,
    revisions: Vec<String>,
    ordered: bool,
    page_size: usize,
    cancel: &AtomicBool,
    sender: &Sender<Result<Vec<Commit>, GitError>>,
    wake: Option<&ui_terminal::InputWake>,
) -> Result<(), GitError> {
    if revisions.is_empty() {
        return Ok(());
    }
    let mut input = tempfile::tempfile()?;
    for revision in revisions {
        writeln!(input, "{revision}")?;
    }
    input.rewind()?;
    let mut spec = super::CommandSpec::new(
        "Read history",
        ["log", "--no-show-signature", "-z", COMMIT_FORMAT, "--stdin"],
    );
    spec.mutates = false;
    if ordered {
        spec.args
            .extend(["--topo-order".into(), "--date-order".into()]);
    }
    let mut command = repo.command(&spec);
    command.stdin(Stdio::from(input));
    let output = crate::process::stream(&mut command, Some(cancel), None, |mut reader| {
        let mut page = Vec::with_capacity(page_size);
        let mut record = Vec::new();
        let batch_bytes = reader.capacity();
        let mut first_page = true;
        let mut page_bytes = 0;
        loop {
            crate::process::check_cancel(Some(cancel))?;
            record.clear();
            for field in 0..7 {
                if reader.read_until(0, &mut record)? == 0 {
                    if field != 0 {
                        return Err(io::Error::other("Incomplete history record"));
                    }
                    if !page.is_empty() {
                        send(sender, Ok(page), cancel, wake)?;
                    }
                    return Ok(());
                }
                if record.last() != Some(&0) {
                    return Err(io::Error::other("Incomplete history field"));
                }
            }
            let commit = parse_commits(&record)
                .map_err(io::Error::other)?
                .pop()
                .ok_or_else(|| io::Error::other("Missing history record"))?;
            page.push(commit);
            page_bytes += record.len();
            if (first_page && page.len() >= page_size) || page_bytes >= batch_bytes {
                send(
                    sender,
                    Ok(std::mem::replace(&mut page, Vec::with_capacity(page_size))),
                    cancel,
                    wake,
                )?;
                first_page = false;
                page_bytes = 0;
            }
        }
    })?;
    if !output.status.success() {
        return Err(GitError {
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            status: output.status.code(),
        });
    }
    Ok(())
}
