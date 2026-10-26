use super::*;

#[test]
fn known_references_round_trip_and_stay_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("known.json");
    let known = vec![
        "op://Dev/a/credential".to_string(),
        "op://Dev/b/credential".to_string(),
    ];
    save_known(&path, &known).unwrap();
    assert_eq!(load_known(&path).into_iter().collect::<Vec<_>>(), known);
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_missing_or_corrupt_known_file_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("known.json");
    assert!(load_known(&path).is_empty());
    fs::write(&path, "not json").unwrap();
    assert!(load_known(&path).is_empty());
}

#[test]
fn a_reload_succeeds_only_when_archie_forgot_too() {
    assert!(matches!(
        reloaded(Ok((2, 2)), Ok(())),
        Response::Reloaded {
            refilled: 2,
            known: 2
        }
    ));
    match reloaded(Ok((2, 2)), Err("archie kept its memory: refused".to_string())) {
        Response::Refused(reason) => {
            assert_eq!(reason, "reloaded 2/2; archie kept its memory: refused");
        }
        _ => panic!("not refused"),
    }
}

#[test]
fn archie_forgets_through_the_reverse_socket() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("archie.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let archie = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        assert!(matches!(
            protocol::receive::<Request>(&stream).unwrap(),
            Request::Forget
        ));
        protocol::send(&stream, &Response::Forgotten).unwrap();
    });
    forget_peer(&socket).unwrap();
    archie.join().unwrap();
}

#[test]
fn an_unreachable_archie_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let reason = forget_peer(&dir.path().join("archie.sock")).unwrap_err();
    assert!(reason.starts_with("archie kept its memory"), "{reason}");
}
