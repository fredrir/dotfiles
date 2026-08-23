import pytest

from tools.dmux_rollout.errors import StateError
from tools.dmux_rollout.model import PHASES, Release

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


def test_phases_are_ordered_and_never_regress(tmp_path):
    item = release(tmp_path)
    assert item.phase == "planned"
    assert PHASES.index("deployed") < PHASES.index("verified") < PHASES.index("canary_mac")

    item.set_phase("mac_deployed")
    item.set_phase("mac_deployed")  # re-entering the current phase is allowed
    item.set_phase("deployed")
    with pytest.raises(StateError, match="cannot regress from 'deployed' to 'built'"):
        item.set_phase("built")
    assert item.phase == "deployed"

    # A resumed step that finds the release further along leaves it there.
    assert item.advance_phase("built") is False
    assert item.advance_phase("canary_mac") is True
    assert item.phase == "canary_mac"
    with pytest.raises(StateError, match="cannot regress"):
        item.set_phase("verified")

    # Rollback is reachable from anywhere, and terminal.
    item.set_phase("rolled_back")
    with pytest.raises(StateError, match="cannot regress from 'rolled_back'"):
        item.set_phase("flipped")


def test_unknown_phase_is_refused_and_r5_shape_still_loads(tmp_path):
    item = release(tmp_path)
    with pytest.raises(StateError, match="unknown release phase 'arch_deployed'"):
        item.set_phase("arch_deployed")

    # r5's on-disk manifest: schema 1, phase mac_deployed, the runtime-only
    # launchd witness under rollback.mac. It must keep loading verbatim.
    r5_shape = dict(item.data)
    r5_shape["phase"] = "mac_deployed"
    r5_shape["rollback"] = {
        "mac": {"files": [], "launchd_dmux_wez_first": ""},
        "archie": {"dmux_wez_first": ""},
    }
    assert Release.from_json(r5_shape).phase == "mac_deployed"

    r5_shape["phase"] = "shipped"
    with pytest.raises(StateError, match="unknown release phase 'shipped'"):
        Release.from_json(r5_shape)
