"""The document last materialised at a destination.

Keeping it lets a merge tell "the repo moved ahead" from "this machine edited the
file", which is the difference between applying a change quietly and asking.
"""

import hashlib
import json
import os


def slot(ctx, dst):
    digest = hashlib.sha256(dst.encode("utf-8")).hexdigest()
    return os.path.join(ctx.state_dir, "merge", digest + ".json")


def load(ctx, dst):
    """The recorded document, or None when nothing usable was recorded."""
    try:
        with open(slot(ctx, dst), encoding="utf-8") as handle:
            return json.loads(handle.read())
    except (OSError, ValueError):
        return None


def save(ctx, dst, document):
    if ctx.dry:
        return
    path = slot(ctx, dst)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(json.dumps(document, indent=4, ensure_ascii=False) + "\n")


def forget(ctx, dst):
    if ctx.dry:
        return
    try:
        os.remove(slot(ctx, dst))
    except OSError:
        pass
