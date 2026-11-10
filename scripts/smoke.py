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
        proc = subprocess.Popen(cmd + ["--no-browser", "--port", "0", "--data-dir", tmp], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                text=True, encoding="utf-8", errors="replace")
        lines = queue.Queue()
        observed = []
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
                observed.append(line)
            assert url, "No local server URL; stdout before timeout: " + repr(observed)
            def request(path="", payload=None, origin=None, headers_extra=None, method=None, absolute=False):
                headers = {"Content-Type": "application/json"} if payload is not None else {}
                if origin:
                    headers["Origin"] = origin
                headers.update(headers_extra or {})
                req = urllib.request.Request((path if absolute else url + path), data=json.dumps(payload).encode() if payload is not None else None, headers=headers, method=method)
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
            cable_info = {"length_m":1.25,"price_cny":39,"softness":4}
            if state["snapshot"]["ports"]:
                cable_info["port_id"] = state["snapshot"]["ports"][0]["id"]
            assert json.load(request("api/save", {"name": "smoke", "notes": "synthetic test label", "cable_info":cable_info}))["ok"]
            report = json.load(request("api/report"))
            assert report["records"][0]["name"] == "smoke"
            assert report["records"][0]["snapshot"]["cable_info"]["length_m"] == 1.25
            try:
                request("api/save", {"name":"bad","notes":"","cable_info":{"price_cny":True}})
                raise AssertionError("Boolean accepted as a cable price")
            except urllib.error.HTTPError as exc:
                assert exc.code == 400
            assert b"input_w_system_telemetry" in request("api/history.csv").read()
            from urllib.parse import urlsplit
            split = urlsplit(url)
            bridge = split.scheme + "://" + split.netloc + "/bridge/"
            site = "https://cablescope-lab.leisurelyhorse4.chatgpt.site"
            code = state["bridge"]["code"]
            preflight = request(bridge+"api/state", origin=site, method="OPTIONS", absolute=True,
                                headers_extra={"Access-Control-Request-Method":"GET", "Access-Control-Request-Headers":"authorization"})
            assert preflight.status == 204 and preflight.headers["Access-Control-Allow-Origin"] == site
            try:
                request(bridge+"api/state", origin=site, absolute=True)
                raise AssertionError("Unpaired hardware read allowed")
            except urllib.error.HTTPError as exc:
                assert exc.code == 401
            token = json.load(request(bridge+"pair", {"code":code}, site, absolute=True))["token"]
            remote = json.load(request(bridge+"api/state", origin=site, absolute=True, headers_extra={"Authorization":"Bearer "+token}))
            assert "bridge" not in remote, "Pairing code leaked to remote website"
            for endpoint in ("pair", "api/state"):
                try:
                    request(bridge+endpoint, {"code":code} if endpoint=="pair" else None, "https://evil.invalid", absolute=True,
                            headers_extra={"Authorization":"Bearer "+token})
                    raise AssertionError("Untrusted origin accepted")
                except urllib.error.HTTPError as exc:
                    assert exc.code == 403 and exc.headers.get("Access-Control-Allow-Origin") is None
            assert json.load(request("api/revoke", {}))["ok"]
            try:
                request(bridge+"api/state", origin=site, absolute=True, headers_extra={"Authorization":"Bearer "+token})
                raise AssertionError("Revoked session accepted")
            except urllib.error.HTTPError as exc:
                assert exc.code == 401
            assert json.load(request("api/quit", {}))["ok"]
            proc.wait(timeout=20)
            assert proc.returncode == 0
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
    print("Native snapshot, assets, exports, save, pairing, CORS, revocation and shutdown passed.")


if __name__ == "__main__":
    main()
