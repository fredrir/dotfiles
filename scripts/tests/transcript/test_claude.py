import json

from tools.transcript import claude


def write_session(tmp_path, lines):
    path = tmp_path / "abc-123.jsonl"
    path.write_text("\n".join(json.dumps(line) for line in lines))
    return path


def user(text, **extra):
    entry = {
        "type": "user",
        "message": {"role": "user", "content": text},
        "cwd": "/home/fredrir/dotfiles",
        "sessionId": "abc-123",
        "timestamp": "2026-08-09T10:00:00Z",
    }
    entry.update(extra)
    return entry


def assistant(blocks, model="claude-fable-5"):
    return {
        "type": "assistant",
        "message": {"role": "assistant", "content": blocks, "model": model},
        "timestamp": "2026-08-09T10:00:05Z",
    }


def test_parses_rounds_and_tools(tmp_path):
    path = write_session(
        tmp_path,
        [
            {"type": "ai-title", "aiTitle": "Fix the sync script"},
            user("please fix the sync script"),
            assistant(
                [
                    {"type": "text", "text": "Looking at it now."},
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "rg -n sync"}},
                ]
            ),
            user(
                [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "scripts/sync.py:3"},
                ]
            ),
            assistant([{"type": "text", "text": "Found it."}]),
            user("thanks, ship it"),
        ],
    )
    session = claude.parse(path)
    assert session.title == "Fix the sync script"
    assert session.model == "claude-fable-5"
    assert session.cwd == "/home/fredrir/dotfiles"
    assert session.user_rounds == 2
    kinds = [turn.kind for turn in session.rounds[0].turns]
    assert kinds == ["me", "turn", "tool", "turn"]
    tool = session.rounds[0].turns[2]
    assert "Bash" in tool.title and "rg -n sync" in tool.title
    assert tool.body == "scripts/sync.py:3"


def test_skips_meta_and_sidechain(tmp_path):
    path = write_session(
        tmp_path,
        [
            user("<local-command-caveat>noise</local-command-caveat>", isMeta=True),
            user("real question"),
            assistant([{"type": "text", "text": "sidechain"}], model="m")
            | {"isSidechain": True},
            assistant([{"type": "text", "text": "answer"}]),
        ],
    )
    session = claude.parse(path)
    assert session.user_rounds == 1
    assert [t.kind for t in session.rounds[0].turns] == ["me", "turn"]
    assert session.rounds[0].turns[1].body == "answer"


def test_command_messages_reduced(tmp_path):
    text = "<command-name>/model</command-name><command-args>opus</command-args>"
    path = write_session(tmp_path, [user(text), assistant([{"type": "text", "text": "ok"}])])
    session = claude.parse(path)
    assert session.rounds[0].turns[0].body == "/model opus"


def test_degrades_on_garbage(tmp_path):
    path = tmp_path / "junk.jsonl"
    path.write_text("not json at all\nstill not json\n{\"type\": \"unknown-thing\"}\n")
    session = claude.parse(path)
    assert session.degraded
    assert "not json at all" in session.raw_text
