import pytest

from tools.theme.schema import ANSI_KEYS, UI_KEYS, parse_profile, primitives


def profile():
    return {
        "name": "Test",
        "dark": True,
        "ui": {name: "#112233" for name in UI_KEYS},
        "ansi": {
            "normal": {name: "#223344" for name in ANSI_KEYS},
            "bright": {name: "#334455" for name in ANSI_KEYS},
        },
    }


def test_canonical_profile_becomes_namespaced_primitives():
    parsed = parse_profile(profile(), "test.toml")
    colors = primitives(parsed)
    assert colors["ui.background"] == "#112233"
    assert colors["ansi.normal.red"] == "#223344"
    assert colors["ansi.bright.red"] == "#334455"


@pytest.mark.parametrize(
    ("mutate", "expected"),
    [
        (lambda value: value.update({"tokens": {}}), "unknown 'tokens'"),
        (lambda value: value["ui"].pop("surface"), "missing 'surface'"),
        (lambda value: value["ansi"]["normal"].update({"orange": "#123456"}), "unknown 'orange'"),
        (lambda value: value["ui"].update({"primary": "blue"}), "six-digit hex"),
    ],
)
def test_profile_shape_is_strict(mutate, expected):
    value = profile()
    mutate(value)
    with pytest.raises(SystemExit) as error:
        parse_profile(value, "test.toml")
    assert expected in str(error.value)
