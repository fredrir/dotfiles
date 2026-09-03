import re

from tools.theme import oklab

INTEGER = re.compile(r"[+-]?\d+")
PERCENT = re.compile(r"\d+(\.\d+)?")
HEX = re.compile(r"#[0-9a-fA-F]{6}")
BACKGROUND_NAMES = ("bg", "background", "ui.background")
FOREGROUND_NAMES = ("fg", "foreground", "ui.foreground")


class Resolved:
    def __init__(self, value, alpha=None):
        self.hex = value.lower()
        self.alpha = alpha


class _Scope:
    def __init__(self, lookup, background, foreground):
        self.lookup = lookup
        self.background = background
        self.foreground = foreground

    def named(self, name):
        if HEX.fullmatch(name):
            return name.lower()
        if name in BACKGROUND_NAMES:
            return self.background
        if name in FOREGROUND_NAMES:
            return self.foreground
        return self.lookup(name)


def resolve(expression, lookup, background, foreground):
    return _resolve(expression, _Scope(lookup, background, foreground))


def _resolve(expression, scope):
    parts = _parts(expression)
    step, alpha = _tail(parts, expression)
    return Resolved(_color(parts[0], step, expression, scope), alpha)


def _color(head, step, expression, scope):
    function = _function(head)
    if function is not None:
        name, arguments = function
        return _derived(name, arguments, step, expression, scope)
    if "~" in head:
        return _mixed(head, step, expression, scope)
    return _ladder(head, step, expression, scope)


def _derived(name, arguments, step, expression, scope):
    if step is not None:
        raise SystemExit(f"{name}() takes no ladder step: {expression}")
    if name == "contrast":
        if len(arguments) != 1:
            raise SystemExit(f"contrast() takes one color: {expression}")
        return _on(arguments[0], 4.5, expression, scope)
    if name == "on":
        if len(arguments) not in (1, 2):
            raise SystemExit(f"on() takes a color and optional floor: {expression}")
        floor = _floor(arguments[1], expression) if len(arguments) == 2 else 4.5
        return _on(arguments[0], floor, expression, scope)
    if name in ("readable", "visible"):
        if len(arguments) not in (2, 3):
            raise SystemExit(f"{name}() takes color, background, and optional floor: {expression}")
        floor = _floor(arguments[2], expression) if len(arguments) == 3 else 4.5
        color = _opaque(arguments[0], expression, scope)
        against = _opaque(arguments[1], expression, scope)
        return oklab.ensure_contrast(color, against, floor)
    raise SystemExit(f"unknown color function '{name}': {expression}")


def _on(argument, floor, expression, scope):
    fill = _opaque(argument, expression, scope)
    return oklab.on_color(fill, scope.foreground, scope.background, floor)


def _opaque(argument, expression, scope):
    resolved = _resolve(argument, scope)
    if resolved.alpha is not None:
        raise SystemExit(f"alpha is not allowed inside a color function: {expression}")
    return resolved.hex


def _floor(value, expression):
    try:
        floor = float(value)
    except ValueError:
        raise SystemExit(f"contrast floor must be a number: {expression}")
    if floor < 1 or floor > 21:
        raise SystemExit(f"contrast floor must be from 1 to 21: {expression}")
    return floor


def _mixed(pair, step, expression, scope):
    names = [name.strip() for name in pair.split("~")]
    if len(names) != 2 or not all(names):
        raise SystemExit(f"mix takes two colors: {expression}")
    if step is None:
        raise SystemExit(f"mix needs a step: {expression} (write {names[0]}~{names[1]}/500)")
    left, right = [scope.named(name) for name in names]
    return oklab.mix(left, right, step / 1000)


def _ladder(name, step, expression, scope):
    if step is None:
        return scope.named(name)
    if name not in (*BACKGROUND_NAMES, *FOREGROUND_NAMES):
        raise SystemExit(
            f"ladder needs background or foreground: {expression} (write {name}~foreground/{step})"
        )
    from_background = name in BACKGROUND_NAMES
    if from_background:
        start, other = scope.background, scope.foreground
    else:
        start, other = scope.foreground, scope.background
    darker_is_ahead = oklab.is_dark(scope.background, scope.foreground) != from_background
    return oklab.mix(start, other, _amount(step, darker_is_ahead))


def _amount(permille, darker_is_ahead):
    amount = permille / 1000
    if amount < 0 and darker_is_ahead:
        return -amount
    return amount


def _function(head):
    opening = head.find("(")
    if opening < 1 or not head.endswith(")"):
        return None
    name = head[:opening].strip()
    body = head[opening + 1 : -1]
    arguments = []
    start = 0
    depth = 0
    for index, char in enumerate(body):
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        elif char == "," and depth == 0:
            arguments.append(body[start:index].strip())
            start = index + 1
    arguments.append(body[start:].strip())
    if not body.strip() or not all(arguments):
        raise SystemExit(f"empty color function argument: {head}")
    return name, arguments


def _parts(expression):
    text = expression.strip()
    if not text:
        raise SystemExit(f"empty color expression: {expression!r}")
    parts = [""]
    depth = 0
    for char in text:
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        if char == "/" and depth == 0:
            parts.append("")
        else:
            parts[-1] += char
    if depth:
        raise SystemExit(f"unbalanced parentheses: {expression}")
    return [part.strip() for part in parts]


def _tail(parts, expression):
    rest = parts[1:]
    if len(rest) > 2:
        raise SystemExit(f"too many '/' parts: {expression}")
    alpha = None
    if rest and rest[-1].endswith("%"):
        alpha = _alpha(rest.pop(), expression)
    if len(rest) > 1:
        raise SystemExit(f"alpha must come last: {expression}")
    step = _permille(rest[0], expression) if rest else None
    return step, alpha


def _permille(text, expression):
    if not INTEGER.fullmatch(text):
        raise SystemExit(f"step must be a whole per-mille: {expression}")
    return int(text)


def _alpha(text, expression):
    number = text[:-1].strip()
    if not PERCENT.fullmatch(number) or float(number) > 100:
        raise SystemExit(f"alpha must be a percent from 0 to 100: {expression}")
    return float(number) / 100
