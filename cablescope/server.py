import csv
import io
import json
import secrets
import threading
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit
from . import __version__

WEB = Path(__file__).parent / "web"


def serve(engine, open_browser=True, port=0):
    token = secrets.token_urlsafe(24)
    prefix = "/" + token + "/"
    last_request = [time.monotonic()]

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass  # Do not log the access token or device inventory.

        def reply(self, code, content, mime="application/json; charset=utf-8", filename=None):
            if isinstance(content, str):
                content = content.encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", mime)
            self.send_header("Content-Length", str(len(content)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")
            if filename:
                self.send_header("Content-Disposition", 'attachment; filename="%s"' % filename)
            self.end_headers()
            self.wfile.write(content)

        def route(self):
            if self.headers.get("Host") != server.expected_host:
                self.reply(403, '{}')
                return None
            origin = self.headers.get("Origin")
            if origin and origin != "http://" + server.expected_host:
                self.reply(403, '{}')
                return None
            path = urlsplit(self.path).path
            if not path.startswith(prefix):
                self.reply(404, '{}')
                return None
            last_request[0] = time.monotonic()
            return path[len(prefix):]

        def do_GET(self):
            path = self.route()
            if path is None:
                return
            if path == "api/state":
                return self.reply(200, json.dumps(engine.state(), ensure_ascii=False, allow_nan=False))
            if path == "api/report":
                return self.reply(200, json.dumps({"app": "CableScope", "version": __version__, **engine.state()}, ensure_ascii=False, indent=2), filename="CableScope-report.json")
            if path == "api/history.csv":
                data = io.StringIO(newline="")
                w = csv.writer(data)
                w.writerow(["at_UTC", "input_w_system_telemetry", "adapter_rated_w", "battery_net_w", "battery_percent"])
                for row in engine.store.history(5000):
                    w.writerow([row.get(k) for k in ("at", "input_w", "adapter_rated_w", "battery_net_w", "battery_percent")])
                return self.reply(200, "\ufeff" + data.getvalue(), "text/csv; charset=utf-8", "CableScope-power.csv")
            files = {"": ("index.html", "text/html; charset=utf-8"), "style.css": ("style.css", "text/css; charset=utf-8"), "app.js": ("app.js", "text/javascript; charset=utf-8")}
            if path in files:
                name, mime = files[path]
                return self.reply(200, (WEB / name).read_bytes(), mime)
            self.reply(404, '{}')

        def do_POST(self):
            path = self.route()
            if path is None:
                return
            if self.headers.get("Content-Type", "").split(";")[0] != "application/json":
                return self.reply(415, '{}')
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 8192:
                    raise ValueError("请求长度无效")
                data = json.loads(self.rfile.read(length))
                if not isinstance(data, dict):
                    raise ValueError("请求必须为对象")
                if path == "api/save":
                    name, notes = data.get("name", ""), data.get("notes", "")
                    if not isinstance(name, str) or not isinstance(notes, str):
                        raise ValueError("名称和备注必须为文字")
                    engine.save(name, notes)
                    return self.reply(200, '{"ok":true}')
                if path == "api/quit":
                    self.reply(200, '{"ok":true}')
                    threading.Thread(target=server.shutdown, daemon=True).start()
                    return
                self.reply(404, '{}')
            except (ValueError, TypeError) as exc:
                self.reply(400, json.dumps({"error": str(exc)}, ensure_ascii=False))

    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.daemon_threads = True
    server.expected_host = "127.0.0.1:%d" % server.server_port
    url = "http://" + server.expected_host + prefix
    engine.thread.start()
    print("CableScope %s · 本地运行；Ctrl+C 退出\n%s" % (__version__, url), flush=True)

    def idle_check():
        while not engine.stop.wait(15):
            if time.monotonic() - last_request[0] > 600:
                server.shutdown()
                return

    threading.Thread(target=idle_check, daemon=True).start()
    if open_browser:
        webbrowser.open(url)
    try:
        server.serve_forever(poll_interval=0.3)
    except KeyboardInterrupt:
        pass
    finally:
        engine.stop.set()
        server.server_close()
        engine.thread.join(timeout=15)
