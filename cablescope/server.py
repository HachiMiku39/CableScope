import csv
import io
import json
import secrets
import socketserver
import threading
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit
from . import __version__
from .pairing import Pairing

WEB = Path(__file__).parent / "web"


def serve(engine, open_browser=True, port=47831, site_url="https://cablescope-lab.leisurelyhorse4.chatgpt.site/"):
    pairing = Pairing(site_url)
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
            if getattr(self, "cors_origin", None):
                self.send_header("Access-Control-Allow-Origin", self.cors_origin)
                self.send_header("Vary", "Origin")
                self.send_header("Access-Control-Expose-Headers", "Content-Disposition")
                if self.command == "OPTIONS":
                    self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
                    self.send_header("Access-Control-Allow-Headers", "Authorization, Content-Type")
                    if self.headers.get("Access-Control-Request-Private-Network") == "true":
                        self.send_header("Access-Control-Allow-Private-Network", "true")
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
            path = urlsplit(self.path).path
            if path.startswith("/bridge/"):
                if origin != pairing.origin:
                    self.reply(403, '{}')
                    return None
                self.cors_origin = origin
                if self.command == "OPTIONS" or path in ("/bridge/health", "/bridge/pair"):
                    return path
                auth = self.headers.get("Authorization", "")
                self.session_token = auth[7:] if auth.startswith("Bearer ") else ""
                if not pairing.valid(origin, self.session_token):
                    self.reply(401, '{"error":"配对已失效，请重新连接"}')
                    return None
                last_request[0] = time.monotonic()
                return path[len("/bridge/"):]
            if origin and origin != "http://" + server.expected_host:
                self.reply(403, '{}')
                return None
            if not path.startswith(prefix):
                self.reply(404, '{}')
                return None
            last_request[0] = time.monotonic()
            return path[len(prefix):]

        def do_OPTIONS(self):
            path = self.route()
            if path is not None and path.startswith("/bridge/"):
                self.reply(204, b"")
            elif path is not None:
                self.reply(404, '{}')

        def do_GET(self):
            path = self.route()
            if path is None:
                return
            if path == "/bridge/health":
                return self.reply(200, json.dumps({"app": "CableScope", "version": __version__, "pairing": True}))
            if path == "api/state":
                state = engine.state()
                if not getattr(self, "cors_origin", None):
                    state["bridge"] = pairing.local_info("http://" + server.expected_host)
                return self.reply(200, json.dumps(state, ensure_ascii=False, allow_nan=False))
            if path == "api/report":
                return self.reply(200, json.dumps({"app": "CableScope", "version": __version__, **engine.state()}, ensure_ascii=False, indent=2), filename="CableScope-report.json")
            if path == "api/history.csv":
                data = io.StringIO(newline="")
                w = csv.writer(data)
                w.writerow(["at_UTC", "input_w_system_telemetry", "adapter_rated_w", "battery_net_w", "battery_percent"])
                for row in engine.store.history(5000):
                    w.writerow([row.get(k) for k in ("at", "input_w", "adapter_rated_w", "battery_net_w", "battery_percent")])
                return self.reply(200, "\ufeff" + data.getvalue(), "text/csv; charset=utf-8", "CableScope-power.csv")
            files = {"": ("index.html", "text/html; charset=utf-8"), "style.css": ("style.css", "text/css; charset=utf-8"), "app.js": ("app.js", "text/javascript; charset=utf-8"), "connection.js": ("connection.js", "text/javascript; charset=utf-8")}
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
                if path == "/bridge/pair":
                    result = pairing.pair(self.headers.get("Origin"), data.get("code"))
                    if not result:
                        return self.reply(403, '{"error":"配对码错误、已使用或已过期；请在本机窗口刷新配对码"}')
                    last_request[0] = time.monotonic()
                    return self.reply(200, json.dumps({"token": result, "expires_in": 3600, "version": __version__}))
                if path == "api/disconnect" and getattr(self, "session_token", None):
                    pairing.disconnect(self.session_token)
                    return self.reply(200, '{"ok":true}')
                if path in ("api/new-code", "api/revoke"):
                    if getattr(self, "cors_origin", None):
                        return self.reply(403, '{}')
                    pairing.revoke() if path == "api/revoke" else pairing.rotate()
                    return self.reply(200, '{"ok":true}')
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

    class LocalServer(ThreadingHTTPServer):
        def server_bind(self):
            # This is an explicit loopback service; avoid blocking reverse DNS.
            socketserver.TCPServer.server_bind(self)
            self.server_name, self.server_port = self.server_address[:2]

    server = LocalServer(("127.0.0.1", port), Handler)
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
