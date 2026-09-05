use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

#[allow(unsafe_code)]
pub fn open_pty(rows: u16, cols: u16) -> (File, File, libc::termios) {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let mut state = MaybeUninit::<libc::termios>::uninit();
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut size,
            )
        },
        0
    );
    assert_eq!(unsafe { libc::tcgetattr(slave, state.as_mut_ptr()) }, 0);
    let flags = unsafe { libc::fcntl(master, libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe { libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    (
        unsafe { File::from_raw_fd(master) },
        unsafe { File::from_raw_fd(slave) },
        unsafe { state.assume_init() },
    )
}

#[allow(unsafe_code)]
pub fn terminal_state(fd: RawFd) -> libc::termios {
    let mut state = MaybeUninit::<libc::termios>::uninit();
    assert_eq!(unsafe { libc::tcgetattr(fd, state.as_mut_ptr()) }, 0);
    unsafe { state.assume_init() }
}

#[allow(unsafe_code)]
pub fn read_available(master: &File, output: &mut Vec<u8>, timeout_ms: libc::c_int) {
    let mut descriptor = libc::pollfd {
        fd: master.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let _ = unsafe { libc::poll(&raw mut descriptor, 1, timeout_ms) };
    let mut reader = master;
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => output.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
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
