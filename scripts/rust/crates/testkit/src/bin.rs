use std::ffi::OsStr;
use std::fmt;
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitStatus, Output, Stdio};

pub struct Bin {
    command: Command,
    input: Option<String>,
}

impl Bin {
    pub fn new(program: impl AsRef<OsStr>) -> Bin {
        Bin {
            command: Command::new(program),
            input: None,
        }
    }

    pub fn arg(mut self, argument: impl AsRef<OsStr>) -> Bin {
        self.command.arg(argument);
        self
    }

    pub fn args<I, S>(mut self, arguments: I) -> Bin
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command.args(arguments);
        self
    }

    pub fn env(mut self, name: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Bin {
        self.command.env(name, value);
        self
    }

    pub fn envs<I, K, V>(mut self, variables: I) -> Bin
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.command.envs(variables);
        self
    }

    pub fn env_remove(mut self, name: impl AsRef<OsStr>) -> Bin {
        self.command.env_remove(name);
        self
    }

    pub fn current_dir(mut self, directory: impl AsRef<Path>) -> Bin {
        self.command.current_dir(directory);
        self
    }

    pub fn stdin(mut self, input: &str) -> Bin {
        self.input = Some(input.to_string());
        self
    }

    pub fn plain(self) -> Bin {
        self.env("NO_COLOR", "1").env("COLUMNS", "80")
    }

    pub fn output(mut self) -> Output {
        let Some(input) = self.input.take() else {
            return self.command.output().expect("the binary runs");
        };
        let mut child = self
            .command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary runs");
        child
            .stdin
            .take()
            .expect("stdin is a pipe")
            .write_all(input.as_bytes())
            .expect("the input is read");
        child.wait_with_output().expect("the binary finishes")
    }

    pub fn run(self) -> Ran {
        Ran::new(self.output())
    }
}

pub struct Ran {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub output: Output,
}

impl Ran {
    fn new(output: Output) -> Ran {
        Ran {
            status: output.status,
            stdout: stdout(&output),
            stderr: stderr(&output),
            output,
        }
    }

    pub fn code(&self) -> Option<i32> {
        self.status.code()
    }

    pub fn success(&self) -> bool {
        self.status.success()
    }
}

impl fmt::Debug for Ran {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ran")
            .field("status", &self.status)
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .finish()
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[cfg(test)]
#[path = "../tests/unit/bin_tests.rs"]
mod tests;
