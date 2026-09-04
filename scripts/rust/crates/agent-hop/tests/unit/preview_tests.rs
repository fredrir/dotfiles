use std::fs;
use std::path::PathBuf;

use serde_json::json;

use super::*;
use crate::session::SessionId;

fn session(agent: Agent, content: &str) -> (tempfile::TempDir, Session) {
    let temporary = tempfile::tempdir().unwrap();
    let transcript = temporary.path().join("session.jsonl");
    fs::write(&transcript, content).unwrap();
    (
        temporary,
        Session {
            agent,
            id: SessionId::new("session").unwrap(),
            transcript,
            companion: None,
            workspace: PathBuf::from("/home/user/work"),
        },
    )
}

fn lines(records: impl IntoIterator<Item = Value>) -> String {
    records
        .into_iter()
        .map(|record| format!("{record}\n"))
        .collect()
}

#[test]
fn codex_uses_user_text_and_recent_assistant_text_only() {
    let content = lines([
        json!({"type":"session_meta","payload":{}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>secret</environment_context>"}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":["environments.environment_context"]}}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Build the picker"}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]}}}),
        json!({"type":"response_item","payload":{"type":"reasoning","role":"assistant","content":[{"type":"reasoning","text":"hidden"}]}}),
        json!({"type":"response_item","payload":{"type":"function_call","role":"assistant","content":"tool"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}}),
    ]);
    let (_temporary, session) = session(Agent::Codex, &content);
    let preview = load(&session, PreviewLimits::default()).unwrap();
    assert_eq!(preview.title, "Build the picker");
    assert_eq!(
        preview.messages,
        vec![
            PreviewMessage {
                role: PreviewRole::User,
                text: "Build the picker".into()
            },
            PreviewMessage {
                role: PreviewRole::Assistant,
                text: "Done".into()
            },
        ]
    );
}

#[test]
fn claude_omits_thinking_tools_sidechains_and_extracts_command_arguments() {
    let content = lines([
        json!({"type":"user","message":{"role":"user","content":"<command-message>x</command-message><command-args>Fix it nicely</command-args>"}}),
        json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"secret"},{"type":"tool_use","name":"Bash"},{"type":"text","text":"Working"}]}}),
        json!({"type":"user","isSidechain":true,"message":{"role":"user","content":"subagent"}}),
        json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"secret"}]}}),
    ]);
    let (_temporary, session) = session(Agent::Claude, &content);
    let preview = load(&session, PreviewLimits::default()).unwrap();
    assert_eq!(preview.title, "Fix it nicely");
    assert_eq!(preview.messages.len(), 2);
    assert_eq!(preview.messages[1].text, "Working");
}

#[test]
fn claude_local_command_output_is_not_a_user_prompt() {
    let content = lines([
        json!({"type":"user","message":{"role":"user","content":"<local-command-stdout>Set model to Opus</local-command-stdout>"}}),
        json!({"type":"user","message":{"role":"user","content":"Build the real feature"}}),
        json!({"type":"assistant","message":{"role":"assistant","content":"Done"}}),
    ]);
    let (_temporary, session) = session(Agent::Claude, &content);
    let preview = load(&session, PreviewLimits::default()).unwrap();
    assert_eq!(preview.title, "Build the real feature");
}

#[test]
fn consecutive_assistant_fragments_are_coalesced_around_the_user_exchange() {
    let content = lines([
        json!({"type":"user","message":"Keep this prompt"}),
        json!({"type":"assistant","message":"one"}),
        json!({"type":"assistant","message":"two"}),
        json!({"type":"assistant","message":"three"}),
    ]);
    let (_temporary, session) = session(Agent::Claude, &content);
    let preview = load(
        &session,
        PreviewLimits {
            max_messages: 2,
            ..PreviewLimits::default()
        },
    )
    .unwrap();
    assert_eq!(preview.messages.len(), 2);
    assert_eq!(preview.messages[0].text, "Keep this prompt");
    assert_eq!(preview.messages[1].text, "one two three");
}

#[test]
fn coalesced_fragments_keep_the_latest_tail_when_bounded() {
    let content = lines([
        json!({"type":"user","message":"Keep this prompt"}),
        json!({"type":"assistant","message":"first"}),
        json!({"type":"assistant","message":"second"}),
        json!({"type":"assistant","message":"LATEST"}),
    ]);
    let (_temporary, session) = session(Agent::Claude, &content);
    let preview = load(
        &session,
        PreviewLimits {
            max_message_chars: 10,
            ..PreviewLimits::default()
        },
    )
    .unwrap();
    assert_eq!(preview.messages.len(), 2);
    assert_eq!(preview.messages[1].text, "…ond LATEST");
    assert!(preview.truncated);
}

