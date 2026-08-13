import json

from tools.transcript import codex


def write_session(tmp_path, lines):
    path = tmp_path / "rollout-2026-08-09T11-00-09-019fe5c0-416d-7d80-a669-16367fc1f819.jsonl"
    path.write_text("\n".join(json.dumps(line) for line in lines))
    return path


def test_parses_meta_rounds_and_tools(tmp_path):
    path = write_session(
        tmp_path,
        [
            {
                "type": "session_meta",
                "payload": {
                    "id": "019fe5c0-416d-7d80-a669-16367fc1f819",
                    "timestamp": "2026-08-09T09:00:09.991Z",
                    "cwd": "/home/fredrir/projects/ArchTeX",
                },
            },
            {"type": "turn_context", "payload": {"model": "gpt-5.6-sol"}},
            {
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": "<user_instructions>rules</user_instructions>",
                        },
                        {"type": "input_text", "text": "tighten the layout"},
                    ],
                },
            },
            {
                "type": "response_item",
                "payload": {"type": "reasoning", "summary": []},
            },
            {
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call",
                    "call_id": "c1",
                    "name": "exec",
                    "input": "ls -la",
                },
            },
            {
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call_output",
                    "call_id": "c1",
                    "output": [{"type": "input_text", "text": "total 42"}],
                },
            },
            {
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "Done."}],
                },
            },
        ],
    )
    session = codex.parse(path)
    assert session.session_id == "019fe5c0-416d-7d80-a669-16367fc1f819"
    assert session.model == "gpt-5.6-sol"
    assert session.cwd == "/home/fredrir/projects/ArchTeX"
    assert session.user_rounds == 1
    turns = session.rounds[0].turns
    assert [t.kind for t in turns] == ["me", "tool", "turn"]
    assert turns[0].body == "tighten the layout"
    assert turns[1].body == "total 42"


def test_skips_developer_messages(tmp_path):
    path = write_session(
        tmp_path,
        [
            {
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "internal instructions"}],
                },
            },
            {
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "hello"}],
                },
            },
        ],
    )
    session = codex.parse(path)
    assert session.user_rounds == 1
    assert session.rounds[0].turns[0].body == "hello"


def test_skips_agents_md_instruction_messages(tmp_path):
    path = write_session(
        tmp_path,
        [
            {
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": "# AGENTS.md instructions for /home/fredrir/dotfiles",
                        }
                    ],
                },
            },
            {
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "actual question"}],
                },
            },
        ],
    )
    session = codex.parse(path)
    assert session.user_rounds == 1
    assert session.title == "actual question"


def test_falls_back_to_event_messages(tmp_path):
    path = write_session(
        tmp_path,
        [
            {"type": "event_msg", "payload": {"type": "user_message", "message": "hi there"}},
            {"type": "event_msg", "payload": {"type": "agent_message", "message": "hello back"}},
        ],
    )
    session = codex.parse(path)
    assert session.user_rounds == 1
    assert [t.kind for t in session.rounds[0].turns] == ["me", "turn"]
