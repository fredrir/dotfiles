import math
import re

import pytest

from tools.theme.oklab import (
    contrast_ratio,
    from_oklab,
    is_dark,
    mix,
    relative_luminance,
    to_oklab,
)

HEXES = [
    "#000000",
    "#ffffff",
    "#1e1e2e",
    "#cdd6f4",
    "#eff1f5",
    "#4c4f69",
    "#f38ba8",
    "#89b4fa",
    "#40a02b",
    "#7f7f7f",
    "#010203",
    "#fedcba",
]


@pytest.mark.parametrize("value", HEXES)
def test_every_color_survives_a_round_trip_through_oklab(value):
    assert from_oklab(*to_oklab(value)) == value


def test_parsing_ignores_the_leading_hash_and_the_case():
    assert to_oklab("CDD6F4") == to_oklab("#cdd6f4")


def test_mixing_at_the_endpoints_returns_the_endpoints():
    assert mix("#1e1e2e", "#cdd6f4", 0) == "#1e1e2e"
    assert mix("#1e1e2e", "#cdd6f4", 1) == "#cdd6f4"


def test_mixing_walks_lightness_up_from_background_to_foreground():
    steps = [to_oklab(mix("#1e1e2e", "#cdd6f4", index / 10))[0] for index in range(11)]
    assert steps == sorted(steps)
    assert steps[0] < steps[-1]


@pytest.mark.parametrize(
    ("start", "end", "amount", "expected"),
    [
        ("#1e1e2e", "#cdd6f4", 0.25, "#44465a"),
        ("#1e1e2e", "#cdd6f4", -0.1, "#100f1e"),
        ("#eff1f5", "#4c4f69", 0.15, "#d5d7df"),
        ("#40a02b", "#4c4f69", 0.2, "#459042"),
    ],
)
def test_known_mixes_do_not_drift(start, end, amount, expected):
    assert mix(start, end, amount) == expected


def test_mixing_past_the_endpoints_extrapolates():
    assert to_oklab(mix("#1e1e2e", "#cdd6f4", -0.1))[0] < to_oklab("#1e1e2e")[0]
    assert to_oklab(mix("#1e1e2e", "#cdd6f4", 1.1))[0] > to_oklab("#cdd6f4")[0]


def test_black_on_white_is_the_maximum_contrast_ratio():
    assert contrast_ratio("#ffffff", "#000000") == pytest.approx(21.0, abs=0.01)


@pytest.mark.parametrize("value", HEXES)
def test_a_color_against_itself_has_no_contrast(value):
    assert contrast_ratio(value, value) == pytest.approx(1.0)


def test_contrast_does_not_depend_on_the_argument_order():
    assert contrast_ratio("#1e1e2e", "#cdd6f4") == contrast_ratio("#cdd6f4", "#1e1e2e")


def test_relative_luminance_spans_zero_to_one():
    assert relative_luminance("#000000") == pytest.approx(0.0)
    assert relative_luminance("#ffffff") == pytest.approx(1.0)


def test_an_unreachable_chroma_is_pulled_back_into_the_srgb_gamut():
    value = from_oklab(0.7, 0.4, 0.2)
    assert re.fullmatch(r"#[0-9a-f]{6}", value)
    lightness, a, b = to_oklab(value)
    assert lightness == pytest.approx(0.7, abs=0.01)
    assert math.atan2(b, a) == pytest.approx(math.atan2(0.2, 0.4), abs=0.01)


def test_a_lightness_outside_the_gamut_falls_back_to_the_nearest_channel():
    assert from_oklab(1.5, 0.0, 0.0) == "#ffffff"
    assert from_oklab(-0.5, 0.0, 0.0) == "#000000"


def test_is_dark_reports_which_side_the_foreground_is_on():
    assert is_dark("#1e1e2e", "#cdd6f4")
    assert not is_dark("#eff1f5", "#4c4f69")
