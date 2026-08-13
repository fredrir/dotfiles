import pytest

from tools.utils.sysinfo import hosts

DOCUMENT = """
archie {
  hostnames = archpc, archie, archie.local
  role = desktop

  CPU_COOLER = Noctua NH-D15
  MEMORY = Corsair 32 GB DDR5-6000
}

macie {
  hostnames = macie
  role = laptop
}
"""


@pytest.fixture
def hosts_file(tmp_path, monkeypatch):
    path = tmp_path / "hosts.dotfile"
    path.write_text(DOCUMENT, encoding="utf-8")
    monkeypatch.setenv("SYSINFO_CONFIG", str(path))
    monkeypatch.delenv("SYSINFO_HOST", raising=False)
    monkeypatch.setattr(hosts, "saved_host", lambda: "")
    return path


def test_hosts_carry_their_aliases_and_hardware(hosts_file):
    known = hosts.load_hosts()

    assert list(known) == ["archie", "macie"]
    assert known["archie"].hostnames == ("archpc", "archie", "archie.local")
    assert known["archie"].role == "desktop"
    assert known["archie"].hardware["cpu_cooler"] == "Noctua NH-D15"
    assert known["macie"].hardware == {}


def test_unset_hardware_falls_back_to_the_defaults(hosts_file):
    resolved = hosts.load_hosts()["archie"].resolved_hardware()

    assert resolved["cpu_cooler"] == "Noctua NH-D15"
    assert resolved["case"] == "not set"
    assert resolved["power_supply"] == "not set"


def test_a_hostname_alias_resolves_to_the_declared_name(hosts_file):
    known = hosts.load_hosts()

    assert hosts.match_hostname(known, ["archpc"]) == "archie"
    assert hosts.match_hostname(known, ["ARCHIE.LOCAL"]) == "archie"
    assert hosts.match_hostname(known, ["thinkpad"]) == ""


def test_the_environment_overrides_the_hostname(hosts_file, monkeypatch):
    monkeypatch.setenv("SYSINFO_HOST", "macie")
    monkeypatch.setattr(hosts, "local_hostnames", lambda: ("archpc",))

    assert hosts.resolve() == "macie"


def test_an_explicit_name_overrides_the_environment(hosts_file, monkeypatch):
    monkeypatch.setenv("SYSINFO_HOST", "macie")

    assert hosts.resolve("archie") == "archie"


def test_an_unknown_machine_resolves_to_nothing(hosts_file, monkeypatch):
    monkeypatch.setattr(hosts, "local_hostnames", lambda: ("thinkpad-x1",))

    assert hosts.resolve() == ""


def test_a_rendered_host_parses_back_to_itself(tmp_path):
    entry = hosts.Host(
        name="workie",
        hostnames=("thinkpad-x1", "workie"),
        role="laptop",
        hardware={"memory": "16 GB LPDDR5"},
    )
    path = tmp_path / "hosts.dotfile"
    path.write_text("archie {\n  role = desktop\n}\n", encoding="utf-8")

    hosts.append_host(entry, str(path))
    known = hosts.load_hosts(str(path))

    assert list(known) == ["archie", "workie"]
    assert known["workie"].hostnames == ("thinkpad-x1", "workie")
    assert known["workie"].role == "laptop"
    assert known["workie"].hardware == {"memory": "16 GB LPDDR5"}


def test_appending_to_an_empty_file_writes_a_usable_block(tmp_path):
    path = tmp_path / "hosts.dotfile"

    hosts.append_host(hosts.Host(name="solo", role="server"), str(path))

    assert list(hosts.load_hosts(str(path))) == ["solo"]
