#!/usr/bin/env python3
"""Local lifecycle stand-in; never connects to an inference service."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

if sys.argv[1:3] == ["auth", "status"]:
    print(json.dumps({"loggedIn": os.environ.get("AH_FAIL_AUTH") != "1"}))
    sys.exit(0)

home = Path(os.environ["HOME"])
store = Path(os.environ["CLAUDE_CONFIG_DIR"])
settings = json.loads(Path(sys.argv[sys.argv.index("--settings") + 1]).read_text())
session = sys.argv[sys.argv.index("--resume") + 1] if "--resume" in sys.argv else "fake-session"
cwd = str(Path.cwd())
project = "".join(c if c.isascii() and c.isalnum() else "-" for c in cwd)
transcript = store / "projects" / project / f"{session}.jsonl"
transcript.parent.mkdir(parents=True, exist_ok=True)
if not transcript.exists():
    transcript.write_text(json.dumps({"sessionId": session, "cwd": cwd, "type": "user", "message": {"role": "user", "content": "fixture"}}) + "\n")
(home / "agent.pid").write_text(str(os.getpid()))

def hook(event):
    payload = json.dumps({"hook_event_name": event, "session_id": session, "cwd": cwd, "transcript_path": str(transcript)})
    for group in settings["hooks"].get(event, []):
        for handler in group["hooks"]:
            subprocess.run(handler["command"], shell=True, input=payload, text=True, check=True)

def stop(_signal, _frame):
    (home / "agent.stopped").write_text("yes")
    sys.exit(0)

signal.signal(signal.SIGTERM, stop)
hook("SessionStart")
if "--resume" not in sys.argv:
    hook("UserPromptSubmit")
    while not (home / "finish-turn").exists():
        time.sleep(0.05)
    hook("Stop")
while not (home / "stop-owner").exists():
    time.sleep(0.05)
stop(None, None)
