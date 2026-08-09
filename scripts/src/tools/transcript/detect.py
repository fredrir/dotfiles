CLAUDE_MARKERS = (
    "⏺",
    "✻",
    "Claude Code",
    "claude.ai/code",
    "claude-fable",
    "claude-opus",
    "claude-sonnet",
    "claude-haiku",
    "esc to interrupt",
)

CODEX_MARKERS = (
    "Codex",
    "codex-tui",
    "codex resume",
    "gpt-5",
    "OpenAI",
    "Worked for",
    "tokens used",
)


def provider_of(text):
    claude = sum(text.count(marker) for marker in CLAUDE_MARKERS)
    codex = sum(text.count(marker) for marker in CODEX_MARKERS)
    if claude > codex:
        return "claude"
    if codex > claude:
        return "codex"
    return "agent"
