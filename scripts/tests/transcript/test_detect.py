from tools.transcript import detect


def test_detects_claude():
    assert detect.provider_of("⏺ Ran tool\n✻ Thinking…\nesc to interrupt") == "claude"


def test_detects_codex():
    assert detect.provider_of("OpenAI Codex v0.147\nWorked for 52s\ntokens used 15948") == "codex"


def test_falls_back_to_agent():
    assert detect.provider_of("just some ordinary text") == "agent"
