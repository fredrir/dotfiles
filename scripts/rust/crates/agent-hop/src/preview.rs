use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use serde_json::Value;

use crate::cli::Agent;
use crate::session::Session;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreviewLimits {
    pub(crate) head_bytes: usize,
    pub(crate) tail_bytes: usize,
    pub(crate) max_records_per_window: usize,
    pub(crate) max_messages: usize,
    pub(crate) max_message_chars: usize,
    pub(crate) max_title_chars: usize,
}

impl Default for PreviewLimits {
    fn default() -> Self {
        Self {
            head_bytes: 256 * 1024,
            tail_bytes: 768 * 1024,
            max_records_per_window: 4_096,
            max_messages: 8,
            max_message_chars: 1_200,
            max_title_chars: 96,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionPreview {
    pub(crate) title: String,
    pub(crate) messages: Vec<PreviewMessage>,
    pub(crate) truncated: bool,
    pub(crate) skipped_records: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreviewMessage {
    pub(crate) role: PreviewRole,
    pub(crate) text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreviewRole {
    User,
    Assistant,
}

/// Lazily load a bounded head and tail of a validated local transcript.
///
/// The head supplies the title/first meaningful prompt while the tail supplies
/// recent conversation. Individual malformed records and an incomplete active
/// final line are ignored rather than making the whole preview unavailable.
pub(crate) fn load(session: &Session, limits: PreviewLimits) -> Result<SessionPreview, String> {
    if limits.head_bytes == 0 && limits.tail_bytes == 0 {
        return Ok(SessionPreview {
            title: "Untitled session".to_owned(),
            messages: Vec::new(),
            truncated: true,
            skipped_records: 0,
        });
    }
    let (windows, mut truncated) = read_windows(&session.transcript, limits)?;
    let mut messages: Vec<PreviewMessage> = Vec::new();
    let mut first_user = None;
    let mut skipped_records = 0;

    for window in windows {
        let mut seen = 0;
        let mut can_merge = false;
        for line in window.bytes.split(|byte| *byte == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if seen >= limits.max_records_per_window {
                skipped_records += 1;
                continue;
            }
            seen += 1;
            let record: Value = match serde_json::from_slice(line) {
                Ok(record) => record,
                Err(_) => {
                    // Window edges and actively-written final records are allowed
                    // to be incomplete. They are still counted for diagnostics.
                    skipped_records += 1;
                    continue;
                }
            };
            let Some((role, raw)) = extract(session.agent, &record) else {
                continue;
            };
            let raw = if role == PreviewRole::User {
                user_authored_text(&raw)
            } else {
                Some(raw)
            };
            let Some(raw) = raw else {
                continue;
            };
            let clean = sanitize(&raw);
            if clean.is_empty() {
                continue;
            }
            if role == PreviewRole::User && first_user.is_none() {
                first_user = Some(sanitize_limited(&clean, limits.max_title_chars));
            }
            if can_merge
                && let Some(previous) = messages.last_mut()
                && previous.role == role
            {
                let combined = format!("{} {}", previous.text, clean);
                let (combined, shortened) = tail_limited(&combined, limits.max_message_chars);
                previous.text = combined;
                truncated |= shortened;
            } else {
                let (text, shortened) =
                    sanitize_limited_with_status(&clean, limits.max_message_chars);
                truncated |= shortened;
                messages.push(PreviewMessage { role, text });
            }
            can_merge = true;
        }
    }
    truncated |= retain_recent_messages(&mut messages, limits.max_messages);
    truncated |= skipped_records > 0;
    Ok(SessionPreview {
        title: first_user.unwrap_or_else(|| "Untitled session".to_owned()),
        messages,
        truncated,
        skipped_records,
    })
}

/// Bound a chronological conversation while retaining the latest user turn.
///
/// Ordinarily the latest tail already contains that turn and is kept as-is. A
/// user turn can fall outside the tail when the bounded head and tail windows
/// contain separate assistant runs; in that case one slot is reserved for the
/// user and the remaining newest messages are appended after it.
fn retain_recent_messages(messages: &mut Vec<PreviewMessage>, limit: usize) -> bool {
    if messages.len() <= limit {
        return false;
    }
    if limit == 0 {
        messages.clear();
        return true;
    }
    let start = messages.len() - limit;
    let latest_user = messages
        .iter()
        .rposition(|message| message.role == PreviewRole::User);
    if limit > 1 && latest_user.is_some_and(|index| index < start) {
        let user_index = latest_user.unwrap();
        let mut retained = Vec::with_capacity(limit);
        retained.push(messages[user_index].clone());
        retained.extend(messages[messages.len() - (limit - 1)..].iter().cloned());
        *messages = retained;
    } else {
        messages.drain(..start);
    }
    true
}

struct Window {
    bytes: Vec<u8>,
}

fn read_windows(
    path: &std::path::Path,
    limits: PreviewLimits,
) -> Result<(Vec<Window>, bool), String> {
    let mut file = File::open(path)
        .map_err(|error| format!("could not open preview {}: {error}", path.display()))?;
    let length = file
        .metadata()
        .map_err(|error| format!("could not inspect preview {}: {error}", path.display()))?
        .len();
    let bounded = (limits.head_bytes as u64).saturating_add(limits.tail_bytes as u64);
    if length <= bounded {
        let mut bytes = Vec::with_capacity(length.min(usize::MAX as u64) as usize);
        file.take(bounded)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("could not read preview {}: {error}", path.display()))?;
        return Ok((vec![Window { bytes }], false));
    }

    let mut head = Vec::with_capacity(limits.head_bytes);
    file.by_ref()
        .take(limits.head_bytes as u64)
        .read_to_end(&mut head)
        .map_err(|error| format!("could not read preview {}: {error}", path.display()))?;
    // The end of the prefix may be a partial JSON record.
    if !head.ends_with(b"\n") {
        if let Some(index) = head.iter().rposition(|byte| *byte == b'\n') {
            head.truncate(index + 1);
        } else {
            head.clear();
        }
    }

    let tail_start = length.saturating_sub(limits.tail_bytes as u64);
    let read_start = tail_start.saturating_sub(1);
    file.seek(SeekFrom::Start(read_start))
        .map_err(|error| format!("could not seek preview {}: {error}", path.display()))?;
    let allowance = limits.tail_bytes.saturating_add((tail_start > 0) as usize);
    let mut tail = Vec::with_capacity(allowance);
    file.take(allowance as u64)
        .read_to_end(&mut tail)
        .map_err(|error| format!("could not read preview {}: {error}", path.display()))?;
    if tail_start > 0 && !tail.is_empty() {
        let preceding = tail.remove(0);
        if preceding != b'\n' {
            if let Some(index) = tail.iter().position(|byte| *byte == b'\n') {
                tail.drain(..=index);
            } else {
                tail.clear();
            }
        }
    }
    Ok((vec![Window { bytes: head }, Window { bytes: tail }], true))
}

fn extract(agent: Agent, record: &Value) -> Option<(PreviewRole, String)> {
    match agent {
        Agent::Codex => extract_codex(record),
        Agent::Claude => extract_claude(record),
    }
}

fn extract_codex(record: &Value) -> Option<(PreviewRole, String)> {
    match record.get("type").and_then(Value::as_str) {
        Some("response_item") => {
            let payload = record.get("payload")?;
            if payload
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind != "message")
            {
                return None;
            }
            let role = role(payload.get("role")?.as_str()?)?;
            if role == PreviewRole::User && !codex_user_authored(payload) {
                return None;
            }
            content_text(payload.get("content")?).map(|text| (role, text))
        }
        Some("event_msg") => {
            let payload = record.get("payload")?;
            let role = match payload.get("type").and_then(Value::as_str)? {
                "user_message" => PreviewRole::User,
                "agent_message" | "assistant_message" => PreviewRole::Assistant,
                _ => return None,
            };
            payload
                .get("message")
                .and_then(Value::as_str)
                .map(|text| (role, text.to_owned()))
        }
        _ => None,
    }
}

fn codex_user_authored(payload: &Value) -> bool {
    let Some(kinds) = payload
        .get("internal_chat_message_metadata_passthrough")
        .and_then(|metadata| metadata.get("content_item_kinds"))
        .and_then(Value::as_array)
    else {
        return true;
    };
    kinds
        .iter()
        .filter_map(Value::as_str)
        .any(|kind| kind == "user.text" || kind == "user.image" || kind.starts_with("user."))
}

fn extract_claude(record: &Value) -> Option<(PreviewRole, String)> {
    if record.get("isSidechain") == Some(&Value::Bool(true))
        || record.get("isMeta") == Some(&Value::Bool(true))
    {
        return None;
    }
    let role = role(record.get("type")?.as_str()?)?;
    let message = record.get("message")?;
    let content = if let Some(content) = message.get("content") {
        content
    } else {
        message
    };
    content_text(content).map(|text| (role, text))
}

fn role(value: &str) -> Option<PreviewRole> {
    match value {
        "user" => Some(PreviewRole::User),
        "assistant" => Some(PreviewRole::Assistant),
        _ => None,
    }
}

fn content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let values = content.as_array()?;
    let mut texts = Vec::new();
    for value in values {
        let kind = value.get("type").and_then(Value::as_str);
        if kind.is_some_and(|kind| {
            matches!(
                kind,
                "thinking"
                    | "reasoning"
                    | "tool_use"
                    | "tool_result"
                    | "function_call"
                    | "function_call_output"
            )
        }) {
            continue;
        }
        if !kind.is_none_or(|kind| matches!(kind, "text" | "input_text" | "output_text")) {
            continue;
        }
        if let Some(text) = value.get("text").and_then(Value::as_str) {
            texts.push(text);
        }
    }
    (!texts.is_empty()).then(|| texts.join("\n"))
}

fn user_authored_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if let Some(arguments) = between(trimmed, "<command-args>", "</command-args>") {
        return Some(arguments.to_owned());
    }
    if trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<permissions instructions>")
        || trimmed.starts_with("<turn_aborted>")
        || trimmed.starts_with("<local-command-")
        || trimmed.starts_with("<command-name>")
        || trimmed.starts_with("<command-message>")
        || trimmed.starts_with("<system-reminder>")
        || trimmed.starts_with("Base directory for this skill:")
    {
        return None;
    }
    Some(trimmed.to_owned())
}

fn between<'a>(value: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start = value.find(start)? + start.len();
    let end = value[start..].find(end)? + start;
    Some(value[start..end].trim())
}

