import base64
import re
import sys

from .harness import wait_for


def test_reload_is_idempotent_and_navigation_does_not_repeat(server):
    client = server.attach()
    server.load()
    features = server.tm("show-options", "-s", "terminal-features")
    hooks = server.tm("show-hooks", "-g")
    server.run("reload")
    server.run("reload")
    assert server.tm("show-options", "-s", "terminal-features") == features
    assert server.tm("show-hooks", "-g") == hooks
    keys = server.tm("list-keys", "-T", "prefix")
    for key in "hjkl":
        binding = next(
            line for line in keys.splitlines() if re.search(r"prefix\s+" + key + r"\s", line)
        )
        assert " -r " not in binding
    client.press(b"\x02d")
    wait_for(lambda: len(server.tm("list-panes").splitlines()) == 2)
    second = server.tm("display-message", "-p", "#{pane_id}")
    client.press(b"\x02hhello\r")
    wait_for(lambda: "hello" in server.capture())
    assert "hello" not in server.capture(second)


def test_input_decoder_preserves_special_key_bytes(server):
    client = server.attach()
    server.load()
    reader = (
        "import os,tty;tty.setraw(0);os.write(1,b'READY\\r\\n');"
        "exec(\"while True:\\n b=os.read(0,1)\\n if not b: break\\n os.write(1,b.hex().encode()+b'\\\\r\\\\n')\")"
    )
    server.tm("respawn-pane", "-k", "-t", server.pane, sys.executable, "-u", "-c", reader)
    wait_for(lambda: "READY" in server.capture())
    payload = b"\x1b[13;2u\x1b[5;30012~\x03\x04\x1bb\x1bf"
    expected = payload.replace(b"\x1b[5;30012~", b"\x1b[115;9u")
    client.press(payload)
    wanted = " ".join(f"{byte:02x}" for byte in expected)
    wait_for(lambda: " ".join(server.capture().partition("READY")[2].split()) == wanted)


def test_resize_and_nested_modes_belong_to_each_client(server):
    first = server.attach()
    second = server.attach()
    server.load()
    first.press(b"\x02R")
    wait_for(lambda: server.fmt("#{client_key_table}", client=first.name) == "workspace-resize")
    assert server.fmt("#{client_key_table}", client=second.name) == "root"
    assert "RESIZE" in server.fmt("#{E:status-right}", client=first.name)
    assert "RESIZE" not in server.fmt("#{E:status-right}", client=second.name)
    first.press(b"\x1b")
    wait_for(lambda: server.fmt("#{client_key_table}", client=first.name) == "root")
    first.press(b"\x02B")
    wait_for(lambda: server.fmt("#{client_key_table}", client=first.name) == "workspace-nested")
    assert "INNER" not in server.fmt("#{E:status-right}", client=second.name)
    first.press(b"nested text\r")
    wait_for(lambda: "nested text" in server.capture())
    first.press(b"\x02\x1b")
    wait_for(lambda: server.fmt("#{client_key_table}", client=first.name) == "root")


def test_clipboard_write_and_read_reach_the_attached_terminal(server):
    client = server.attach()
    server.load()
    value = "workspace OSC52 roundtrip"
    encoded = base64.b64encode(value.encode())
    server.tm("set-buffer", "-w", "-t", client.name, value)
    wait_for(lambda: encoded in client.output() and b"\x1b]52;" in client.output())
    assert server.tm("show-buffer") == value
    reader = (
        "import os,tty;tty.setraw(0);"
        "os.write(1,bytes((27,))+b']52;c;?'+bytes((7,)));"
        "reply=os.read(0,4096);os.write(1,repr(reply).encode());os.read(0,1)"
    )
    before = len(client.output())
    server.tm("respawn-pane", "-k", "-t", server.pane, sys.executable, "-u", "-c", reader)
    wait_for(lambda: b"\x1b]52;" in client.output()[before:])
    client.press(b"\x1b]52;c;" + encoded + b"\x07")
    wait_for(lambda: encoded.decode() in server.capture())


def test_status_adapts_to_width_and_displays_operation_modes(server):
    client = server.attach()
    server.load()
    assert "origin" in server.fmt("#{E:status-left}", client=client.name)
    host = server.fmt("#h", client=client.name)
    client.resize(30, 60)
    wait_for(lambda: "origin" not in server.fmt("#{E:status-left}", client=client.name))
    assert host in server.fmt("#{E:status-left}", client=client.name)
    client.press(b"\x02\r")
    wait_for(lambda: "COPY" in server.fmt("#{E:status-right}", client=client.name))
    server.tm("send-keys", "-X", "cancel")
    client.press(b"\x02d")
    wait_for(lambda: len(server.tm("list-panes").splitlines()) == 2)
    server.tm("resize-pane", "-Z")
    server.tm("set-window-option", "synchronize-panes", "on")
    status = server.fmt("#{E:status-right}", client=client.name)
    assert "SYNC" in status and "ZOOM" in status
    assert "bold]" not in re.sub(r"#\[[^\]]*\]", "", status)


def test_attachment_marker_is_cleared_when_server_dies(server):
    from .harness import Terminal

    client = Terminal(server, "origin", managed=True)
    server.clients.append(client)
    wait_for(lambda: b"TMUX_WORKSPACE=MQ==" in client.output())
    server.tm("kill-server")
    wait_for(lambda: b"TMUX_WORKSPACE=\x07" in client.output())
    client.process.wait(timeout=3)
