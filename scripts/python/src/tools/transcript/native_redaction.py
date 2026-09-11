"""Keep private-value matching in the native secret service."""

import json
import os
import selectors
import subprocess
import time
import weakref

from tools.core.native import binary

LIMIT = 16 * 1024 * 1024
TIMEOUT = 60


def _stop(process):
    if process.stdin:
        process.stdin.close()
    if process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=1)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
    if process.stdout:
        process.stdout.close()


class Redactor:
    def __init__(self):
        self.process = subprocess.Popen(
            [binary("dotfile"), "secret", "__redact"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            bufsize=0,
        )
        os.set_blocking(self.process.stdin.fileno(), False)
        os.set_blocking(self.process.stdout.fileno(), False)
        self._cleanup = weakref.finalize(self, _stop, self.process)

    def __call__(self, text):
        request = (json.dumps(text, ensure_ascii=False) + "\n").encode()
        if len(request) > LIMIT:
            raise RuntimeError("transcript text exceeds redaction record limit")
        deadline = time.monotonic() + TIMEOUT
        sent = 0
        reply = bytearray()
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(self.process.stdin, selectors.EVENT_WRITE)
                selector.register(self.process.stdout, selectors.EVENT_READ)
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise RuntimeError("native redaction timed out")
                    ready = selector.select(remaining)
                    if not ready:
                        raise RuntimeError("native redaction timed out")
                    for key, _ in ready:
                        if key.fileobj is self.process.stdin:
                            try:
                                sent += os.write(key.fd, request[sent : sent + 65536])
                            except BlockingIOError:
                                continue
                            if sent == len(request):
                                selector.unregister(self.process.stdin)
                        else:
                            try:
                                chunk = os.read(key.fd, 65536)
                            except BlockingIOError:
                                continue
                            if not chunk:
                                raise RuntimeError("native redaction closed unexpectedly")
                            reply.extend(chunk)
                            if len(reply) > LIMIT:
                                raise RuntimeError("invalid native redaction response")
                            if b"\n" in reply:
                                if sent != len(request) or not reply.endswith(b"\n"):
                                    raise RuntimeError("invalid native redaction response")
                                redacted = json.loads(reply)
                                if not isinstance(redacted, str):
                                    raise RuntimeError("invalid native redaction response")
                                return redacted
        except (OSError, ValueError, RuntimeError) as error:
            self._cleanup()
            raise RuntimeError("native redaction failed") from error
