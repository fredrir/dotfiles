use super::*;

impl App {
    pub(super) fn copy_text(&mut self, value: String) {
        self.clipboard_worker
            .get_or_insert_with(Worker::default)
            .submit(move |_| {
                let mut input = tempfile::tempfile().map_err(|error| error.to_string())?;
                input
                    .write_all(value.as_bytes())
                    .map_err(|error| error.to_string())?;
                use std::io::Seek;
                input.rewind().map_err(|error| error.to_string())?;
                let mut command = Command::new("dclip");
                command.stdin(Stdio::from(input));
                let output = hostkit::process::output(
                    &mut command,
                    hostkit::process::CaptureLimits::default(),
                    Duration::from_secs(5),
                )
                .map_err(|error| error.to_string())?;
                if output.status.success() {
                    Ok("Copied".into())
                } else {
                    Err("Clipboard is unavailable".into())
                }
            });
    }
}
