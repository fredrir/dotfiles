use std::path::PathBuf;

use dotfile_cli::decision::{Choice, Prompt};

#[test]
fn decision_channel_round_trips_a_merge_choice() {
    let (client, server) = dotfile_cli::decision::channel();
    let worker = std::thread::spawn(move || {
        client.choose(Prompt::Merge {
            path: PathBuf::from("settings.json"),
            key: "editor.fontSize".to_string(),
            repo: "12".to_string(),
            live: "14".to_string(),
        })
    });
    let request = loop {
        if let Some(request) = server.try_recv() {
            break request;
        }
        std::thread::yield_now();
    };
    server.respond(&request, Choice::Live).unwrap();
    assert_eq!(worker.join().unwrap().unwrap(), Choice::Live);
}

#[test]
fn noninteractive_defaults_are_non_destructive() {
    assert_eq!(
        Prompt::Merge {
            path: PathBuf::new(),
            key: String::new(),
            repo: String::new(),
            live: String::new(),
        }
        .safe_default(),
        Choice::Skip
    );
    assert_eq!(
        Prompt::MergeTarget {
            path: PathBuf::new(),
            key: String::new(),
            targets: vec!["shared".to_string(), "macos".to_string()],
            default: 1,
        }
        .safe_default(),
        Choice::Cancel
    );
    assert_eq!(
        Prompt::RemoteChanges {
            host: String::new(),
            changes: Vec::new(),
        }
        .safe_default(),
        Choice::Cancel
    );
}
