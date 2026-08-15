from types import SimpleNamespace

from tools.dotfile.secret import vault


def test_package_entries_skips_embedded_git_checkouts(tmp_path):
    pkg = tmp_path / "shared" / "wezterm"
    (pkg / "types" / ".git").mkdir(parents=True)
    (pkg / "types" / "page.html.tmpl").write_text("{{ foreign.var }}\n")
    (pkg / "theme.lua.tmpl").write_text("{{ colors.accent }}\n")

    ctx = SimpleNamespace(targets={}, home=str(tmp_path / "home"))
    entries = vault.package_entries(ctx, str(pkg), "shared/wezterm", False)

    assert [entry.rel for entry in entries] == ["theme.lua.tmpl"]


def test_package_entries_follows_ordinary_subdirectories(tmp_path):
    pkg = tmp_path / "shared" / "wezterm"
    (pkg / "wez").mkdir(parents=True)
    (pkg / "wez" / "theme.lua.tmpl").write_text("{{ colors.accent }}\n")

    ctx = SimpleNamespace(targets={}, home=str(tmp_path / "home"))
    entries = vault.package_entries(ctx, str(pkg), "shared/wezterm", False)

    assert [entry.rel for entry in entries] == ["wez/theme.lua.tmpl"]
