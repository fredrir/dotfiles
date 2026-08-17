import pytest

from tools.dmux_rollout.errors import StateError
from tools.dmux_rollout.model import Release

from .helpers import release


def test_release_checkpoint_is_one_shot(tmp_path):
    item = release(tmp_path)

    assert item.checkpoint("built", {"sha256": "abc"}) is True
    assert item.checkpoint("built", {"sha256": "different"}) is False
    assert item.checkpoints["built"]["evidence"] == {"sha256": "abc"}
    assert item.data["journal_seq"] == 1


def test_release_rejects_unknown_manifest_keys(tmp_path):
    item = release(tmp_path)
    item.data["surprise"] = True

    with pytest.raises(StateError, match="keys differ"):
        Release.from_json(item.data)


def test_smoke_identity_can_be_replayed_but_not_replaced(tmp_path):
    item = release(tmp_path)
    space = "11111111-1111-4111-8111-111111111111"
    host = "22222222-2222-4222-8222-222222222222"

    item.set_smoke_identity(space_uid=space, host_uid=host)
    item.set_smoke_identity(space_uid=space, host_uid=host)

    with pytest.raises(StateError, match="changed identity"):
        item.set_smoke_identity(space_uid="33333333-3333-4333-8333-333333333333", host_uid=host)
