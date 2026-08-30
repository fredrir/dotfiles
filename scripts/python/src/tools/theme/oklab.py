import math

GAMUT_STEPS = 16
GAMUT_TOLERANCE = 1e-6


def _srgb(value):
    digits = value.lstrip("#")
    return tuple(int(digits[start : start + 2], 16) / 255 for start in (0, 2, 4))


def _to_linear(channel):
    if channel <= 0.04045:
        return channel / 12.92
    return ((channel + 0.055) / 1.055) ** 2.4


def _from_linear(channel):
    if channel <= 0.0031308:
        return channel * 12.92
    return 1.055 * channel ** (1 / 2.4) - 0.055


def _cbrt(value):
    return math.copysign(abs(value) ** (1 / 3), value)


def _linear_to_oklab(red, green, blue):
    cone_l = _cbrt(0.4122214708 * red + 0.5363325363 * green + 0.0514459929 * blue)
    cone_m = _cbrt(0.2119034982 * red + 0.6806995451 * green + 0.1073969566 * blue)
    cone_s = _cbrt(0.0883024619 * red + 0.2817188376 * green + 0.6299787005 * blue)
    return (
        0.2104542553 * cone_l + 0.7936177850 * cone_m - 0.0040720468 * cone_s,
        1.9779984951 * cone_l - 2.4285922050 * cone_m + 0.4505937099 * cone_s,
        0.0259040371 * cone_l + 0.7827717662 * cone_m - 0.8086757660 * cone_s,
    )


def _oklab_to_srgb(L, a, b):
    cone_l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3
    cone_m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3
    cone_s = (L - 0.0894841775 * a - 1.2914855480 * b) ** 3
    return (
        _from_linear(4.0767416621 * cone_l - 3.3077115913 * cone_m + 0.2309699292 * cone_s),
        _from_linear(-1.2684380046 * cone_l + 2.6097574011 * cone_m - 0.3413193965 * cone_s),
        _from_linear(-0.0041960863 * cone_l - 0.7034186147 * cone_m + 1.7076147010 * cone_s),
    )


def _in_gamut(channels):
    return all(-GAMUT_TOLERANCE <= channel <= 1 + GAMUT_TOLERANCE for channel in channels)


def _byte(channel):
    return round(min(1.0, max(0.0, channel)) * 255)


def to_oklab(value):
    return _linear_to_oklab(*(_to_linear(channel) for channel in _srgb(value)))


def from_oklab(L, a, b):
    channels = _oklab_to_srgb(L, a, b)
    if not _in_gamut(channels):
        # Desaturate towards the L axis rather than clipping, so hue survives.
        low, high = 0.0, 1.0
        for _ in range(GAMUT_STEPS):
            scale = (low + high) / 2
            if _in_gamut(_oklab_to_srgb(L, a * scale, b * scale)):
                low = scale
            else:
                high = scale
        channels = _oklab_to_srgb(L, a * low, b * low)
    return "#" + "".join(f"{_byte(channel):02x}" for channel in channels)


def mix(a, b, t):
    start = to_oklab(a)
    end = to_oklab(b)
    return from_oklab(*(one * (1 - t) + two * t for one, two in zip(start, end)))


def relative_luminance(value):
    red, green, blue = (_to_linear(channel) for channel in _srgb(value))
    return 0.2126 * red + 0.7152 * green + 0.0722 * blue


def contrast_ratio(a, b):
    first = relative_luminance(a)
    second = relative_luminance(b)
    return (max(first, second) + 0.05) / (min(first, second) + 0.05)


def is_dark(background, foreground):
    return relative_luminance(foreground) > relative_luminance(background)
