import os
import shlex

from tools.core.console import colors_enabled
from tools.core.patterns import (
    KEY_FILENAMES,
    KEY_SUFFIXES,
    SOPS_MARKER,
    line_of,
    line_starts,
    mask,
    token_findings,
)
from tools.core.process import capture, capture_bytes
from tools.dotfile.report import plural
from tools.dotfile.secret.allow import allowed, load_allow
from tools.dotfile.secret.canaries import load_canaries
from tools.dotfile.secret.vault import is_encrypted_name
from tools.dotfile.state import die, log

MAX_BYTES = 2 * 1024 * 1024
BINARY_WINDOW = 8192
ITEM_LIMIT = 12

BOLD = "\033[1m"
DIM = "\033[2m"
GREEN = "\033[32m"
RED = "\033[31m"
YELLOW = "\033[33m"
RESET = "\033[0m"

TIER_RANK = {"canary": 0, "invariant": 1, "pattern": 2}


class Finding:
    def __init__(self, tier, label, path, line, detail):
        self.tier = tier
        self.label = label
        self.path = path
        self.line = line
        self.detail = detail

    def where(self):
        return f"{self.path}:{self.line}" if self.line else self.path


def git(ctx, *args):
    result = capture(["git", "-C", ctx.root, *args])
    if result.returncode != 0:
        die(f"git {args[0]} failed: {result.stderr.strip()}")
    return result.stdout


def tracked_paths(ctx):
    return [path for path in git(ctx, "ls-files", "-z").split("\0") if path]


def read_worktree(ctx, path):
    try:
        with open(os.path.join(ctx.root, path), "rb") as handle:
            return handle.read(MAX_BYTES + 1)
    except OSError:
        return None


def read_blob(ctx, ref):
    result = capture_bytes(["git", "-C", ctx.root, "cat-file", "blob", ref])
    if result.returncode != 0:
        return None
    return result.stdout


def worktree_source(ctx, paths):
    for path in paths or tracked_paths(ctx):
        yield path, read_worktree(ctx, path)


def staged_source(ctx):
    listed = git(ctx, "diff", "--cached", "--name-only", "--diff-filter=ACMR", "-z")
    for path in listed.split("\0"):
        if path:
            yield path, read_blob(ctx, f":{path}")


def commits_source(ctx, spec):
    listed = git(ctx, "rev-list", *shlex.split(spec))
    seen = set()
    for commit in listed.split():
        raw = git(ctx, "diff-tree", "-r", "--root", "--no-commit-id", "--diff-filter=AM", commit)
        for line in raw.splitlines():
            meta, _, path = line.partition("\t")
            fields = meta.split()
            if len(fields) < 4 or not path:
                continue
            blob = fields[3]
            if blob in seen:
                continue
            seen.add(blob)
            yield path, read_blob(ctx, blob)


def decode(data):
    if data is None or len(data) > MAX_BYTES:
        return None
    if b"\0" in data[:BINARY_WINDOW]:
        return None
    return data.decode("utf-8", errors="replace")


def secret_dirs(ctx):
    return {
        os.path.dirname(path) for path in tracked_paths(ctx) if os.path.basename(path) == ".secret"
    }


def inside_secret(dirs, path):
    return any(path == directory or path.startswith(directory + "/") for directory in dirs)


def encrypted_paths(ctx):
    return [path for path in tracked_paths(ctx) if is_encrypted_name(path)]


def looks_like_key(path):
    base = os.path.basename(path)
    return base in KEY_FILENAMES or base.endswith(KEY_SUFFIXES)


def pattern_tier(rules, path, text):
    for label, line, matched in token_findings(text):
        if allowed(rules, path, label):
            continue
        yield Finding("pattern", label, path, line, mask(matched))


def canary_tier(canaries, path, text):
    if not canaries:
        return
    lowered = text.lower()
    offsets = line_starts(text)
    for canary in canaries:
        position = lowered.find(canary.needle)
        if position != -1:
            yield Finding("canary", canary.label, path, line_of(offsets, position), "private value")


def invariant_tier(dirs, path, encrypted):
    base = os.path.basename(path)
    if is_encrypted_name(path) and not encrypted:
        yield Finding("invariant", "not-encrypted", path, 0, "named .enc, carries no sops metadata")
        return
    if inside_secret(dirs, path) and base != ".secret" and not encrypted:
        yield Finding("invariant", "plaintext", path, 0, "unencrypted inside a .secret package")
        return
    if looks_like_key(path) and not inside_secret(dirs, path):
        yield Finding("invariant", "key-file", path, 0, "key filename outside a .secret package")


def select_source(ctx, paths, staged, commits):
    if staged and commits:
        die("--staged and --commits cannot be combined")
    if staged:
        return staged_source(ctx)
    if commits:
        return commits_source(ctx, commits)
    return worktree_source(ctx, paths)


def paint(text, color, on):
    return f"{color}{text}{RESET}" if on else text


def report(findings, scanned, skipped, canaries, notes, show_all):
    on = colors_enabled()
    for note in notes:
        log(paint("!", YELLOW, on) + f" {note}")

    if not findings:
        tail = f", {skipped} not text" if skipped else ""
        summary = f"{plural(scanned, 'file')}, {plural(canaries, 'canary', 'canaries')}"
        log(paint("✓", GREEN, on) + f" clean  {summary}{tail}")
        return

    shown = findings if show_all else findings[:ITEM_LIMIT]
    for finding in shown:
        log(
            paint("✗", RED, on)
            + f" {finding.tier:<9} {finding.where()}"
            + paint(f"  {finding.label}", BOLD, on)
            + paint(f"  {finding.detail}", DIM, on)
        )
    if len(findings) > len(shown):
        log(f"  … {len(findings) - len(shown)} more (--all)")
    log("")
    log(f"{plural(len(findings), 'finding')} in {plural(scanned, 'file')}")
    log("allow a false positive in scan.dotfile; a canary is never allowed")
    raise SystemExit(1)


def cmd_scan(ctx, paths, staged, commits, use_canaries, show_all):
    rules = load_allow(ctx)
    canaries, notes = load_canaries(ctx) if use_canaries else ([], [])
    dirs = secret_dirs(ctx)

    findings = []
    scanned = 0
    skipped = 0
    for path, data in select_source(ctx, paths, staged, commits):
        text = decode(data)
        if text is None:
            skipped += 1
            findings.extend(invariant_tier(dirs, path, False))
            continue
        scanned += 1
        encrypted = SOPS_MARKER in text
        findings.extend(invariant_tier(dirs, path, encrypted))
        if encrypted:
            continue
        findings.extend(pattern_tier(rules, path, text))
        findings.extend(canary_tier(canaries, path, text))

    findings.sort(key=lambda finding: (TIER_RANK[finding.tier], finding.path, finding.line))
    report(findings, scanned, skipped, len(canaries), notes, show_all)
