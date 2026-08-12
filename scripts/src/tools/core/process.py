import subprocess


def run(command, check=False, **kwargs):
    return subprocess.run(command, check=check, **kwargs)


def capture(command, check=False, **kwargs):
    return subprocess.run(command, capture_output=True, text=True, check=check, **kwargs)


def capture_bytes(command, check=False, **kwargs):
    return subprocess.run(command, capture_output=True, check=check, **kwargs)


def silent(command, check=False, **kwargs):
    return subprocess.run(
        command,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=check,
        **kwargs,
    ).returncode
