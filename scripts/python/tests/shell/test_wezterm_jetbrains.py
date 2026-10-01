import shutil
import subprocess
from pathlib import Path
from urllib.parse import parse_qs, urlparse

import pytest

ROOT = Path(__file__).resolve().parents[4]
PROJECT = "/home/test/project with 'quotes' & # + 日本語"
IDES = [
    ("pycharm", "PyCharm", "pycharm", "PY-262.10968.92"),
    ("rustrover", "RustRover", "rustrover", "RR"),
    ("intellij", "IntelliJ IDEA", "idea", "IU"),
]

HARNESS = r"""
local root, os, ide, mode, path = table.unpack(arg)
package.path = root .. '/shared/wezterm/?.lua;' .. package.path
local remote = mode ~= 'local'
package.loaded['wezterm'] = {
  hostname = function() return 'macie' end,
  action_callback = function(fn) return fn end,
  shell_quote_arg = function(value) return "'" .. value:gsub("'", "'\\''") .. "'" end,
  background_child_process = function(args)
    for _, value in ipairs(args) do print(value) end
  end,
  open_with = print,
  log_error = function(value) print('ERROR: ' .. value) end,
  run_child_process = function(args)
    assert(args[1] == 'ssh' and args[2] == '-G' and args[3] == '--')
    assert(args[4] == (mode == 'ssh' and 'work-alias' or 'archie'))
    return mode ~= 'config-error', mode == 'missing-user' and 'port 22\n'
      or 'hostname 10.77.77.2\nuser remote-user\nport 2222\n'
  end,
}
package.loaded['domain.hosts'] = {target = {hostname = 'archie'}}
package.loaded['utils.platform'] = {
  is_mac = os == 'mac', is_linux = os == 'linux',
  is_remote = function() return remote end,
  remote_target = function() return nil end,
}
local window = {effective_config = function()
  return {tls_clients = {{name = 'tls:archie'}}, ssh_domains = {{name = 'work-alias'}}}
end}
local pane = {
  get_current_working_dir = function()
    if mode == 'no-cwd' then return nil end
    return {scheme = mode == 'invalid-cwd' and 'https' or 'file', file_path = path}
  end,
  get_domain_name = function()
    if mode == 'unknown-target' then return 'unknown' end
    return mode == 'ssh' and 'work-alias' or 'tls:archie'
  end,
}
require('utils.keymap.open-jetbrains')[ide](window, pane)
"""


def launch(os_name, ide, mode):
    lua = shutil.which("lua")
    if not lua:
        pytest.skip("needs Lua")
    result = subprocess.run(
        [lua, "-", str(ROOT), os_name, ide, mode, PROJECT],
        input=HARNESS,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    return result.stdout.splitlines()


@pytest.mark.parametrize("ide,app,command,product", IDES)
@pytest.mark.parametrize("os_name", ["mac", "linux"])
def test_local_project_opens_in_selected_ide(os_name, ide, app, command, product):
    args = launch(os_name, ide, "local")
    if os_name == "mac":
        assert args == ["/usr/bin/open", "-a", app, PROJECT]
    else:
        quoted = "'" + PROJECT.replace("'", "'\\''") + "'"
        assert args == ["/bin/zsh", "-lic", command + " " + quoted]


@pytest.mark.parametrize("ide,app,command,product", IDES)
@pytest.mark.parametrize("mode,target", [("tls", "archie"), ("ssh", "work-alias")])
@pytest.mark.parametrize("os_name", ["mac", "linux"])
def test_remote_project_opens_via_toolbox_with_ssh_config(
    os_name, ide, app, command, product, mode, target
):
    [url] = launch(os_name, ide, mode)
    parsed = urlparse(url)
    assert (parsed.scheme, parsed.netloc, parsed.path) == (
        "jetbrains",
        "gateway",
        "/ssh/environment",
    )
    assert parse_qs(parsed.query) == {
        "h": [target],
        "u": ["remote-user"],
        "p": ["2222"],
        "launchIde": ["true"],
        "ideHint": [product],
        "projectHint": [PROJECT],
    }


@pytest.mark.parametrize("mode", ["no-cwd", "invalid-cwd"])
def test_unavailable_project_does_not_launch(mode):
    assert launch("mac", "pycharm", mode) == []


@pytest.mark.parametrize("mode", ["unknown-target", "config-error", "missing-user"])
def test_unavailable_ssh_connection_reports_error_without_local_launch(mode):
    [error] = launch("mac", "pycharm", mode)
    assert error.startswith("ERROR: JetBrains:")
