from tools.core.patterns import TOKEN_PATTERNS, VALUE_PATTERN


def redact(text):
    for label, pattern in TOKEN_PATTERNS:
        text = pattern.sub(f"[redacted:{label}]", text)
    return VALUE_PATTERN.sub(lambda m: f"{m.group(1)}{m.group(2)}[redacted:value]", text)


def passthrough(text):
    return text