#[test]
fn bounded_multi_turn_preview_keeps_chronological_order() {
    let content = lines([
        json!({"type":"user","message":"user one"}),
        json!({"type":"assistant","message":"answer one"}),
        json!({"type":"user","message":"user two"}),
        json!({"type":"assistant","message":"answer two"}),
        json!({"type":"user","message":"user three"}),
        json!({"type":"assistant","message":"answer three"}),
    ]);
    let (_temporary, session) = session(Agent::Claude, &content);
    let preview = load(
        &session,
        PreviewLimits {
            max_messages: 4,
            ..PreviewLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        preview
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        ["user two", "answer two", "user three", "answer three"]
    );
}

#[test]
fn latest_user_is_reserved_before_newest_assistant_tail() {
    let mut messages = vec![
        PreviewMessage {
            role: PreviewRole::User,
            text: "latest user".into(),
        },
        PreviewMessage {
            role: PreviewRole::Assistant,
            text: "older fragment".into(),
        },
        PreviewMessage {
            role: PreviewRole::Assistant,
            text: "newest fragment".into(),
        },
    ];
    assert!(retain_recent_messages(&mut messages, 2));
    assert_eq!(
        messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        ["latest user", "newest fragment"]
    );
}

#[test]
fn malformed_and_incomplete_lines_do_not_hide_valid_messages() {
    let content = format!(
        "not json\n{}\n{{\"type\":\"assistant\"",
        json!({"type":"user","message":{"role":"user","content":"Keep me"}})
    );
    let (_temporary, session) = session(Agent::Claude, &content);
    let preview = load(&session, PreviewLimits::default()).unwrap();
    assert_eq!(preview.title, "Keep me");
    assert_eq!(preview.messages.len(), 1);
    assert_eq!(preview.skipped_records, 2);
}

#[test]
fn bounded_windows_find_the_first_prompt_and_latest_exchange_in_a_large_file() {
    let mut content = lines([json!({"type":"user","message":"First prompt"})]);
    for index in 0..2_000 {
        content.push_str(&format!(
            "{}\n",
            json!({"type":"assistant","message":format!("middle-{index:04}")})
        ));
    }
    content.push_str(&format!(
        "{}\n",
        json!({"type":"assistant","message":"Last answer"})
    ));
    let (_temporary, session) = session(Agent::Claude, &content);
    let limits = PreviewLimits {
        head_bytes: 128,
        tail_bytes: 256,
        max_messages: 2,
        ..PreviewLimits::default()
    };
    let preview = load(&session, limits).unwrap();
    assert_eq!(preview.title, "First prompt");
    assert!(preview.truncated);
    assert!(
        preview
            .messages
            .last()
            .unwrap()
            .text
            .ends_with("Last answer")
    );
    assert!(preview.messages.len() <= 2);
}

#[test]
fn sanitization_removes_ansi_osc_c1_and_every_control_character() {
    let dangerous =
        "\x1b[31mred\x1b[0m\n\x1b]8;;https://bad\x1b\\link\x1b]8;;\x1b\\\t\u{9b}32mgreen\u{7}";
    let clean = sanitize(dangerous);
    assert_eq!(clean, "red link green");
    assert!(!clean.chars().any(char::is_control));
    assert!(!clean.contains("https://bad"));
}

#[test]
fn message_and_title_limits_are_unicode_safe() {
    let content = lines([json!({"type":"user","message":"åßç日本語 and more"})]);
    let (_temporary, session) = session(Agent::Claude, &content);
    let preview = load(
        &session,
        PreviewLimits {
            max_message_chars: 5,
            max_title_chars: 3,
            ..PreviewLimits::default()
        },
    )
    .unwrap();
    assert_eq!(preview.title, "åßç…");
    assert_eq!(preview.messages[0].text, "åßç日本…");
}

#[test]
fn zero_byte_limits_do_not_read_the_file() {
    let (_temporary, session) = session(Agent::Codex, "this is not json");
    let preview = load(
        &session,
        PreviewLimits {
            head_bytes: 0,
            tail_bytes: 0,
            ..PreviewLimits::default()
        },
    )
    .unwrap();
    assert_eq!(preview.title, "Untitled session");
    assert!(preview.messages.is_empty());
}