/// Remove terminal escape sequences and normalize every control/whitespace run
/// to one plain space, making text safe for both a TUI and plain output.
pub(crate) fn sanitize(value: &str) -> String {
    sanitize_limited(value, usize::MAX)
}

fn sanitize_limited(value: &str, limit: usize) -> String {
    sanitize_limited_with_status(value, limit).0
}

/// Keep the newest visible characters of already-sanitized text. Prefixing an
/// ellipsis makes it explicit that earlier same-role fragments were dropped.
fn tail_limited(value: &str, limit: usize) -> (String, bool) {
    let count = value.chars().count();
    if count <= limit {
        return (value.to_owned(), false);
    }
    let tail = value.chars().skip(count - limit).collect::<String>();
    (format!("…{tail}"), true)
}

fn sanitize_limited_with_status(value: &str, limit: usize) -> (String, bool) {
    #[derive(Clone, Copy)]
    enum State {
        Text,
        Escape,
        Csi,
        Osc,
        String,
        StringEscape,
    }

    let mut state = State::Text;
    let mut output = String::new();
    let mut whitespace = false;
    let mut count = 0;
    let mut shortened = false;
    for character in value.chars() {
        match state {
            State::Text => match character {
                '\u{1b}' => state = State::Escape,
                '\u{9b}' => state = State::Csi,
                '\u{9d}' => state = State::Osc,
                '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => state = State::String,
                character if character.is_control() || character.is_whitespace() => {
                    whitespace = !output.is_empty();
                }
                character => {
                    if count >= limit {
                        shortened = true;
                        break;
                    }
                    if whitespace {
                        output.push(' ');
                        whitespace = false;
                    }
                    output.push(character);
                    count += 1;
                }
            },
            State::Escape => {
                state = match character {
                    '[' => State::Csi,
                    ']' => State::Osc,
                    'P' | 'X' | '^' | '_' => State::String,
                    _ => State::Text,
                }
            }
            State::Csi => {
                if ('@'..='~').contains(&character) {
                    state = State::Text;
                }
            }
            State::Osc => {
                if character == '\u{7}' {
                    state = State::Text;
                } else if character == '\u{1b}' {
                    state = State::StringEscape;
                }
            }
            State::String => {
                if character == '\u{1b}' {
                    state = State::StringEscape;
                }
            }
            State::StringEscape => {
                state = if character == '\\' {
                    State::Text
                } else {
                    State::String
                };
            }
        }
    }
    if shortened {
        output.push('…');
    }
    (output, shortened)
}

#[cfg(test)]
mod tests {
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
}
