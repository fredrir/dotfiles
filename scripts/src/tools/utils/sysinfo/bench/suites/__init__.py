import re
import shutil
from dataclasses import dataclass, field

from tools.core.process import capture as run

WRITTEN = "__bytes_written"


@dataclass(frozen=True)
class Output:
    key: str
    scale: str
    proportion: str
    comparable: str


@dataclass(frozen=True)
class Job:
    name: str
    tool: str
    version: str
    method: str
    outputs: tuple[Output, ...]
    measure: object
    writes: int = 0
    repeat: bool = True
    detail: dict = field(default_factory=dict)


class MeasurementError(Exception):
    pass


def tool_path(*names):
    for name in names:
        found = shutil.which(name)
        if found:
            return found
    return ""


def version_of(path, args=("--version",), pattern=r"(\d[\d.]*)"):
    if not path:
        return ""
    result = run([path, *args])
    match = re.search(pattern, (result.stdout or "") + (result.stderr or ""))
    return match.group(1) if match else ""


def numbers(text):
    return [float(value) for value in re.findall(r"-?\d+(?:\.\d+)?", text)]


def require(result, tool):
    if result.returncode != 0:
        raise MeasurementError(f"{tool} exited {result.returncode}")
    return result
