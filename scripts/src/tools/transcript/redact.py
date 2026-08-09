import re

TOKEN_PATTERNS = (
    ("github-token", re.compile(r"\bgh[pousr]_[A-Za-z0-9]{20,}\b")),
    ("github-token", re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b")),
    ("api-key", re.compile(r"\bsk-[A-Za-z0-9_-]{16,}\b")),
    ("aws-key", re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("slack-token", re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b")),
    ("jwt", re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")),
    (
        "private-key",
        re.compile(
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|\Z)",
            re.DOTALL,
        ),
    ),
)

VALUE_PATTERN = re.compile(
    r"(?i)\b(api[_-]?key|access[_-]?token|auth[_-]?token|refresh[_-]?token|client[_-]?secret"
    r"|secret[_-]?key|password|passwd)\b(\s*[:=]\s*)(\"[^\"]{8,}\"|'[^']{8,}'|\S{8,})"
)


def redact(text):
    for label, pattern in TOKEN_PATTERNS:
        text = pattern.sub(f"[redacted:{label}]", text)
    return VALUE_PATTERN.sub(lambda m: f"{m.group(1)}{m.group(2)}[redacted:value]", text)


def passthrough(text):
    return text
