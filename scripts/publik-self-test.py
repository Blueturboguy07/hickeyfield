#!/usr/bin/env python3
"""publik-self-test.py <app executable> <slug> <expected version>

Proves that the BUILT app runs the publik sign-in gate at start-up. With PUBLIK_SELF_TEST=1 the
publik module prints `publik self-test: ok <slug> <version>` after a real GET /wallet, then exits.
publik itself is a listener on 127.0.0.1 here (PUBLIK_ORIGIN), so no request reaches publikhq.com,
no app token is needed (PUBLIK_APP_TOKEN is a fixture) and nothing is counted (DO_NOT_TRACK=1).
The app runs with a temporary home folder, so no real credential file is read or written.
Standard library only. Exit 0 only when the line is printed, the app exits by itself, and it
called POST /api/v1/installs and then GET /api/v1/wallet with the key the fake publik minted.
"""
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

exe, slug, want_version = sys.argv[1:4]
exe = os.path.abspath(exe)
KEY = "pk_self_test_fixture"
requests = []


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _json(self, status, data):
        body = json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _body(self):
        length = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(length).decode("utf-8", "replace") if length else ""

    def do_POST(self):
        body = self._body()
        auth = self.headers.get("Authorization")
        requests.append({"method": "POST", "path": self.path, "auth": auth == "Bearer " + KEY})
        origin = "http://127.0.0.1:%d" % self.server.server_address[1]
        if self.path == "/api/v1/installs":
            try:
                sent = json.loads(body)
            except ValueError:
                sent = {}
            if not str(sent.get("app_token", "")).startswith("pat_") or sent.get("app_slug") != slug:
                return self._json(401, {"error": {"type": "invalid_app_token", "message": "This app token is not valid."}})
            return self._json(201, {"key": KEY, "install_id": sent.get("install_id"), "claim_url": origin + "/claim/self-test",
                                    "claim_state": "anonymous", "base_url": origin + "/api/v1"})
        if self.path == "/api/v1/telemetry":
            self.send_response(204)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return None
        return self._json(404, {"error": {"type": "not_found", "message": "no such route"}})

    def do_GET(self):
        auth = self.headers.get("Authorization")
        requests.append({"method": "GET", "path": self.path, "auth": auth == "Bearer " + KEY})
        origin = "http://127.0.0.1:%d" % self.server.server_address[1]
        if self.path == "/api/v1/wallet":
            if auth != "Bearer " + KEY:
                return self._json(401, {"error": {"type": "invalid_key", "message": "This key is not valid."}})
            return self._json(200, {"claim_state": "claimed", "claim_url": origin + "/claim/self-test", "account": {"handle": "ci"},
                                    "plan": {"kind": "none", "label": None}, "balance_micros": 0,
                                    "top_up_url": origin + "/wallet", "linked_at": None})
        return self._json(404, {"error": {"type": "not_found", "message": "no such route"}})

    def log_message(self, *args):
        pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
server.daemon_threads = True
threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True).start()
origin = "http://127.0.0.1:%d" % server.server_address[1]

home = tempfile.mkdtemp(prefix="publik-self-test-home-")
env = {
    "PATH": os.environ.get("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"),
    "HOME": home,
    "USERPROFILE": home,
    "XDG_CONFIG_HOME": os.path.join(home, ".config"),
    "LOCALAPPDATA": os.path.join(home, "AppData", "Local"),
    "CFFIXED_USER_HOME": home,  # WebKit keeps its data here too, not in the real home
    "USER": os.environ.get("USER", ""),
    "LANG": "en_US.UTF-8",
    "PUBLIK_ORIGIN": origin,
    "PUBLIK_APP_TOKEN": "pat_self_test_fixture",
    "PUBLIK_SELF_TEST": "1",
    "DO_NOT_TRACK": "1",
}
started = time.time()
try:
    proc = subprocess.run([exe], env=env, cwd=home, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, timeout=120)
    output = proc.stdout.decode("utf-8", "replace")
    code = proc.returncode
except subprocess.TimeoutExpired as e:
    output = (e.stdout or b"").decode("utf-8", "replace")
    code = "timeout"
ran = round(time.time() - started, 1)
time.sleep(0.3)
server.shutdown()
server.server_close()
subprocess.run(["rm", "-rf", home], check=False)

lines = [line for line in output.splitlines() if line.startswith("publik self-test:")]
want = "publik self-test: ok %s %s" % (slug, want_version)
paths = [(r["method"], r["path"]) for r in requests]
faults = []
if want not in lines:
    faults.append("want the line %r, got %r" % (want, lines))
if code != 0:
    faults.append("the app did not exit with 0 (got %s)" % code)
if paths != [("POST", "/api/v1/installs"), ("GET", "/api/v1/wallet")]:
    faults.append("want POST /installs then GET /wallet and nothing else, got %r" % paths)
if any(r["path"] == "/api/v1/wallet" and not r["auth"] for r in requests):
    faults.append("GET /wallet did not carry the minted key")
report = {"app": slug, "expected_version": want_version, "executable": exe, "ran_seconds": ran, "exit_code": code,
          "self_test_lines": lines, "requests": paths, "faults": faults, "pass": not faults}
print(json.dumps(report, indent=2))
if faults:
    print("--- app output (last 40 lines) ---")
    print("\n".join(output.splitlines()[-40:]))
print("RESULT: " + ("PASS" if not faults else "FAIL"))
sys.exit(0 if not faults else 1)
