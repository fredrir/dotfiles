import os
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[4]
SCRIPT = ROOT / "scripts/shell/wezterm-mtls"


def openssl3():
    for candidate in ("/opt/homebrew/bin/openssl", "/usr/local/bin/openssl", "/usr/bin/openssl"):
        if shutil.which(candidate):
            version = subprocess.run(
                [candidate, "version"], capture_output=True, text=True, check=False
            ).stdout
            if version.startswith(("OpenSSL 3.", "OpenSSL 4.")):
                return candidate
    return None


@pytest.fixture
def mtls(tmp_path):
    openssl = openssl3()
    if openssl is None:
        pytest.skip("needs OpenSSL 3")
    passfile = tmp_path / "ca-pass"
    passfile.write_text("ca passphrase\n")

    def run(*args, passfile_env=True):
        env = {
            **os.environ,
            "HOME": str(tmp_path),
            "USER": "tester",
            "WEZTERM_MTLS_OPENSSL": openssl,
            "WEZTERM_MTLS_DIR": str(tmp_path / "mtls"),
            "WEZTERM_MTLS_CA_DIR": str(tmp_path / "mtls-ca"),
            "WEZTERM_MTLS_HOSTNAME": "box",
        }
        if passfile_env:
            env["WEZTERM_MTLS_CA_PASSFILE"] = str(passfile)
        return subprocess.run(
            [str(SCRIPT), *args],
            capture_output=True,
            text=True,
            env=env,
            stdin=subprocess.DEVNULL,
            check=False,
        )

    assert run("ca", "--encrypt").returncode == 0
    assert run("csr").returncode == 0
    return tmp_path / "mtls", run


def test_a_failed_signing_leaves_no_certificate_behind(mtls):
    directory, run = mtls
    result = run("issue", "box", passfile_env=False)
    assert result.returncode == 1
    assert "can simply be re-run" in result.stderr
    assert not (directory / "box-cert.pem").exists()


def test_a_signed_certificate_installs(mtls):
    directory, run = mtls
    assert run("issue", "box").returncode == 0
    result = run("install", str(directory.parent / "mtls-ca" / "ca.pem"))
    assert result.returncode == 0, result.stderr
    assert (directory / "cert.pem").is_file()
