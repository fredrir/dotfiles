use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

pub fn open_pty(rows: u16, cols: u16) -> (File, File, libc::termios) {
    let size = nix::pty::Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pair = nix::pty::openpty(Some(&size), None).expect("the pty opens");
    let state = terminal_state(&pair.slave);
    let flags = OFlag::from_bits_retain(fcntl(&pair.master, FcntlArg::F_GETFL).unwrap());
    fcntl(&pair.master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
    (File::from(pair.master), File::from(pair.slave), state)
}

pub fn terminal_state(fd: impl AsFd) -> libc::termios {
    nix::sys::termios::tcgetattr(fd)
        .expect("terminal state")
        .into()
}

pub fn read_available(master: &File, output: &mut Vec<u8>, timeout_ms: libc::c_int) {
    let mut descriptors = [PollFd::new(master.as_fd(), PollFlags::POLLIN)];
    let timeout = PollTimeout::try_from(timeout_ms).unwrap_or(PollTimeout::NONE);
    let _ = poll(&mut descriptors, timeout);
    let mut reader = master;
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => output.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => panic!("read pty: {error}"),
        }
    }
}

pub fn last_cursor_column(output: &[u8]) -> Option<u16> {
    output
        .windows(2)
        .enumerate()
        .filter(|(_, window)| *window == b"\x1b[")
        .filter_map(|(start, _)| {
            let tail = &output[start + 2..];
            let end = tail.iter().position(|byte| *byte == b'H')?;
            let parameters = std::str::from_utf8(&tail[..end]).ok()?;
            parameters.split(';').next_back()?.parse().ok()
        })
        .next_back()
}

pub fn reply_to_cursor_queries(master: &File, output: &[u8], replied: &mut usize) {
    let queries = output
        .windows(4)
        .filter(|window| *window == b"\x1b[6n")
        .count();
    while *replied < queries {
        let mut terminal = master;
        terminal.write_all(b"\x1b[24;1R").unwrap();
        *replied += 1;
    }
}

pub fn stdio(slave: &File) -> (Stdio, Stdio, Stdio) {
    let input = slave.try_clone().expect("the pty clones");
    let output = slave.try_clone().expect("the pty clones");
    let errors = slave.try_clone().expect("the pty clones");
    (Stdio::from(input), Stdio::from(output), Stdio::from(errors))
}

#[allow(unsafe_code)]
pub fn take_controlling_terminal(command: &mut Command) {
    // SAFETY: The child only calls setsid, ioctl and last_os_error between fork
    // and exec. No locks, allocation or borrowed parent resources are involved;
    // Command has already installed the child's stdin at descriptor 0.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
#[path = "../tests/unit/pty_tests.rs"]
mod tests;
