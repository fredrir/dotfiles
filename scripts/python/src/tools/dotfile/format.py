import fnmatch
import os
import re
import sys

from tools.core.process import capture
from tools.dotfile.state import die, log

HYPR_PATTERNS = ("*/hypr/*", "*/hypr-local.conf", "hypr*.conf")
PLAIN_PATTERNS = ("*/kitty/colors*.conf", "*/colors*.conf", "*/kitty/conf.d/fonts.conf")
KITTY_PATTERNS = ("*/kitty/*.conf", "*/kitty.conf")

HYPR_KEY = re.compile(r"^[$A-Za-z0-9_.:-]+$")
HYPR_OPEN = re.compile(r"\{[ \t]*$")
HYPR_COMMENT = re.compile(r"^[ \t]*#")


def conf_mode(path):
    for pattern in HYPR_PATTERNS:
        if fnmatch.fnmatchcase(path, pattern):
            return "hypr"
    for pattern in PLAIN_PATTERNS:
        if fnmatch.fnmatchcase(path, pattern):
            return "plain"
    for pattern in KITTY_PATTERNS:
        if fnmatch.fnmatchcase(path, pattern):
            return "kitty"
    return "plain"


def compact(text):
    out = []
    quote = ""
    escaped = False
    space = False
    for ch in text:
        if quote:
            out.append(ch)
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == quote:
                quote = ""
        elif ch in ('"', "'"):
            if space and out:
                out.append(" ")
            space = False
            quote = ch
            out.append(ch)
        elif ch in (" ", "\t"):
            space = True
        else:
            if space and out:
                out.append(" ")
            space = False
            out.append(ch)
    return "".join(out)


def format_kitty(lines):
    stored = []
    blank_pending = False
    key_width = 0
    map_width = 0
    for raw in lines:
        line = raw.rstrip(" \t")
        if line == "":
            if stored:
                blank_pending = True
            continue
        entry = compact(line)
        if blank_pending and stored:
            stored.append("")
        blank_pending = False
        stored.append(entry)
        if entry.startswith("#") or " " not in entry:
            continue
        key, _, tail = entry.partition(" ")
        if key == "map":
            shortcut, sep, _action = tail.partition(" ")
            if not sep:
                continue
            map_width = max(map_width, len(shortcut))
        else:
            key_width = max(key_width, len(key))

    out = []
    for entry in stored:
        if entry == "" or entry.startswith("#") or " " not in entry:
            out.append(entry)
            continue
        key, _, value = entry.partition(" ")
        if key != "map":
            out.append(key + " " * (key_width - len(key) + 2) + value)
            continue
        shortcut, sep, action = value.partition(" ")
        if not sep:
            out.append(entry)
            continue
        out.append("map " + shortcut + " " * (map_width - len(shortcut) + 2) + action)
    return out


def hypr_line(line, indent):
    line = line.lstrip(" \t")
    if line == "}":
        indent -= 1
    indent = max(indent, 0)
    position = line.find("=")
    if not line.startswith("#") and position != -1:
        lhs = line[:position].rstrip(" \t")
        rhs = line[position + 1 :].lstrip(" \t")
        if HYPR_KEY.match(lhs):
            line = f"{lhs} =" + (f" {rhs}" if rhs else "")
    line = "    " * indent + line
    if not HYPR_COMMENT.match(line) and HYPR_OPEN.search(line):
        indent += 1
    return line, indent


def format_lines(lines, mode):
    if mode == "kitty":
        return format_kitty(lines)
    out = []
    printed = False
    blank = False
    indent = 0
    for raw in lines:
        line = raw.rstrip(" \t")
        if line == "":
            if printed:
                blank = True
            continue
        closing = line.lstrip(" \t")
        if blank and not (mode == "hypr" and closing == "}"):
            out.append("")
        blank = False
        if mode == "hypr":
            line, indent = hypr_line(line, indent)
        out.append(line)
        printed = True
    return out


def format_text(text, mode):
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    formatted = format_lines(lines, mode)
    if not formatted:
        return ""
    return "\n".join(formatted) + "\n"


def format_conf_file(ctx, file):
    if not os.path.isfile(file):
        die(f"not a file: {file}")
    if not file.endswith(".conf"):
        die(f"not a .conf file: {file}")
    with open(file, encoding="utf-8") as handle:
        text = handle.read()
    formatted = format_text(text, conf_mode(file))
    if formatted == text:
        return 0
    with open(file, "w", encoding="utf-8") as handle:
        handle.write(formatted)
    label = file[len(ctx.root) + 1 :] if file.startswith(ctx.root + "/") else file
    log(f"  format {label}")
    return 1


def tracked_conf_files(ctx):
    result = capture(["git", "-C", ctx.root, "ls-files", "-z", "--", "*.conf"])
    return [name for name in result.stdout.split("\0") if name]


def conf_files_under(directory):
    found = []
    for parent, _dirnames, filenames in os.walk(directory):
        for name in filenames:
            if name.endswith(".conf"):
                found.append(os.path.join(parent, name))
    return sorted(found)


def cmd_format(ctx, paths, stdin_name):
    if stdin_name is not None:
        if paths:
            die("usage: dotfile format --stdin <filename>")
        sys.stdout.write(format_text(sys.stdin.read(), conf_mode(stdin_name)))
        return

    files = []
    if not paths:
        for path in tracked_conf_files(ctx):
            files.append(os.path.join(ctx.root, path))
    else:
        for path in paths:
            if os.path.isdir(path):
                files.extend(conf_files_under(path))
            else:
                files.append(path)

    if not files:
        die("no .conf files found")
    changed = 0
    for file in files:
        changed += format_conf_file(ctx, file)
    log(f"formatted {changed} of {len(files)} config files")
