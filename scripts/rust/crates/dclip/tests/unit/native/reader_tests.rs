use std::sync::mpsc;

use super::*;

const TIMEOUT: Duration = Duration::from_millis(50);

fn hanging() -> (Reader, mpsc::SyncSender<()>) {
    let (release, gate) = mpsc::sync_channel::<()>(0);
    let gate = Mutex::new(gate);
    let reader = Reader::spawn(
        move || {
            let _ = gate.lock().unwrap().recv();
            Ok("late".into())
        },
        2,
    );
    (reader, release)
}

#[test]
fn a_read_answers_through_the_one_reader() {
    let reader = Reader::spawn(|| Ok("text".into()), 2);
    for _ in 0..3 {
        assert_eq!(reader.ask(TIMEOUT), Ok("text".into()));
    }
}

#[test]
fn a_hung_read_times_out_and_later_requests_fail_fast() {
    let (reader, release) = hanging();
    assert_eq!(reader.ask(TIMEOUT), Err("clipboard timed out".into()));
    let started = Instant::now();
    assert_eq!(reader.ask(TIMEOUT), Err("clipboard busy".into()));
    assert!(started.elapsed() < TIMEOUT, "{:?}", started.elapsed());
    drop(release);
}

#[test]
fn the_reader_recovers_once_the_hung_read_returns() {
    let (reader, release) = hanging();
    assert!(reader.ask(TIMEOUT).is_err());
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(2);
    while reader.ask(TIMEOUT).is_err() {
        assert!(Instant::now() < deadline, "the reader never recovered");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(reader.ask(TIMEOUT), Ok("late".into()));
}

#[test]
fn a_panicking_read_is_an_error_not_a_dead_reader() {
    let reader = Reader::spawn(|| panic!("pasteboard"), 2);
    assert_eq!(reader.ask(TIMEOUT), Err("clipboard failed".into()));
    assert_eq!(reader.ask(TIMEOUT), Err("clipboard failed".into()));
}
