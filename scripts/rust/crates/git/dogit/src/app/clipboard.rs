use super::*;

impl App {
    pub(super) fn copy_text(&mut self, value: String) {
        self.clipboard_worker
            .get_or_insert_with(|| {
                let mut worker = Worker::default();
                if let Some(waker) = &self.waker {
                    worker.set_waker(waker.clone());
                }
                worker
            })
            .submit(move |cancel| {
                let mut input = tempfile::tempfile().map_err(|error| error.to_string())?;
                input
                    .write_all(value.as_bytes())
                    .map_err(|error| error.to_string())?;
                use std::io::Seek;
                input.rewind().map_err(|error| error.to_string())?;
                let mut command = Command::new("dclip");
                command.stdin(Stdio::from(input));
                let output = crate::process::capture(
                    &mut command,
                    Some(&cancel),
                    Some(Duration::from_secs(5)),
                );
                if output.is_ok_and(|output| output.status.success()) {
                    Ok(None)
                } else {
                    Ok(Some(value))
                }
            });
    }
}
