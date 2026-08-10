from pathlib import Path

import tomlkit

from tools.core.process import capture
from tools.transcript import config


def resolve_repo(path):
    result = capture(["git", "-C", str(path), "rev-parse", "--show-toplevel"])
    output = (result.stdout or "").strip()
    if result.returncode == 0 and output:
        return Path(output)
    return Path(path)


def load_document():
    path = config._config_path()
    try:
        text = path.read_text()
    except OSError:
        text = ""
    return tomlkit.parse(text), path


def save_document(document, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(tomlkit.dumps(document))
    config._load_file.cache_clear()


def _multiline_array(values=()):
    array = tomlkit.array()
    array.multiline(True)
    for value in values:
        array.append(str(value))
    return array


def track(directory, name="", group=""):
    root = resolve_repo(directory)
    project = name.strip() or root.name
    document, path = load_document()
    if "projects" not in document:
        document["projects"] = _multiline_array()
    projects = document["projects"]
    added = project.lower() not in {str(item).lower() for item in projects}
    if added:
        projects.append(project)
    if project.lower() != root.name.lower():
        aliases = document.setdefault("aliases", tomlkit.table())
        aliases[f"{root.parent.name}/{root.name}"] = project
    if group:
        groups = document.setdefault("groups", tomlkit.table())
        if group not in groups:
            groups[group] = _multiline_array()
        members = groups[group]
        if project.lower() not in {str(item).lower() for item in members}:
            members.append(project)
    save_document(document, path)
    return project, added


def untrack(name):
    document, path = load_document()
    lowered = name.lower()
    removed = False
    projects = document.get("projects")
    if projects is not None:
        keep = [str(item) for item in projects if str(item).lower() != lowered]
        if len(keep) != len(projects):
            document["projects"] = _multiline_array(keep)
            removed = True
    aliases = document.get("aliases")
    if aliases is not None:
        for key in [k for k in aliases if str(aliases[k]).lower() == lowered]:
            del aliases[key]
            removed = True
    groups = document.get("groups")
    if groups is not None:
        for group_name in list(groups):
            members = groups[group_name]
            keep = [str(item) for item in members if str(item).lower() != lowered]
            if len(keep) != len(members):
                removed = True
                if keep:
                    groups[group_name] = _multiline_array(keep)
                else:
                    del groups[group_name]
    if removed:
        save_document(document, path)
    return removed
