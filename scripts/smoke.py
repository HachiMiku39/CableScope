"""Exercise real native backend + local server; never upload hardware snapshots."""
import argparse
import json
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    cmd = [str(args.executable.resolve())] if args.executable else [sys.executable, str(root / "run.py")]
    once = subprocess.run(cmd + ["--json"], capture_output=True, timeout=35, check=True)
    d = json.loads(once.stdout)
    assert d["platform"] in ("macOS", "Windows")
    assert isinstance(d["ports"], list) and isinstance(d["errors"], list)
    with tempfile.TemporaryDirectory(prefix="cablescope-smoke-") as tmp:
        proc = subprocess.Popen(cmd + ["--no-browser", "--data-dir", tmp], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        lines = queue.Queue()
        def read_lines():
            for line in proc.stdout:
                lines.put(line.strip())
        threading.Thread(target=read_lines, daemon=True).start()
        try:
            deadline, url = time.monotonic() + 25, None
            while time.monotonic() < deadline:
                try:
                    line = lines.get(timeout=1)
                except queue.Empty:
                    if proc.poll() is not None:
                        raise RuntimeError("Server exited: " + proc.stderr.read())
                    continue
                if line.startswith("http://127.0.0.1:"):
                    url = line
                    break
            assert url, "No local server URL"
            def request(path="", payload=None, origin=None):
                headers = {"Content-Type": "application/json"} if payload is not None else {}
                if origin:
                    headers["Origin"] = origin
                req = urllib.request.Request(url + path, data=json.dumps(payload).encode() if payload is not None else None, headers=headers)
                return urllib.request.urlopen(req, timeout=10)
            assert "CableScope" in request().read().decode()
            assert b"function render" in request("app.js").read()
            try:
                request("api/state", origin="https://example.invalid")
                raise AssertionError("Cross-origin read allowed")
            except urllib.error.HTTPError as exc:
                assert exc.code == 403
            state = None
            for _ in range(30):
                state = json.load(request("api/state"))
                if not state["snapshot"].get("loading"):
                    break
                time.sleep(.5)
            assert state["snapshot"]["platform"] in ("macOS", "Windows"), state["snapshot"].get("errors")
            assert json.load(request("api/save", {"name": "smoke", "notes": "synthetic test label"}))["ok"]
            report = json.load(request("api/report"))
            assert report["records"][0]["name"] == "smoke"
            assert b"input_w_system_telemetry" in request("api/history.csv").read()
            assert json.load(request("api/quit", {}))["ok"]
            proc.wait(timeout=20)
            assert proc.returncode == 0
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
    print("Native snapshot, bundled assets, HTTP API, export, save, origin check and shutdown passed.")


if __name__ == "__main__":
    main()
