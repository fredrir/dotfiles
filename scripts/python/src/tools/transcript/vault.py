import os
import re
from datetime import datetime
from pathlib import Path

from tools.transcript import config, render

MANAGED_KEYS = {
    "provider",
    "project",
    "model",
    "session",
    "cwd",
    "created",
    "updated",
    "source",
    "status",
    "import",
    "tags",
    "obsidianUIMode",
    "cssclasses",
}

SESSION_KEY_RE = re.compile(r"^session:\s*(\S+)\s*$", re.MULTILINE)

CAPTURES_PROJECT = "Captures"


def project_of(cwd):
    if not cwd:
        return "Unsorted"
    path = Path(cwd)
    lowered = [part.lower() for part in path.parts]
    for pattern, name in config.project_aliases().items():
        width = len(pattern)
        for start in range(len(lowered) - width + 1):
            if tuple(lowered[start : start + width]) == pattern:
                return name
    allowed = config.allowed_projects()
    for part in path.parts:
        if part.lower() in allowed:
            return part
    if path == Path.home():
        return "Home"
    name = path.name.strip()
    return name or "Unsorted"


def group_subfolder(group, project):
    if project.lower().startswith(group.lower()):
        rest = project[len(group) :].lstrip("-_ ")
        if rest:
            return rest
    return project


def folder_for(project):
    for group, members in config.project_groups().items():
        if project.lower() in members:
            if group.lower() in config.nested_groups():
                sub = group_subfolder(group, project)
                if sub and sub.lower() != group.lower():
                    return f"{group}/{sub}"
            return group
    return project


def directory_for(project):
    relative = Path(folder_for(project))
    destination = config.destination_dir(relative.parts[0])
    if destination is None:
        return config.transcripts_dir() / relative
    return destination.joinpath(*relative.parts[1:])


def note_roots():
    roots = [config.transcripts_dir()]
    for group in config.group_destinations():
        destination = config.destination_dir(group)
        if destination is not None and destination not in roots:
            roots.append(destination)
    return roots


def slugify(text, limit=40):
    text = re.sub(r"[^a-z0-9æøå]+", "-", text.lower()).strip("-")
    if len(text) > limit:
        text = text[:limit].rsplit("-", 1)[0] or text[:limit]
    return text.strip("-") or "session"


def atomic_write(path, content):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.tmp")
    tmp.write_text(content, encoding="utf-8")
    os.replace(tmp, path)


def read_frontmatter(path):
    try:
        text = Path(path).read_text(errors="replace")
    except OSError:
        return {}
    if not text.startswith("---\n"):
        return {}
    end = text.find("\n---", 4)
    if end < 0:
        return {}
    fields = {}
    for line in text[4:end].splitlines():
        if ":" in line and not line.startswith((" ", "\t", "-", "#")):
            key, _, value = line.partition(":")
            fields[key.strip()] = value.strip()
    return fields


def existing_notes():
    mapping = {}
    for root in note_roots():
        if not root.is_dir():
            continue
        for path in root.rglob("*.md"):
            try:
                head = path.read_text(errors="replace")[:2000]
            except OSError:
                continue
            if not head.startswith("---"):
                continue
            match = SESSION_KEY_RE.search(head.split("\n---", 1)[0])
            if match:
                mapping[match.group(1)] = path
    return mapping


def compose(fields, preserved, body):
    lines = ["---"]
    for key, value in fields.items():
        if value:
            lines.append(f"{key}: {value}")
    lines.extend(f"{key}: {value}" for key, value in preserved.items())
    lines.append("---")
    return "\n".join(lines) + "\n\n" + body.rstrip("\n") + "\n"


def dedupe_path(directory, base):
    path = directory / f"{base}.md"
    counter = 1
    while path.exists():
        path = directory / f"{base} ({counter}).md"
        counter += 1
    return path


def note_path_for(session, project):
    stamp = session.started or datetime.now().astimezone()
    directory = directory_for(project) / f"{stamp:%Y-%m}" / session.provider
    slug = slugify(session.title or "session")
    return dedupe_path(directory, f"{stamp:%d}-{slug}")


def save_session(session, source, redactor, index=None, include_tools=False):
    project = project_of(session.cwd)
    if index is None:
        index = existing_notes()
    existing = index.get(session.session_id)
    path = existing if existing is not None else note_path_for(session, project)
    old = read_frontmatter(path) if path.exists() else {}
    try:
        updated_stamp = datetime.fromtimestamp(
            Path(session.source_path).stat().st_mtime
        ).astimezone()
    except OSError:
        updated_stamp = datetime.now().astimezone()
    fields = {
        "provider": session.provider,
        "project": old.get("project") or project,
        "model": session.model,
        "session": session.session_id,
        "cwd": session.cwd,
        "created": f"{session.started:%Y-%m-%dT%H:%M}" if session.started else "",
        "updated": f"{updated_stamp:%Y-%m-%dT%H:%M}",
        "source": old.get("source") or source,
        "status": old.get("status") or "inbox",
        "import": "degraded" if session.degraded else "",
        "tags": old.get("tags") or "[transcript]",
        "obsidianUIMode": "preview",
        "cssclasses": "transcript",
    }
    preserved = {key: value for key, value in old.items() if key not in MANAGED_KEYS}
    body = redactor(render.render_session(session, include_tools))
    atomic_write(path, compose(fields, preserved, body))
    index[session.session_id] = path
    return path, existing is not None


def save_capture(provider, text, redactor):
    now = datetime.now().astimezone()
    first_line = next((line for line in text.strip().splitlines() if line.strip()), "capture")
    slug = slugify(render.clean_inline(first_line, 60))
    directory = directory_for(CAPTURES_PROJECT) / f"{now:%Y-%m}" / provider
    path = dedupe_path(directory, f"{now:%d}-{slug}")
    fields = {
        "provider": provider,
        "project": CAPTURES_PROJECT,
        "created": f"{now:%Y-%m-%dT%H:%M}",
        "source": "capture",
        "status": "inbox",
        "tags": "[transcript]",
        "obsidianUIMode": "preview",
        "cssclasses": "transcript",
    }
    body = redactor(render.render_capture(provider, now, text))
    atomic_write(path, compose(fields, {}, body))
    return path


def add_daily_link(note_path, label):
    now = datetime.now().astimezone()
    daily = config.vault_root() / f"{now:%Y-%m-%d}.md"
    relative = Path(note_path).relative_to(config.vault_root())
    link = f"- {now:%H:%M} [[{str(relative)[:-3]}|{label}]]"
    try:
        existing = daily.read_text(errors="replace")
    except OSError:
        existing = ""
    if link in existing:
        return
    text = existing.rstrip("\n")
    text = f"{text}\n{link}\n" if text else f"{link}\n"
    atomic_write(daily, text)
