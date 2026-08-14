from types import SimpleNamespace

from tools.utils.sysinfo import identity


def test_macos_hostname_prefers_local_host_name(monkeypatch):
    monkeypatch.delenv("SYSINFO_HOSTNAME", raising=False)
    monkeypatch.setattr(identity.sys, "platform", "darwin")
    monkeypatch.setattr(identity.shutil, "which", lambda _name: "/usr/sbin/scutil")
    monkeypatch.setattr(
        identity,
        "capture",
        lambda command: SimpleNamespace(
            returncode=0,
            stdout="Fredrirs-MacBook-Pro\n" if command[-1] == "LocalHostName" else "",
        ),
    )
    monkeypatch.setattr(identity.socket, "gethostname", lambda: "173")

    assert identity.display_hostname() == "Fredrirs-MacBook-Pro"


def test_hostname_override_has_priority(monkeypatch):
    monkeypatch.setenv("SYSINFO_HOSTNAME", "my-machine")
    monkeypatch.setattr(identity.socket, "gethostname", lambda: "173")

    assert identity.display_hostname() == "my-machine"
