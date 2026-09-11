#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs::File;
use std::io::Write;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use testkit::pty::{
    open_pty, read_available, reply_to_cursor_queries, stdio, take_controlling_terminal,
    terminal_state,
};

#[path = "support/theme.rs"]
mod support;

struct Gallery {
    child: Child,
    master: File,
    before: libc::termios,
    output: Vec<u8>,
    cursor_replies: usize,
    _repository: tempfile::TempDir,
}

impl Gallery {
    fn open(term: &str) -> Self {
        let repository = support::repository();
        let (master, slave, before) = open_pty(24, 120);
        let (input, output, errors) = stdio(&slave);
        let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
        command
            .args(["theme", "gallery", "latte"])
            .current_dir(repository.path())
            .env("DOTFILE_ROOT", repository.path())
            .env("HOME", repository.path().join("home"))
            .env("XDG_CONFIG_HOME", repository.path().join("home/.config"))
            .env("TERM", term)
            .env_remove("COLORTERM")
            .env_remove("CI")
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR")
            .env_remove("CLICOLOR_FORCE")
            .stdin(input)
            .stdout(output)
            .stderr(errors);
        take_controlling_terminal(&mut command);
        let child = command.spawn().unwrap();
        drop(slave);
        Self {
            child,
            master,
            before,
            output: Vec::new(),
            cursor_replies: 0,
            _repository: repository,
        }
    }

    fn read(&mut self) {
        read_available(&self.master, &mut self.output, 50);
        reply_to_cursor_queries(&self.master, &self.output, &mut self.cursor_replies);
    }

    fn wait_for(&mut self, start: usize, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.read();
            if self.output[start..]
                .windows(text.len())
                .any(|window| window == text.as_bytes())
            {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "gallery did not render {text:?}: {:?}",
                String::from_utf8_lossy(&self.output)
            );
        }
    }

    fn send(&mut self, keys: &[u8]) -> usize {
        let start = self.output.len();
        (&self.master).write_all(keys).unwrap();
        start
    }

    fn resize(&mut self, rows: u16, columns: u16) -> usize {
        let start = self.output.len();
        rustix::termios::tcsetwinsize(
            &self.master,
            rustix::termios::Winsize {
                ws_row: rows,
                ws_col: columns,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();
        start
    }

    fn quit(&mut self) {
        self.send(b"q");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.read();
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "{:?}",
                    String::from_utf8_lossy(&self.output)
                );
                break;
            }
            assert!(Instant::now() < deadline, "gallery did not quit");
        }
        read_available(&self.master, &mut self.output, 0);
        let after = terminal_state(&self.master);
        let flags = libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN;
        assert_eq!(after.c_lflag & flags, self.before.c_lflag & flags);
        assert_eq!(after.c_iflag, self.before.c_iflag);
        assert_eq!(after.c_oflag, self.before.c_oflag);
        let last_escape = |escape: &[u8]| {
            self.output
                .windows(escape.len())
                .rposition(|window| window == escape)
                .unwrap_or_else(|| panic!("missing {escape:?}"))
        };
        assert!(last_escape(b"\x1b[?25h") > last_escape(b"\x1b[?25l"));
        assert!(last_escape(b"\x1b[?1049l") > last_escape(b"\x1b[?1049h"));
    }
}

impl Drop for Gallery {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn gallery_picker_diff_resize_and_exit_are_wired_to_the_terminal() {
    let mut gallery = Gallery::open("xterm-256color");
    gallery.wait_for(0, "UI components");
    gallery.wait_for(0, "latte");
    gallery.wait_for(0, "Local files");
    gallery.wait_for(0, "unified");
    assert_eq!(terminal_state(&gallery.master).c_lflag & libc::ICANON, 0);

    let start = gallery.send(b" ");
    gallery.wait_for(start, "[✓]");
    let start = gallery.send(b"/zzzz");
    gallery.wait_for(start, "No matches");
    let start = gallery.send(b"\r\tv");
    gallery.wait_for(start, "side by");

    let start = gallery.resize(32, 42);
    gallery.wait_for(start, "unified");
    for keys in [b"\x1b[F".as_slice(), b"v", b"v"] {
        let start = gallery.send(keys);
        gallery.wait_for(start, "\x1b[?25l");
    }
    let start = gallery.resize(24, 120);
    gallery.wait_for(start, "side by side");
    gallery.quit();
}

#[test]
fn ansi16_gallery_uses_only_basic_terminal_colors() {
    let mut gallery = Gallery::open("vt100");
    gallery.wait_for(0, "Comparison");
    gallery.quit();
    let output = String::from_utf8_lossy(&gallery.output);
    for unsupported in ["38;2;", "48;2;", "38;5;", "48;5;"] {
        assert!(!output.contains(unsupported), "{unsupported}: {output:?}");
    }
    assert!(
        output.split("\x1b[").any(|sequence| {
            sequence
                .split([';', 'm'])
                .next()
                .and_then(|code| code.parse::<u8>().ok())
                .is_some_and(|code| (30..=37).contains(&code))
        }),
        "gallery did not emit basic foreground colors: {output:?}"
    );
}
