use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use super::*;

fn summary() -> Summary {
    Summary {
        profile: "test".to_string(),
        peer: None,
        remote_changed: None,
        checked: 1,
        changed: 0,
        links: 0,
        merges: 0,
        secrets: 0,
        generated: 0,
        dry_run: false,
        elapsed: Duration::ZERO,
    }
}

fn merge_prompt() -> Prompt {
    Prompt::Merge {
        path: PathBuf::from("/tmp/settings.json"),
        key: "font".to_string(),
        repo: "mono".to_string(),
        live: "sans".to_string(),
    }
}

#[test]
fn sync_wire_round_trips_merge_and_target_with_exact_ids() {
    let (event_sender, event_receiver) = crossbeam_channel::bounded(8);
    let (client, server) = crate::decision::channel();
    let worker = std::thread::spawn(move || {
        event_sender
            .send(Event::Started {
                profile: "test".to_string(),
                dry_run: false,
                peer: None,
            })
            .unwrap();
        assert_eq!(client.choose(merge_prompt()), Ok(Choice::Live));
        assert_eq!(
            client.choose(Prompt::MergeTarget {
                path: PathBuf::from("/tmp/settings.json"),
                key: "font".to_string(),
                targets: vec!["shared".to_string(), "macos".to_string()],
                default: 1,
            }),
            Ok(Choice::Target(1))
        );
        Ok(summary())
    });
    let responses = [
        Message::DecisionResponse {
            id: 1,
            choice: Choice::Live,
        },
        Message::DecisionResponse {
            id: 2,
            choice: Choice::Target(1),
        },
    ]
    .iter()
    .map(protocol::encode)
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
    .join("\n")
        + "\n";
    let mut input = Cursor::new(responses);
    let mut output = Vec::new();

    assert_eq!(
        bridge_worker(&mut input, &mut output, &event_receiver, &server, worker),
        Ok(())
    );

    let messages = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(protocol::decode)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let requests = messages
        .iter()
        .filter(|message| matches!(message, Message::DecisionRequest { .. }))
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        requests[0],
        Message::DecisionRequest {
            id: 1,
            prompt: Prompt::Merge { .. }
        }
    ));
    assert!(matches!(
        requests[1],
        Message::DecisionRequest {
            id: 2,
            prompt: Prompt::MergeTarget { .. }
        }
    ));
}

#[test]
fn sync_wire_rejects_eof_malformed_or_mismatched_response() {
    for response in [
        String::new(),
        "not-json\n".to_string(),
        protocol::encode(&Message::DecisionResponse {
            id: 99,
            choice: Choice::Repo,
        })
        .unwrap()
            + "\n",
    ] {
        let (_client, server) = crate::decision::channel();
        let request = Request {
            id: 1,
            prompt: merge_prompt(),
        };
        let mut input = Cursor::new(response);
        let mut output = Vec::new();

        assert!(relay_decision(&mut input, &mut output, &server, &request).is_err());
    }
}
