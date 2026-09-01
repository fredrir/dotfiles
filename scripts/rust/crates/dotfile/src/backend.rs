use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

pub fn delegate(arguments: Vec<OsString>) -> ExitCode {
    let backend = path();
    let mut command = Command::new(&backend);
    command.args(arguments);
    command.env("DOTFILE_PROGRAM_NAME", "dotfile");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!("dotfile: cannot run {}: {error}", backend.display());
        ExitCode::FAILURE
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(error) => {
                eprintln!("dotfile: cannot run {}: {error}", backend.display());
                ExitCode::FAILURE
            }
        }
    }
}

pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("DOTFILE_PYTHON") {
        return path.into();
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        let sibling = directory.join("dotfile-py");
        if sibling.is_file() {
            return sibling;
        }
    }
    PathBuf::from("dotfile-py")
}
