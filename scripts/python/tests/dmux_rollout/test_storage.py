import json
import os

import pytest

from tools.dmux_rollout.errors import StateError
from tools.dmux_rollout.storage import RolloutStore

from .helpers import release


def test_manifest_and_journal_are_private_and_resume(tmp_path):
    root = tmp_path / "state"
    store = RolloutStore(root)
    item = release(tmp_path)

    with store.exclusive():
        store.create(item)
        store.checkpoint(item, "one", {"pid": 42, "epoch": "e"})

    loaded = store.load(item.release_id)
    assert loaded.checkpoints["one"]["evidence"]["pid"] == 42
    assert (root.stat().st_mode & 0o777) == 0o700
    assert (store.manifest_path(item.release_id).stat().st_mode & 0o777) == 0o600
    journal = store.release_dir(item.release_id).joinpath("journal.jsonl").read_text().splitlines()
    assert [json.loads(line)["kind"] for line in journal] == ["release_planned", "checkpoint"]


def test_lock_excludes_a_second_runner(tmp_path):
    store = RolloutStore(tmp_path / "state")

    with (
        store.exclusive(),
        pytest.raises(StateError, match="another dmux-rollout"),
        store.exclusive(),
    ):
        pass


def test_state_root_symlink_is_refused(tmp_path):
    real = tmp_path / "real"
    real.mkdir(mode=0o700)
    link = tmp_path / "state"
    link.symlink_to(real, target_is_directory=True)
    store = RolloutStore(link)

    with pytest.raises(StateError, match="not a real directory"), store.exclusive():
        pass


def test_group_readable_state_root_is_refused(tmp_path):
    root = tmp_path / "state"
    root.mkdir(mode=0o750)
    os.chmod(root, 0o750)
    store = RolloutStore(root)

    with pytest.raises(StateError, match="mode 0700"), store.exclusive():
        pass


def test_corrupt_manifest_fails_closed(tmp_path):
    root = tmp_path / "state"
    store = RolloutStore(root)
    item = release(tmp_path)
    with store.exclusive():
        store.create(item)
    path = store.manifest_path(item.release_id)
    path.write_text("{broken", encoding="utf-8")
    os.chmod(path, 0o600)

    with pytest.raises(StateError, match="canonical JSON"):
        store.load(item.release_id)
