import unittest
from cablescope.pairing import Pairing

SITE = "https://cablescope-lab.leisurelyhorse4.chatgpt.site"


class PairingTests(unittest.TestCase):
    def setUp(self):
        self.time = 100.0
        self.p = Pairing(SITE, clock=lambda: self.time)

    def test_code_single_use_and_session_origin_bound(self):
        code = self.p.local_info("http://127.0.0.1:47831")["code"]
        token = self.p.pair(SITE, code)
        self.assertTrue(self.p.valid(SITE, token))
        self.assertFalse(self.p.valid("https://evil.invalid", token))
        self.assertIsNone(self.p.pair(SITE, code))

    def test_code_expires_after_five_minutes(self):
        code = self.p.code
        self.time += 300
        self.assertIsNone(self.p.pair(SITE, code))

    def test_five_failures_require_local_refresh(self):
        code = self.p.code
        for _ in range(5):
            self.assertIsNone(self.p.pair(SITE, "bad"))
        self.assertIsNone(self.p.pair(SITE, code))
        fresh = self.p.rotate()
        self.assertIsNotNone(self.p.pair(SITE, fresh))

    def test_revoke_invalidates_sessions(self):
        token = self.p.pair(SITE, self.p.code)
        self.p.revoke()
        self.assertFalse(self.p.valid(SITE, token))

    def test_disconnect_and_one_hour_session_expiry(self):
        token = self.p.pair(SITE, self.p.code)
        self.time += 3600
        self.assertFalse(self.p.valid(SITE, token))
        token = self.p.pair(SITE, self.p.rotate())
        self.p.disconnect(token)
        self.assertFalse(self.p.valid(SITE, token))

    def test_origin_has_to_be_https(self):
        for url in ("http://example.com", "https://user:password@example.com", "https://example.com/#token"):
            with self.assertRaises(ValueError):
                Pairing(url)
