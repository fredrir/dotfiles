use super::*;

fn sample() -> State {
    State {
        pid: 42,
        peer: "archie".into(),
        route: Some("cable".into()),
        connected: true,
        error: None,
        services: vec![Entry {
            port: 5173,
            process: "bun".into(),
            alias: Status::Active,
            mirror: Status::Busy(Some("node".into())),
        }],
    }
}

#[test]
fn statuses_serialize_with_a_tag_and_their_detail() {
    let text = serde_json::to_string(&sample().services[0]).unwrap();
    assert!(text.contains(r#""alias":{"state":"active"}"#), "{text}");
    assert!(
        text.contains(r#""mirror":{"state":"busy","detail":"node"}"#),
        "{text}"
    );
}

#[test]
fn state_survives_a_write_and_read() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths {
        state: directory.path().join("hport/state.json"),
        socket: directory.path().join("hport/master.sock"),
    };
    paths.prepare().unwrap();
    write(&paths, &sample()).unwrap();
    assert_eq!(read(&paths).unwrap(), Some(sample()));
}

#[test]
fn a_missing_state_file_is_not_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths {
        state: directory.path().join("state.json"),
        socket: directory.path().join("master.sock"),
    };
    assert_eq!(read(&paths).unwrap(), None);
}

#[test]
fn an_idle_state_names_the_peer_and_this_process() {
    let idle = State::idle(hostkit::Host::Macie);
    assert_eq!(idle.peer, "macie");
    assert_eq!(idle.pid, std::process::id());
    assert!(!idle.connected);
}
