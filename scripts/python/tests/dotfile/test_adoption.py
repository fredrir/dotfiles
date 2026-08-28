"""End-to-end: a change made on the machine finds its way back into the repository."""

import json
import os

import pytest
from test_merge import sandbox  # noqa: F401  (fixture)

from tools.dotfile import adoption, jsonc, merge, select
from tools.dotfile.link import cmd_link
from tools.dotfile.state import Context


def live_path(home):
    return home / ".config" / "Code" / "User" / "settings.json"


def settings(repo):
    return repo / "shared" / "vscode" / "settings.json"


def overlay(repo):
    return repo / "macos" / "vscode" / "settings.macos.json"


@pytest.fixture
def synced(sandbox, monkeypatch):  # noqa: F811
    """A materialised destination, plus a way to sync again."""
    repo, home, env = sandbox
    for name, value in env.items():
        monkeypatch.setenv(name, value)
    monkeypatch.setattr(adoption, "interactive", lambda: False)

    def sync(resolution=merge.SKIP):
        cmd_link(Context(), "test", False, [], resolution=resolution)

    sync()
    return repo, home, sync


def edit_live(home, mutate):
    path = live_path(home)
    document = json.loads(path.read_text())
    mutate(document)
    path.write_text(json.dumps(document, indent=2) + "\n")


def test_a_local_addition_is_adopted_into_the_shared_layer(synced):
    repo, home, sync = synced
    edit_live(home, lambda d: d.__setitem__("editor.fontSize", 14))

    sync(merge.LIVE)

    text = settings(repo).read_text()
    assert jsonc.loads(text)["editor.fontSize"] == 14
    assert "// git" in text, "the repo file's comments must survive adoption"


def test_a_local_change_goes_to_the_layer_that_already_owns_the_key(synced):
    repo, home, sync = synced
    # shellformat.path is defined by the macos overlay, not the shared base.
    edit_live(home, lambda d: d.__setitem__("shellformat.path", "/usr/bin/shfmt"))

    sync(merge.LIVE)

    assert jsonc.loads(overlay(repo).read_text())["shellformat.path"] == "/usr/bin/shfmt"
    assert "shellformat.path" not in jsonc.loads(settings(repo).read_text())


def test_adopting_settles_the_destination(synced):
    repo, home, sync = synced
    edit_live(home, lambda d: d.__setitem__("editor.fontSize", 14))
    sync(merge.LIVE)

    sync()  # a plain sync must now find nothing left to decide

    assert jsonc.loads(settings(repo).read_text())["editor.fontSize"] == 14
    assert json.loads(live_path(home).read_text())["editor.fontSize"] == 14


def test_discarding_restores_the_repo_value(synced):
    repo, home, sync = synced
    edit_live(home, lambda d: d.__setitem__("git.autofetch", False))

    sync(merge.REPO)

    assert json.loads(live_path(home).read_text())["git.autofetch"] is True
    assert jsonc.loads(settings(repo).read_text())["git.autofetch"] is True


def test_drift_blocks_a_headless_sync_and_leaves_the_file_alone(synced):
    repo, home, sync = synced
    edit_live(home, lambda d: d.__setitem__("editor.fontSize", 14))
    before = live_path(home).read_text()

    with pytest.raises(SystemExit):
        sync()

    assert live_path(home).read_text() == before
    assert "editor.fontSize" not in jsonc.loads(settings(repo).read_text())


def answering(monkeypatch, decisions):
    """Drive the selector without a terminal: every row gets `decisions` in order."""
    monkeypatch.setattr(adoption, "interactive", lambda: True)

    def fake(dst_label, changes):
        assert changes and all(isinstance(c, select.Change) for c in changes)
        return {index: decisions[index] for index in range(len(changes))}

    monkeypatch.setattr(select, "resolve", fake)


def test_the_selector_sends_a_key_to_the_overlay_it_was_told_to(synced, monkeypatch):
    repo, home, sync = synced
    edit_live(home, lambda d: d.__setitem__("editor.fontSize", 14))
    answering(monkeypatch, {0: "target:macos"})

    sync()

    assert jsonc.loads(overlay(repo).read_text())["editor.fontSize"] == 14
    assert "editor.fontSize" not in jsonc.loads(settings(repo).read_text())


def test_the_selector_can_put_a_key_on_the_ignore_list(synced, monkeypatch):
    repo, home, sync = synced
    edit_live(home, lambda d: d.__setitem__("workbench.colorTheme", "Nord"))
    answering(monkeypatch, {0: "ignore"})

    sync()

    listed = (repo / "shared" / "vscode" / "merge.dotfile").read_text()
    assert "ignore" in listed and "workbench.colorTheme" in listed
    # ignored from now on: the key stays live and stops being drift
    sync()
    assert json.loads(live_path(home).read_text())["workbench.colorTheme"] == "Nord"
    assert "workbench.colorTheme" not in jsonc.loads(settings(repo).read_text())


def test_aborting_the_selector_changes_nothing(synced, monkeypatch):
    repo, home, sync = synced
    edit_live(home, lambda d: d.__setitem__("editor.fontSize", 14))
    before = settings(repo).read_text()
    monkeypatch.setattr(adoption, "interactive", lambda: True)
    monkeypatch.setattr(select, "resolve", lambda *args, **kwargs: None)

    with pytest.raises(SystemExit):
        sync()

    assert settings(repo).read_text() == before


class FakeEntry:
    def __init__(self, root):
        self.base_dir = os.path.join(root, "shared", "vscode")
        self.dst = "/dev/null"

    def paths(self):
        return [os.path.join(self.base_dir, "settings.json")]


class FakeCtx:
    def __init__(self, root, groups):
        self.root = root
        self.link_groups = groups


def test_a_nested_group_is_offered_by_its_basename_not_its_parent(tmp_path):
    """merge reads the tag off the group directory, so linux/arch is 'arch'."""
    ctx = FakeCtx(str(tmp_path), ["shared", "linux/common", "linux/arch"])
    slots = adoption.overlay_slots(ctx, FakeEntry(str(tmp_path)))

    assert set(slots) == {"common", "arch"}
    assert "linux" not in slots, "'linux' is a directory, never an overlay tag"
    assert slots["arch"].endswith("linux/arch/vscode/settings.arch.json")


def test_an_override_directory_is_never_offered_as_a_layer(tmp_path):
    ctx = FakeCtx(str(tmp_path), ["shared", "macos", "macos/overrides/laptop"])
    slots = adoption.overlay_slots(ctx, FakeEntry(str(tmp_path)))

    assert set(slots) == {"macos"}


def test_force_is_decisive_and_never_opens_the_selector(synced, monkeypatch):
    """--force is an explicit override; being asked anyway would defeat it."""
    _repo, home, _sync = synced
    edit_live(home, lambda d: d.__setitem__("git.autofetch", False))
    monkeypatch.setattr(adoption, "interactive", lambda: True)
    monkeypatch.setattr(select, "resolve", lambda *a, **k: pytest.fail("selector opened"))

    cmd_link(Context(), "test", False, [], force=True)

    assert json.loads(live_path(home).read_text())["git.autofetch"] is True
