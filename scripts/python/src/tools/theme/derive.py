import re

from tools.theme import oklab

CONTRAST = "contrast("
INTEGER = re.compile(r"[+-]?\d+")
PERCENT = re.compile(r"\d+(\.\d+)?")
HEX = re.compile(r"#[0-9a-fA-F]{6}")


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
        if name == "bg":
            return self.background
        if name == "fg":
            return self.foreground
        return self.lookup(name)


def resolve(expression, lookup, background, foreground):
    return _resolve(expression, _Scope(lookup, background, foreground))


def _resolve(expression, scope):
    parts = _parts(expression)
    step, alpha = _tail(parts, expression)
    return Resolved(_color(parts[0], step, expression, scope), alpha)


def _color(head, step, expression, scope):
    inner = _contrast_argument(head)
    if inner is not None:
        return _contrast(inner, step, expression, scope)
    if "~" in head:
        return _mixed(head, step, expression, scope)
    return _ladder(head, step, expression, scope)


def _contrast(inner, step, expression, scope):
    if step is not None:
        raise SystemExit(f"contrast() takes no ladder step: {expression}")
    resolved = _resolve(inner, scope)
    if resolved.alpha is not None:
        raise SystemExit(f"alpha belongs outside contrast(): {expression}")
    on_background = oklab.contrast_ratio(scope.background, resolved.hex)
    on_foreground = oklab.contrast_ratio(scope.foreground, resolved.hex)
    return scope.background if on_background >= on_foreground else scope.foreground


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
    if name not in ("bg", "fg"):
        raise SystemExit(f"ladder needs bg or fg: {expression} (write {name}~fg/{step})")
    from_background = name == "bg"
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


def _contrast_argument(head):
    if head.startswith(CONTRAST) and head.endswith(")"):
        return head[len(CONTRAST) : -1]
    return None


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
