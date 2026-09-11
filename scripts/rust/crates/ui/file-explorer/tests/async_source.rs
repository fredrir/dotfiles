use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use ui_file_explorer::{Cancellation, Directory, DirectoryStatus, Explorer, FileSource, Outcome};
use ui_terminal::{Event, Key, Surface};
use ui_theme::Style;

#[derive(Clone)]
struct BlockedSource {
    entered: mpsc::SyncSender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
    stopped: mpsc::SyncSender<bool>,
}

impl FileSource for BlockedSource {
    type Location = usize;
    type Error = std::io::Error;
    fn read_directory(&self, location: &usize) -> Result<Directory<usize>, Self::Error> {
        Ok(Directory {
            location: *location,
            parent: None,
            label: "loaded".into(),
            entries: Vec::new(),
            status: DirectoryStatus::Present,
        })
    }
    fn read_directory_cancellable(
        &self,
        location: &usize,
        cancel: &Cancellation,
    ) -> Result<Directory<usize>, Self::Error> {
        self.entered.send(()).unwrap();
        let _ = self
            .release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3));
        self.stopped.send(cancel.is_cancelled()).unwrap();
        self.read_directory(location)
    }
}

struct CancellingTerminal {
    entered: mpsc::Receiver<()>,
    clears: usize,
    frames: Vec<Vec<String>>,
}

impl Surface for CancellingTerminal {
    type Error = std::io::Error;
    fn size(&self) -> (usize, usize) {
        (40, 8)
    }
    fn draw(&mut self, lines: &[String]) -> Result<(), Self::Error> {
        self.frames.push(lines.to_vec());
        Ok(())
    }
    fn event(&mut self) -> Result<Event, Self::Error> {
        Ok(Event::Key(Key::Escape))
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.clears += 1;
        Ok(())
    }
    fn poll_event(&mut self, _: Duration) -> Result<Option<Event>, Self::Error> {
        self.entered
            .recv_timeout(Duration::from_secs(2))
            .expect("source started");
        Ok(Some(Event::Key(Key::Escape)))
    }
}

#[test]
fn cancellation_restores_ui_without_waiting_for_the_source() {
    let (entered, started) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let (stopped, observed) = mpsc::sync_channel(1);
    let source = BlockedSource {
        entered,
        release: Arc::new(Mutex::new(released)),
        stopped,
    };
    let mut terminal = CancellingTerminal {
        entered: started,
        clears: 0,
        frames: Vec::new(),
    };
    let style = Style::plain();
    assert_eq!(
        Explorer::new(source, 0, &style)
            .into_async()
            .run_in(&mut terminal)
            .unwrap(),
        Outcome::Cancelled
    );
    assert_eq!(terminal.clears, 1);
    assert!(terminal.frames[0][0].contains("Loading"));
    release.send(()).unwrap();
    assert!(observed.recv_timeout(Duration::from_secs(2)).unwrap());
}

#[derive(Clone)]
struct TreeSource {
    calls: Arc<Mutex<Vec<usize>>>,
}

impl FileSource for TreeSource {
    type Location = usize;
    type Error = std::io::Error;
    fn read_directory(&self, location: &usize) -> Result<Directory<usize>, Self::Error> {
        self.calls.lock().unwrap().push(*location);
        let root = *location == 0;
        Ok(Directory {
            location: *location,
            parent: (!root).then_some(0),
            label: if root {
                "root-directory"
            } else {
                "child-directory"
            }
            .into(),
            entries: vec![ui_file_explorer::Entry {
                location: if root { 1 } else { 2 },
                name: if root { "child" } else { "selected-file" }.into(),
                kind: if root {
                    ui_file_explorer::EntryKind::Directory
                } else {
                    ui_file_explorer::EntryKind::File
                },
            }],
            status: DirectoryStatus::Present,
        })
    }
}

struct FrameDrivenTerminal {
    frames: Vec<Vec<String>>,
    step: usize,
    deadline: Instant,
    clears: usize,
}

impl Surface for FrameDrivenTerminal {
    type Error = std::io::Error;
    fn size(&self) -> (usize, usize) {
        (60, 12)
    }
    fn draw(&mut self, lines: &[String]) -> Result<(), Self::Error> {
        self.frames.push(lines.to_vec());
        Ok(())
    }
    fn event(&mut self) -> Result<Event, Self::Error> {
        unreachable!("async polling")
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.clears += 1;
        Ok(())
    }
    fn poll_event(&mut self, _: Duration) -> Result<Option<Event>, Self::Error> {
        assert!(
            Instant::now() < self.deadline,
            "source result did not arrive"
        );
        let expected = if self.step == 0 {
            "root-directory"
        } else {
            "child-directory"
        };
        if self
            .frames
            .last()
            .is_some_and(|lines| lines.iter().any(|line| line.contains(expected)))
        {
            let key = if self.step == 0 {
                Key::Right
            } else {
                Key::Enter
            };
            self.step += 1;
            Ok(Some(Event::Key(key)))
        } else {
            std::thread::yield_now();
            Ok(None)
        }
    }
}

#[test]
fn background_navigation_accepts_the_loaded_entry_and_preserves_identity() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let source = TreeSource {
        calls: Arc::clone(&calls),
    };
    let mut terminal = FrameDrivenTerminal {
        frames: Vec::new(),
        step: 0,
        deadline: Instant::now() + Duration::from_secs(3),
        clears: 0,
    };
    let style = Style::plain();
    let Outcome::Selected(selection) = Explorer::new(source, 0, &style)
        .into_async()
        .run_in(&mut terminal)
        .unwrap()
    else {
        panic!("selection expected");
    };
    assert_eq!(selection.location, 2);
    assert_eq!(selection.label, "selected-file");
    assert_eq!(*calls.lock().unwrap(), [0, 1]);
    assert_eq!(terminal.clears, 1);
}
