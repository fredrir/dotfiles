import bisect
import re

TOKEN_PATTERNS = (
    ("github-token", re.compile(r"\bgh[pousr]_[A-Za-z0-9]{20,}\b")),
    ("github-token", re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b")),
    ("gitlab-token", re.compile(r"\bglpat-[A-Za-z0-9_-]{20,}\b")),
    ("npm-token", re.compile(r"\bnpm_[A-Za-z0-9]{36}\b")),
    ("api-key", re.compile(r"\bsk-[A-Za-z0-9_-]{16,}\b")),
    ("aws-key", re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("slack-token", re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b")),
    ("jwt", re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")),
    ("age-identity", re.compile(r"\bAGE-SECRET-KEY-1[0-9A-Z]{50,}\b")),
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

SOPS_MARKER = "ENC[AES256_GCM"

KEY_FILENAMES = (
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ecdsa_sk",
    "id_ed25519",
    "id_ed25519_sk",
    "keys.txt",
    "credentials",
    ".env",
    ".env.local",
    ".netrc",
    ".pgpass",
)

KEY_SUFFIXES = (".pem", ".key", ".p12", ".pfx", ".jks", ".keystore", ".kdbx", ".ovpn")


def mask(value):
    text = " ".join(str(value).split())
    if len(text) < 12:
        return "*" * len(text)
    return f"{text[:4]}…{text[-4:]}"


def line_starts(text):
    offsets = [0]
    position = text.find("\n")
    while position != -1:
        offsets.append(position + 1)
        position = text.find("\n", position + 1)
    return offsets


def line_of(offsets, position):
    return bisect.bisect_right(offsets, position)


def token_findings(text):
    offsets = line_starts(text)
    for label, pattern in TOKEN_PATTERNS:
        for match in pattern.finditer(text):
            yield label, line_of(offsets, match.start()), match.group(0)
    for match in VALUE_PATTERN.finditer(text):
        yield "value", line_of(offsets, match.start()), match.group(3)
