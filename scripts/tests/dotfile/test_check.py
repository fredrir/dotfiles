import pytest
import typer

from tools.dotfile import check


class FakeContext:
    def __init__(self, root):
        self.root = str(root)
        self.environment_dir = str(root / "environment")
        self.requires_file = str(root / "requires.dotfile")


@pytest.fixture
def ctx(tmp_path):
    (tmp_path / "shared").mkdir()
    (tmp_path / "environment" / "macos").mkdir(parents=True)
    return FakeContext(tmp_path)


def write_requires(ctx, text):
    with open(ctx.requires_file, "w", encoding="utf-8") as handle:
        handle.write(text)


def test_reads_every_entry_kind(ctx):
    write_requires(
        ctx,
        "# a comment\nshared {\n  git\n  nvim = neovim\n  ?docker  # trailing\n"
        "  font Hack Nerd Font Mono\n  file ~/.config/hypr/wallpaper.png\n}\n",
    )
    assert check.load_requirements(ctx) == {
        "shared": [
            ("command", "git", "git", False),
            ("command", "nvim", "neovim", False),
            ("command", "docker", "docker", True),
            ("font", "Hack Nerd Font Mono", "Hack Nerd Font Mono", False),
            ("file", "~/.config/hypr/wallpaper.png", "~/.config/hypr/wallpaper.png", False),
        ]
    }


def test_missing_file_means_no_requirements(ctx):
    assert check.load_requirements(ctx) == {}


def test_rejects_a_group_that_is_not_in_the_repository(ctx):
    write_requires(ctx, "linux/kde {\n  konsole\n}\n")
    with pytest.raises(typer.Exit):
        check.load_requirements(ctx)


def test_rejects_an_entry_outside_a_group(ctx):
    write_requires(ctx, "git\n")
    with pytest.raises(typer.Exit):
        check.load_requirements(ctx)


def test_accepts_a_profile_for_this_platform(ctx):
    assert check.wrong_platform(ctx, "macos", "macos") == ""


def test_warns_about_a_profile_from_another_platform(ctx):
    assert check.wrong_platform(ctx, "arch-linux/kde", "macos") == "not a macos profile"


def test_stays_quiet_when_the_platform_has_no_profiles_in_the_repository(ctx):
    assert check.wrong_platform(ctx, "ubuntu/server", "ubuntu") == ""


def test_font_matches_a_family_and_its_weights():
    assert not check.font_missing("Hack Nerd Font Mono", {"hacknerdfontmonoregular"})
    assert not check.font_missing("Hack Nerd Font Mono", {"hacknerdfontmonobolditalic"})
    assert not check.font_missing("Hack Nerd Font Mono", {"hacknerdfontmono"})


def test_font_does_not_match_a_longer_family_name():
    assert check.font_missing("Noto Sans", {"notosansadlamregular", "notosansbatakregular"})
    assert check.font_missing("Hack Nerd Font Mono", {"hacknerdfontregular"})


def test_brewfile_lists_formulae_and_casks(tmp_path):
    path = tmp_path / "Brewfile"
    path.write_text(
        '# comment\ntap "homebrew/bundle"\nbrew "starship"\n'
        'brew \'eza\'\nbrew "some/tap/tool"\ncask "kitty"\n'
    )
    assert check.read_brewfile(str(path)) == ["starship", "eza", "tool", "kitty"]


def test_pkglist_drops_comments_and_blank_lines(tmp_path):
    path = tmp_path / "pkglist.txt"
    path.write_text("git\n\n# comment\nneovim\n")
    assert check.read_pkglist(str(path)) == ["git", "neovim"]


def test_row_lists_items_under_the_label(capsys):
    check.emit(check.row("bad", "tools", "2 missing", [("yazi", ""), ("rg", "ripgrep")]), False)
    assert capsys.readouterr().out == "  ✗ tools      2 missing\n      yazi\n      rg  ripgrep\n"


def test_items_align_on_the_widest_name_that_carries_a_note():
    assert check.align([("short", "note"), ("much-longer-name", "")]) == [
        ("short", "note"),
        ("much-longer-name", ""),
    ]
    assert check.align([("short", "note"), ("much-longer-name", "other")])[0] == (
        "short           ",
        "note",
    )


def test_clips_items_and_says_how_many_were_dropped():
    items = [(f"tool{index}", "") for index in range(15)]
    assert check.clip(items, False)[-1] == ("+3 more", "")
    assert check.clip(items, True) == items


def test_a_requirement_in_two_groups_is_checked_once():
    requirements = {
        "shared": [("command", "git", "git", True)],
        "macos": [("command", "git", "git", False)],
    }
    assert check.profile_requirements(requirements, ["shared", "macos"]) == [
        ("command", "git", "git", False)
    ]
