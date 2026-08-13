import os
import re
import subprocess

from tools.core.process import capture as run
from tools.utils.sysinfo.bench.record import HIB, WORLD
from tools.utils.sysinfo.bench.suites import (
    Job,
    MeasurementError,
    Output,
    numbers,
    require,
    tool_path,
    version_of,
)

DICTIONARY = "-md22"
PASSES = "1"
CIPHER = "aes-256-gcm"
BLOCK = "16384"

TOTAL = re.compile(r"^Tot:\s+(.*)$", re.MULTILINE)

# p7zip announces itself as "7-Zip [64] 16.02"; the bracketed word size is not
# the version, and recording it as one defeated the version guard that keeps
# world-comparable ratings from being compared across incompatible tools.
SEVEN_ZIP_VERSION = r"7-Zip\s*(?:\([^)]*\)|\[[^\]]*\])?\s*(\d[\d.]*)"

BREW_OPENSSL = (
    "/opt/homebrew/opt/openssl@3/bin/openssl",
    "/usr/local/opt/openssl@3/bin/openssl",
)


def openssl_path():
    for candidate in BREW_OPENSSL:
        if os.access(candidate, os.X_OK):
            return candidate
    return tool_path("openssl")


def openssl_implementation(path):
    try:
        result = run([path, "version"], timeout=15)
    except (OSError, subprocess.SubprocessError):
        return "unknown"
    return result.stdout.strip().split(" ", 1)[0] or "unknown"


def seven_zip_rating(path, threads):
    arguments = [path, "b", PASSES, DICTIONARY]
    if threads == 1:
        arguments.append("-mmt1")
    result = require(run(arguments, timeout=300), "7z")
    match = TOTAL.search(result.stdout)
    if not match:
        raise MeasurementError("7z produced no total rating")
    values = numbers(match.group(1))
    if len(values) < 3:
        raise MeasurementError("7z total rating is incomplete")
    return values[-1]


def openssl_throughput(path):
    result = require(
        run([path, "speed", "-evp", CIPHER, "-seconds", "1", "-bytes", BLOCK], timeout=180),
        "openssl",
    )
    for line in result.stdout.splitlines():
        if line.upper().startswith(CIPHER.upper()):
            fields = line.split()
            if len(fields) < 2:
                continue
            values = numbers(fields[-1])
            if values:
                return values[-1] / 1000
    raise MeasurementError("openssl reported no throughput")


def jobs(setting):
    found = []
    seven = tool_path("7z", "7zz")
    if seven:
        version = version_of(seven, args=(), pattern=SEVEN_ZIP_VERSION)
        name = os.path.basename(seven)
        found.append(
            Job(
                name="cpu.single",
                tool=name,
                version=version,
                method="cpu.single/1.0.0",
                outputs=(Output("cpu.single", "MIPS", HIB, WORLD),),
                measure=lambda: {"cpu.single": seven_zip_rating(seven, 1)},
                detail={"dictionary": DICTIONARY, "passes": PASSES},
            )
        )
        found.append(
            Job(
                name="cpu.multi",
                tool=name,
                version=version,
                method="cpu.multi/1.0.0",
                outputs=(Output("cpu.multi", "MIPS", HIB, WORLD),),
                measure=lambda: {"cpu.multi": seven_zip_rating(seven, 0)},
                detail={"dictionary": DICTIONARY, "passes": PASSES},
            )
        )
    openssl = openssl_path()
    if openssl:
        implementation = openssl_implementation(openssl)
        found.append(
            Job(
                name="cpu.crypto",
                tool=implementation.lower(),
                version=version_of(openssl, args=("version",), pattern=r"(\d[\d.]*)"),
                method="cpu.crypto/1.0.0",
                outputs=(Output("cpu.crypto", "MB/s", HIB, WORLD),),
                measure=lambda: {"cpu.crypto": openssl_throughput(openssl)},
                detail={"cipher": CIPHER, "block": BLOCK, "implementation": implementation},
            )
        )
    return found
