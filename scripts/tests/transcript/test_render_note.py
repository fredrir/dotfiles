from datetime import UTC, datetime

from tools.transcript import render
from tools.transcript.model import Round, Session, Turn


def test_prefix_quote_handles_blank_lines():
    assert render.prefix_quote("a\n\nb") == "> a\n>\n> b"


def test_fence_grows_past_embedded_backticks():
    body = "```python\nprint('hi')\n```"
    assert render.fence_for(body) == "````"


def test_cap_lines_inserts_omission_marker():
    text = "\n".join(str(n) for n in range(50))
    capped = render.cap_lines(text)
    assert "… (+25 lines omitted)" in capped
    assert capped.splitlines()[0] == "0"
    assert capped.splitlines()[-1] == "49"


def test_render_session_headings_and_turns():
    stamp = datetime(2026, 8, 9, 16, 32, tzinfo=UTC)
    session = Session(provider="claude", session_id="s", source_path="x")
    session.rounds = [
        Round(timestamp=stamp, label="fix the thing", turns=[Turn("me", "You", "fix the thing")]),
        Round(timestamp=stamp, label="fix the thing", turns=[Turn("turn", "Response", "done")]),
    ]
    body = render.render_session(session)
    assert "### 16:32 — fix the thing" in body
    assert "### 16:32 — fix the thing (2)" in body
    assert "> [!me]+ You" in body
    assert "> [!turn|claude]- Claude" in body


def test_tool_turns_dropped_by_default_and_responses_merged():
    stamp = datetime(2026, 8, 9, 16, 32, tzinfo=UTC)
    session = Session(provider="codex", session_id="s", source_path="x")
    session.rounds = [
        Round(
            timestamp=stamp,
            label="do it",
            turns=[
                Turn("me", "You", "do it"),
                Turn("turn", "Response", "first part"),
                Turn("tool", "Bash · ls", "output"),
                Turn("turn", "Response", "second part"),
            ],
        ),
    ]
    body = render.render_session(session)
    assert "[!tool]" not in body
    assert body.count("[!turn|codex]- Codex") == 1
    assert "first part\n>\n> second part" in body
    with_tools = render.render_session(session, include_tools=True)
    assert "[!tool]- Bash · ls" in with_tools
    assert with_tools.count("[!turn|codex]- Codex") == 2


def test_toc_added_for_long_sessions():
    stamp = datetime(2026, 8, 9, 16, 32, tzinfo=UTC)
    session = Session(provider="claude", session_id="s", source_path="x")
    session.rounds = [
        Round(timestamp=stamp, label=f"question {n}", turns=[Turn("me", "You", f"question {n}")])
        for n in range(5)
    ]
    body = render.render_session(session)
    assert body.startswith("> [!toc]- Contents")
    assert "> - [[#16:32 — question 0|16:32 — question 0]]" in body


def test_degraded_session_renders_raw():
    session = Session(provider="codex", session_id="s", source_path="x")
    session.degraded = True
    session.raw_text = "some raw junk"
    body = render.render_session(session)
    assert "Degraded import" in body
    assert "some raw junk" in body


def test_capture_uses_provider_callout():
    stamp = datetime(2026, 8, 9, 16, 32, tzinfo=UTC)
    body = render.render_capture("codex", stamp, "hello world")
    assert body.startswith("> [!codex]- 16:32 clipboard capture")
    assert "> hello world" in body
