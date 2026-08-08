import getpass
import os
import shutil
import socket
import sys

from tools.core.process import capture


def display_username():
    return getpass.getuser()


def display_hostname():
    override = os.environ.get("SYSINFO_HOSTNAME")
    if override:
        return override
    if sys.platform == "darwin" and shutil.which("scutil"):
        for key in ("LocalHostName", "ComputerName"):
            result = capture(["scutil", "--get", key])
            if result.returncode == 0 and result.stdout.strip():
                return result.stdout.strip()
    return socket.gethostname().split(".", 1)[0]
