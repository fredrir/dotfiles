from tools.utils.sysinfo.models import Fact


def fact(label, value):
    return Fact(label, str(value)) if value not in (None, "") else None


def facts(*values):
    return tuple(value for value in values if value is not None)
