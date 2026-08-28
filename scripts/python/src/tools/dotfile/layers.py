"""Which repo file receives a key a user decided to adopt.

A merged config is built from an ordered list of layer files: index 0 is the shared
base and every later entry is a platform overlay that wins over it. An overlay only
merges when it sits in a group directory named for its tag and carries that same tag
in its filename, so macos/vscode/settings.macos.json overlays shared/vscode/settings.json
and linux/arch/vscode/settings.arch.json overlays it under the arch group. The first
half of this module inverts that naming to answer "where does this key go?".

The second half writes merge.dotfile, whose `ignore` patterns name keys a merge must
leave alone. `/` separates nesting levels there and a dot is an ordinary character, so
"editor.formatOnSave" is one flat key and ("[lua]", "editor.tabSize") renders as
"[lua]/editor.tabSize".
"""

import os

from tools.dotfile import jsonspan

SHARED = "shared"
TARGET = "target"

NAME = "merge.dotfile"
IGNORE = "ignore"
GAP = "  "

BLANKS = " \t\n\r\f\v"


def resolve(path_to_file, root):
    """A layer path, absolute already or read as relative to root."""
    if os.path.isabs(path_to_file):
        return path_to_file
    return os.path.join(root, path_to_file)


def relative(path_to_file, root):
    """The layer's path below root, with forward slashes."""
    return os.path.relpath(resolve(path_to_file, root), root).replace(os.sep, "/")


def stem_for(base, tag):
    """A filename's part before its `.<tag>.json` suffix, or "" when it has none."""
    suffix = f".{tag}.json"
    if not base.endswith(suffix):
        return ""
    return base[: -len(suffix)]


def layer_name(path_to_file, root):
    """The overlay tag for a layer file: "shared" for the base, else the platform
    tag ("macos", "linux") taken from the settings.<tag>.json suffix.

    The tag has to be an ancestor directory's name as well, which is what keeps
    settings.macos.json in a linux group from passing itself off as a macos overlay.
    """
    segments = relative(path_to_file, root).split("/")
    for tag in segments[:-1]:
        if stem_for(segments[-1], tag):
            return tag
    return SHARED


def base_stem(path_to_file, root):
    """A layer filename with both its tag and its `.json` suffix stripped."""
    base = relative(path_to_file, root).rsplit("/", 1)[-1]
    stripped = stem_for(base, layer_name(path_to_file, root))
    if stripped:
        return stripped
    return base.removesuffix(".json")


def decision_name(decision):
    """The layer a decision names: "shared" itself, or the <name> in "target:<name>"."""
    if decision == SHARED:
        return SHARED
    kind, sep, name = decision.partition(":")
    if not sep or kind != TARGET or not name:
        raise ValueError(f"expected 'shared' or 'target:<name>', got '{decision}'")
    return name


def target_layer(layers, decision, root):
    """decision is "shared" | "target:<name>". Return the layer file path to
    write to, or None if that layer does not exist yet. Always absolute."""
    name = decision_name(decision)
    for path_to_file in layers:
        if layer_name(path_to_file, root) == name:
            return resolve(path_to_file, root)
    return None


def defines(path_to_file, path):
    """True when this layer file already has a member at path."""
    if not os.path.isfile(path_to_file):
        return False
    with open(path_to_file, encoding="utf-8", newline="") as handle:
        return jsonspan.key_span(handle.read(), path) is not None


def owning_layer(layers, path, root):
    """The file that already defines key `path`, searched from the LAST overlay
    backwards (the one that actually wins). None if no layer defines it.
    Used by `--resolve live`: adopt into the owning layer, else the shared base.
    Always absolute."""
    for path_to_file in reversed(layers):
        if defines(resolve(path_to_file, root), path):
            return resolve(path_to_file, root)
    return None


def overlay_path(layers, name, root):
    """Where a not-yet-existing overlay for platform <name> WOULD live, so a decision
    of target:linux on a mac can create linux/…/settings.linux.json.

    The tag has to name the group directory as well as the file, so the base layer's
    group segment is swapped for <name> and the tag is spliced into its filename:
    shared/vscode/settings.json + "linux" gives linux/vscode/settings.linux.json. A
    group nested inside another (linux/arch) can only be reproduced when a layer for
    that name is already among `layers`. Always absolute.
    """
    if not layers:
        raise ValueError("no layers to derive an overlay path from")
    existing = target_layer(layers, f"{TARGET}:{name}", root)
    if existing is not None:
        return existing
    segments = relative(layers[0], root).split("/")
    segments[-1] = f"{base_stem(layers[0], root)}.{name}.json"
    if len(segments) > 1:
        segments[0] = name
    return os.path.join(root, *segments)


def unrepresentable(key):
    """Why merge.dotfile cannot hold this key name verbatim, or "" when it can."""
    if not key:
        return "it is empty"
    if "/" in key:
        return "'/' separates nesting levels"
    if "#" in key:
        return "'#' starts a comment"
    if "\n" in key or "\r" in key:
        return "a pattern is one line"
    if key != key.strip(BLANKS):
        return "the reader trims the space around a pattern"
    return ""


def render_pattern(path):
    """Path tuple -> merge.dotfile pattern. ("[lua]","editor.tabSize") ->
    "[lua]/editor.tabSize"; ("cSpell.userWords",) -> "cSpell.userWords".

    Raises when a key cannot survive the round trip, which `/` and `#` cannot: the
    reader has no escape syntax, so either one would name a different key. A `*` or
    `?` does survive, but reads back as a glob, so the pattern covers its siblings
    too -- deliberately allowed, because "*.zsh" is an ordinary VS Code key.
    """
    for key in path:
        reason = unrepresentable(key)
        if reason:
            raise ValueError(f"key {key!r} cannot be a merge.dotfile pattern: {reason}")
    return "/".join(path)


def read_patterns(text):
    """The patterns a merge.dotfile already lists."""
    found = []
    for line in text.splitlines():
        fields = line.split("#", 1)[0].strip(BLANKS).split(None, 1)
        if len(fields) == 2 and fields[0] == IGNORE:
            found.append(fields[1])
    return found


def column(text):
    """The gap the file already puts between `ignore` and its pattern, else two spaces."""
    for line in text.splitlines():
        directive = line.split("#", 1)[0].strip(BLANKS)
        if not directive.startswith(IGNORE):
            continue
        rest = directive[len(IGNORE) :]
        gap = rest[: len(rest) - len(rest.lstrip(" \t"))]
        if gap and rest.strip(BLANKS):
            return gap
    return GAP


def appended(text, pattern):
    """The file's bytes with one more ignore line, in the column style already in use."""
    nl = "\r\n" if "\r\n" in text else "\n"
    line = f"{IGNORE}{column(text)}{pattern}{nl}"
    if not text:
        return line
    if not text.endswith(("\n", "\r")):
        return text + nl + line
    return text + line


def add_ignore(path_to_merge_dotfile, path):
    """Append `ignore  <pattern>` for key path. Create the file if missing.
    Idempotent: never add a pattern that is already present verbatim.
    Returns False when it was already there and not a byte was written."""
    pattern = render_pattern(path)
    text = ""
    if os.path.isfile(path_to_merge_dotfile):
        with open(path_to_merge_dotfile, encoding="utf-8", newline="") as handle:
            text = handle.read()
    if pattern in read_patterns(text):
        return False
    parent = os.path.dirname(path_to_merge_dotfile)
    if parent:
        os.makedirs(parent, exist_ok=True)
    with open(path_to_merge_dotfile, "w", encoding="utf-8", newline="") as handle:
        handle.write(appended(text, pattern))
    return True
