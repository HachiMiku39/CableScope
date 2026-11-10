"""Short-lived, one-use pairing codes and origin-bound in-memory sessions."""
import re
import secrets
import threading
import time
from urllib.parse import urlsplit


class Pairing:
    def __init__(self, site_url, clock=time.monotonic):
        url = urlsplit(site_url)
        if url.scheme != "https" or not url.hostname or url.username or url.password or url.query or url.fragment:
            raise ValueError("网页地址须为不含登录信息的 HTTPS 地址")
        self.site_url = site_url.rstrip("/") + "/"
        self.origin = url.scheme + "://" + url.netloc
        self.clock, self.lock = clock, threading.Lock()
        self.sessions, self.failures = {}, 0
        self.rotate()

    def rotate(self):
        with self.lock:
            self.code = "%08d" % secrets.randbelow(100000000)
            self.code_until = self.clock() + 300
            self.failures = 0
            return self.code

    def local_info(self, base):
        with self.lock:
            return {"code": self.code, "expires_in": max(0, int(self.code_until - self.clock())),
                    "site": self.site_url, "base": base, "sessions": sum(s[1] > self.clock() for s in self.sessions.values())}

    def pair(self, origin, code):
        with self.lock:
            if origin != self.origin or self.clock() >= self.code_until or self.failures >= 5 or self.code is None:
                return None
            if not isinstance(code, str) or not re.fullmatch(r"[0-9]{8}", code) or not secrets.compare_digest(code, self.code):
                self.failures += 1
                return None
            self.sessions = {k: v for k, v in self.sessions.items() if v[1] > self.clock()}
            token = secrets.token_urlsafe(32)
            self.sessions[token] = (origin, self.clock() + 3600)
            self.code = None
            return token

    def valid(self, origin, token):
        with self.lock:
            session = self.sessions.get(token)
            return bool(session and session[0] == origin and session[1] > self.clock())

    def disconnect(self, token):
        with self.lock:
            self.sessions.pop(token, None)

    def revoke(self):
        with self.lock:
            self.sessions.clear()
        self.rotate()
