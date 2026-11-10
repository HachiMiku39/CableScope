import struct
import tempfile
from pathlib import Path
import unittest
from cablescope.core import decode_identity, number, signed64, diagnostics
from cablescope.engine import Store
from cablescope.backends.windows import decode_connection, InterfaceData, PowerStatus


def identity(cable, revision=3, kind=3):
    return {"Specification Revision": revision, "Metadata": {"VDOs": [struct.pack("<I", w) for w in
            [(kind << 27) | 0x1234, 0, 0x56780001, cable]]}}


class PDTests(unittest.TestCase):
    def test_little_endian_5a_100w(self):
        result = decode_identity(identity((2 << 5) | 2))
        self.assertEqual(result["current_a"], 5)
        self.assertEqual(result["power_ceiling_w"], 100)
        self.assertIn("10 Gbps", result["speed"])

    def test_epr_is_240_not_250(self):
        result = decode_identity(identity((2 << 5) | (3 << 9) | (1 << 17) | 4))
        self.assertEqual(result["power_ceiling_w"], 240)

    def test_ambiguous_speed_stays_ambiguous(self):
        self.assertIn("歧义", decode_identity(identity(3))["speed"])

    def test_unknown_current_not_assumed_3a(self):
        for bits in (0, 3):
            result = decode_identity(identity(bits << 5))
            self.assertIsNone(result["current_a"])
            self.assertIsNone(result["power_ceiling_w"])

    def test_pd2_layout_is_not_decoded_as_pd3(self):
        self.assertIsNone(decode_identity(identity(0xFFFFFFFF, revision=2))["current_a"])

    def test_active_cable_power_not_guessed(self):
        self.assertIsNone(decode_identity(identity((2 << 5) | (3 << 9), kind=4))["power_ceiling_w"])

    def test_deprecated_voltage_not_claimed_240w(self):
        self.assertIsNone(decode_identity(identity((2 << 5) | (1 << 9)))["power_ceiling_w"])

    def test_boolean_nan_are_not_measurements(self):
        self.assertIsNone(number(True))
        self.assertIsNone(number(float("nan")))
        self.assertEqual(signed64((1 << 64) - 1000), -1000)

    def test_no_cable_bottleneck_invented(self):
        notes = diagnostics({"power": {"external_connected": True, "charging": False}, "ports": []})
        self.assertIn("不能判断", notes[0]["text"])


class WindowsReplyTests(unittest.TestCase):
    def reply(self):
        data = bytearray(35)
        struct.pack_into("<HH", data, 12, 0x1234, 0x5678)
        data[23] = 2
        struct.pack_into("<I", data, 31, 1)
        return data

    def test_usb2_link_does_not_claim_cable_usb_version(self):
        result = decode_connection(self.reply())
        self.assertEqual(result["speed"], "480 Mbps")
        self.assertEqual(result["vendor_id"], 0x1234)

    def test_v2_superspeed_override(self):
        result = decode_connection(self.reply(), struct.pack("<IIII", 1, 16, 4, 4))
        self.assertIn("SuperSpeedPlus", result["speed"])
        self.assertIn("未确定", result["speed"])

    def test_disconnected_and_truncated(self):
        self.assertIsNone(decode_connection(bytes(35)))
        with self.assertRaises(ValueError):
            decode_connection(bytes(34))

    def test_ffi_layout(self):
        import ctypes
        self.assertEqual(ctypes.sizeof(PowerStatus), 12)
        self.assertEqual(ctypes.sizeof(InterfaceData), 32 if ctypes.sizeof(ctypes.c_void_p) == 8 else 28)


class HistoryTests(unittest.TestCase):
    def test_saved_names_are_manual_and_parameterized(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = Store(Path(tmp) / "history.sqlite3")
            snapshot = {"at": "2026-11-10T08:00:00+00:00", "power": {"input_w": None, "battery_net_w": 0}}
            store.add(snapshot)
            store.save("a'); DROP TABLE records; --", "<script>bad</script>", snapshot)
            self.assertEqual(len(store.records()), 1)
            self.assertIsNone(store.history()[0]["input_w"])
            self.assertEqual(store.history()[0]["battery_net_w"], 0)
            with self.assertRaises(ValueError):
                store.save(" ", "", snapshot)


if __name__ == "__main__":
    unittest.main()
