import pytest

from tools.surface import values


def test_an_unknown_source_offers_nothing():
    assert values.lines("no-such-source", []) == []


def test_a_source_that_cannot_answer_stays_quiet(monkeypatch):
    def broken():
        raise RuntimeError("config/hosts.dotfile is half written")

    monkeypatch.setitem(values.PROVIDERS, "hosts", broken)
    assert values.lines("hosts", []) == []


def test_a_description_is_offered_alongside_its_value(monkeypatch):
    monkeypatch.setitem(values.PROVIDERS, "hosts", lambda: [("archie", "desktop")])
    assert values.lines("hosts", []) == ["archie:desktop"]


def test_a_colon_in_a_value_is_escaped_so_it_stays_one_value(monkeypatch):
    monkeypatch.setitem(values.PROVIDERS, "runs", lambda: ["archie@a3f:19c2e"])
    assert values.lines("runs", []) == [r"archie@a3f\:19c2e"]


def test_arguments_reach_the_provider(monkeypatch):
    monkeypatch.setitem(values.PROVIDERS, "override-names", lambda group="": [group])
    assert values.lines("override-names", ["linux/hyprland"]) == ["linux/hyprland"]


@pytest.mark.parametrize("source", sorted(values.PROVIDERS))
def test_every_source_answers_without_raising(source):
    assert isinstance(values.lines(source, []), list)


def test_profiles_are_read_from_the_repository(tmp_path, monkeypatch):
    for profile in ("macos", "ubuntu/server"):
        directory = tmp_path / "environment" / profile
        directory.mkdir(parents=True)
        (directory / "manifest").write_text("shared\n")
    (tmp_path / "config").mkdir()
    (tmp_path / "config" / "targets.dotfile").write_text("")
    monkeypatch.setenv("DOTFILE_ROOT", str(tmp_path))
    # The profile this host can actually run carries a note, the others do not.
    offered = [line.split(":")[0] for line in values.lines("profiles", [])]
    assert offered == ["macos", "ubuntu/server"]
