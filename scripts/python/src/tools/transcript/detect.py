PROVIDER_MARKERS = {
    "claude": (
        "⏺",
        "✻",
        "Claude Code",
        "claude.ai",
        "claude-fable",
        "claude-opus",
        "claude-sonnet",
        "claude-haiku",
        "esc to interrupt",
        "Claude can make mistakes",
    ),
    "codex": (
        "Codex",
        "codex-tui",
        "codex resume",
        "gpt-5",
        "OpenAI",
        "Worked for",
        "tokens used",
    ),
    "chatgpt": (
        "ChatGPT",
        "chatgpt.com",
    ),
}


def provider_of(text):
    scores = {
        name: sum(text.count(marker) for marker in markers)
        for name, markers in PROVIDER_MARKERS.items()
    }
    ranked = sorted(scores.items(), key=lambda item: item[1], reverse=True)
    if ranked[0][1] == 0 or ranked[0][1] == ranked[1][1]:
        return "agent"
    return ranked[0][0]
