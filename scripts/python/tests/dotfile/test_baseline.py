import hashlib
import json
import os

import pytest

from tools.dotfile import baseline

DOCUMENT = {"git.autofetch": True, "[lua]": {"editor.tabSize": 2}}


class Ctx:
    def __init__(self, state_dir):
        self.state_dir = str(state_dir)
        self.dry = False


@pytest.fixture
def ctx(tmp_path):
    return Ctx(tmp_path / "state")


def test_slot_is_the_hashed_destination_under_the_state_dir(ctx):
    dst = "/home/me/.config/Code/User/settings.json"
    digest = hashlib.sha256(dst.encode("utf-8")).hexdigest()
    assert baseline.slot(ctx, dst) == os.path.join(ctx.state_dir, "merge", digest + ".json")


def test_save_then_load_round_trips(ctx):
    baseline.save(ctx, "/x/settings.json", DOCUMENT)
    assert baseline.load(ctx, "/x/settings.json") == DOCUMENT


def test_destinations_keep_separate_records(ctx):
    baseline.save(ctx, "/x/settings.json", {"a": 1})
    baseline.save(ctx, "/y/settings.json", {"a": 2})
    assert baseline.load(ctx, "/x/settings.json") == {"a": 1}
    assert baseline.load(ctx, "/y/settings.json") == {"a": 2}


def test_load_is_none_when_nothing_was_recorded(ctx):
    assert baseline.load(ctx, "/x/settings.json") is None


def test_load_is_none_when_the_record_is_unreadable(ctx):
    path = baseline.slot(ctx, "/x/settings.json")
    os.makedirs(os.path.dirname(path))
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("{ truncated")
    assert baseline.load(ctx, "/x/settings.json") is None


def test_an_empty_document_is_not_a_missing_record(ctx):
    baseline.save(ctx, "/x/settings.json", {})
    assert baseline.load(ctx, "/x/settings.json") == {}


def test_a_dry_run_records_nothing(ctx):
    ctx.dry = True
    baseline.save(ctx, "/x/settings.json", DOCUMENT)
    assert baseline.load(ctx, "/x/settings.json") is None


def test_forget_drops_the_record(ctx):
    baseline.save(ctx, "/x/settings.json", DOCUMENT)
    baseline.forget(ctx, "/x/settings.json")
    assert baseline.load(ctx, "/x/settings.json") is None


def test_forget_is_quiet_without_a_record(ctx):
    baseline.forget(ctx, "/x/settings.json")


def test_a_dry_run_forgets_nothing(ctx):
    baseline.save(ctx, "/x/settings.json", DOCUMENT)
    ctx.dry = True
    baseline.forget(ctx, "/x/settings.json")
    assert baseline.load(ctx, "/x/settings.json") == DOCUMENT


def test_the_record_is_plain_json(ctx):
    baseline.save(ctx, "/x/settings.json", DOCUMENT)
    with open(baseline.slot(ctx, "/x/settings.json"), encoding="utf-8") as handle:
        assert json.loads(handle.read()) == DOCUMENT
