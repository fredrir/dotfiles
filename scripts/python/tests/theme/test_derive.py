import pytest

from tools.theme import oklab
from tools.theme.derive import resolve

MOCHA = ("#1e1e2e", "#cdd6f4")
LATTE = ("#eff1f5", "#4c4f69")

PALETTE = {
    "magenta": "#f5c2e7",
    "red": "#f38ba8",
    "yellow": "#f9e2af",
    "surface0": "#313244",
    "mid": "#777777",
}


def lookup(name):
    try:
        return PALETTE[name]
    except KeyError:
        raise SystemExit(f"unknown palette color: {name}")


def mocha(expression):
    return resolve(expression, lookup, *MOCHA)


def latte(expression):
    return resolve(expression, lookup, *LATTE)


def darker(candidate, reference):
    return oklab.contrast_ratio("#ffffff", candidate) > oklab.contrast_ratio("#ffffff", reference)


def test_a_bare_name_comes_from_the_palette():
    assert mocha("magenta").hex == "#f5c2e7"
    assert mocha("magenta").alpha is None


def test_a_hex_literal_is_a_color():
    assert mocha("#A1B2C3").hex == "#a1b2c3"


def test_bg_and_fg_name_the_two_anchors():
    assert mocha("bg").hex == "#1e1e2e"
    assert mocha("fg").hex == "#cdd6f4"
    assert latte("bg").hex == "#eff1f5"
    assert latte("fg").hex == "#4c4f69"


def test_an_unknown_name_is_left_to_the_lookup():
    with pytest.raises(SystemExit) as error:
        mocha("nosuch")
    assert "unknown palette color: nosuch" in str(error.value)


def test_a_ladder_step_walks_from_the_background_toward_the_foreground():
    assert mocha("bg/250").hex == "#44465a"
    assert latte("bg/150").hex == "#d5d7df"


def test_a_positive_step_is_never_mirrored():
    assert mocha("bg/250").hex == oklab.mix(*MOCHA, 0.25)
    assert latte("bg/150").hex == oklab.mix(*LATTE, 0.15)


def test_the_foreground_ladder_walks_back_toward_the_background():
    assert mocha("fg/300").hex == oklab.mix(MOCHA[1], MOCHA[0], 0.3)
    assert darker(mocha("fg/300").hex, MOCHA[1])
    assert not darker(latte("fg/300").hex, LATTE[1])


def test_a_negative_step_darkens_a_dark_theme():
    assert mocha("bg/-100").hex == "#100f1e"
    assert mocha("bg/-100").hex == oklab.mix(*MOCHA, -0.1)
    assert darker(mocha("bg/-100").hex, MOCHA[0])


def test_a_negative_step_darkens_a_light_theme_too():
    assert latte("bg/-100").hex == "#dde0e6"
    assert latte("bg/-100").hex == latte("bg/100").hex
    assert darker(latte("bg/-100").hex, LATTE[0])


def test_a_negative_foreground_step_darkens_in_both_themes():
    assert darker(mocha("fg/-100").hex, MOCHA[1])
    assert darker(latte("fg/-100").hex, LATTE[1])
    assert mocha("fg/-100").hex == mocha("fg/100").hex
    assert latte("fg/-100").hex != latte("fg/100").hex


def test_a_general_mix_walks_between_two_named_colors():
    assert mocha("red~yellow/600").hex == oklab.mix("#f38ba8", "#f9e2af", 0.6)
    assert mocha("magenta~fg/250").hex == oklab.mix("#f5c2e7", MOCHA[1], 0.25)


def test_whitespace_around_the_operators_is_stripped():
    assert mocha(" bg / 250 ").hex == "#44465a"
    assert mocha(" red ~ yellow / 600 ").hex == mocha("red~yellow/600").hex


def test_alpha_is_a_percent_of_the_resolved_color():
    resolved = mocha("magenta/30%")
    assert resolved.hex == "#f5c2e7"
    assert resolved.alpha == 0.30


def test_alpha_may_follow_a_ladder_step():
    resolved = mocha("bg/250/40%")
    assert resolved.hex == "#44465a"
    assert resolved.alpha == 0.40


def test_alpha_takes_whole_and_decimal_percents():
    assert mocha("magenta/0%").alpha == 0.0
    assert mocha("magenta/12.5%").alpha == 0.125
    assert mocha("magenta/100%").alpha == 1.0


def test_contrast_answers_with_the_more_readable_anchor():
    assert mocha("contrast(magenta)").hex == "#1e1e2e"
    assert mocha("contrast(surface0)").hex == "#cdd6f4"
    assert latte("contrast(magenta)").hex == "#4c4f69"
    assert latte("contrast(surface0)").hex == "#eff1f5"


def test_contrast_has_a_real_floor_and_falls_back_to_black_or_white():
    result = resolve("on(mid,4.5)", lookup, "#666666", "#888888").hex
    assert result in {"#000000", "#ffffff"}
    assert oklab.contrast_ratio(result, PALETTE["mid"]) >= 4.5


def test_readable_preserves_hue_while_reaching_the_requested_floor():
    result = mocha("readable(surface0,bg,4.5)").hex
    assert oklab.contrast_ratio(result, MOCHA[0]) >= 4.5
    assert result != PALETTE["surface0"]


def test_contrast_resolves_its_argument_first():
    assert mocha("contrast(bg/250)").hex == "#cdd6f4"
    assert mocha("contrast(red~yellow/600)").hex == "#1e1e2e"


def test_alpha_may_follow_contrast():
    resolved = mocha("contrast(magenta)/50%")
    assert resolved.hex == "#1e1e2e"
    assert resolved.alpha == 0.50


def test_a_ladder_step_on_a_palette_color_asks_for_an_explicit_mix():
    with pytest.raises(SystemExit) as error:
        mocha("magenta/250")
    assert "magenta~foreground/250" in str(error.value)


def test_malformed_expressions_are_rejected_by_name():
    malformed = (
        "",
        "bg/",
        "bg/abc",
        "bg/2.5",
        "magenta/150%",
        "magenta/-10%",
        "red~yellow",
        "red~yellow~blue/500",
        "bg/40%/250",
        "bg/250/40%/10",
        "contrast(bg/250",
        "contrast(magenta/30%)",
    )
    for expression in malformed:
        with pytest.raises(SystemExit) as error:
            mocha(expression)
        assert expression in str(error.value)
