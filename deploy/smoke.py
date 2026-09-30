#!/usr/bin/env python3
"""Offline real-binary smoke: command adapter, authentication, SIGTERM and restart."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix="morse-smoke-") as temp:
    root = Path(temp)
    wrapper = root / "agent"
    wrapper.write_text('#!/usr/bin/env bash\nset -euo pipefail\n[[ -z ${MORSE_TOKEN+x} ]] || exit 41\nif [[ "$1" == slow ]]; then sleep 15; touch "'+str(root / 'escaped-child')+'"; else printf "%s" "$1"; fi\n')
    wrapper.chmod(0o755)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    token = "smoke-token-private-" * 4
    env = dict(os.environ, MORSE_HOME=str(root / "state"), MORSE_TOKEN=token,
               MORSE_PROVIDER="command", MORSE_AGENT_PROGRAM=str(wrapper), MORSE_AGENT_ARGS='["{prompt}"]')

    def request(path, body=None, authenticated=True):
        headers = {"Authorization": f"Bearer {token}"} if authenticated else {}
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(base + path, data=data, headers=headers)
        with urllib.request.urlopen(req, timeout=3) as res:
            raw = res.read()
            return json.loads(raw) if path != "/healthz" else raw

    def start():
        log = open(root / "server.log", "ab")
        proc = subprocess.Popen([binary, "serve", "--bind", f"127.0.0.1:{port}"], env=env, stdout=log, stderr=log)
        log.close()
        for _ in range(100):
            if proc.poll() is not None:
                raise RuntimeError((root / "server.log").read_text())
            try:
                request("/healthz")
                return proc
            except OSError:
                time.sleep(.05)
        proc.kill()
        raise RuntimeError("server did not start")

    def wait_status(sid, status):
        for _ in range(200):
            if request(f"/api/sessions/{sid}")["status"] == status:
                return
            time.sleep(.05)
        raise RuntimeError(f"session did not reach {status}")

    proc = start()
    try:
        try:
            request("/api/sessions", authenticated=False)
            raise AssertionError("unauthenticated request accepted")
        except urllib.error.HTTPError as error:
            assert error.code == 401
        sid = request("/api/sessions", {"workspace": str(root / "project")})["id"]
        prompt = "quotes ' \" and newline\n$(touch injected) {session_id}"
        request(f"/api/sessions/{sid}/instruction", {"text": prompt})
        for _ in range(200):
            events = request(f"/api/sessions/{sid}/events")["events"]
            output = "".join(e["chunk"] for e in events if e["type"] == "output" and e["stream"] == "stdout")
            successful = any(e["type"] == "tool_result" and e["ok"] for e in events)
            if output == prompt and successful:
                break
            time.sleep(.05)
        else:
            raise AssertionError(f"literal prompt missing in replay: {events}")
        wait_status(sid, "idle")
        assert not (root / "project/injected").exists()
        request(f"/api/sessions/{sid}/instruction", {"text": "slow"})
        wait_status(sid, "working")
        time.sleep(.2)
        proc.send_signal(signal.SIGTERM)
        assert proc.wait(timeout=5) == 0
        assert not (root / "escaped-child").exists()
        proc = start()
        assert sid in [s["id"] for s in request("/api/sessions")]
        wait_status(sid, "interrupted")
        assert request(f"/api/sessions/{sid}/events")["last_seq"] > 0
        print("PASS: command prompt quoting, token isolation, auth, SIGTERM and persisted replay")
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)
