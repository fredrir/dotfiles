import re

from tools.core.patterns import TOKEN_PATTERNS, VALUE_PATTERN


def redact(text, private=()):
    for label, pattern in TOKEN_PATTERNS:
        text = pattern.sub(f"[redacted:{label}]", text)
    text = VALUE_PATTERN.sub(lambda m: f"{m.group(1)}{m.group(2)}[redacted:value]", text)
    return strip_private(text, private)


def strip_private(text, private):
    for value in private:
        if value:
            text = re.sub(re.escape(value), "[redacted:private]", text, flags=re.IGNORECASE)
    return text


def redactor(private=()):
    return lambda text: redact(text, private)


def passthrough(text):
    return text
