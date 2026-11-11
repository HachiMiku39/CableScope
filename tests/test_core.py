import json
import struct
import tempfile
import unittest
from pathlib import Path
from cablescope.core import decode_identity, number, signed64, diagnostics
from cablescope.engine import Store
from cablescope.analysis import enrich
from cablescope.backends.windows import (
    IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES, PORT_CONNECTOR_IS_TYPE_C, PORT_DEBUG_CAPABLE,
    PORT_HAS_MULTIPLE_COMPANIONS, PORT_USER_CONNECTABLE, USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH,
    USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH, Backend, classify_ucsi_state, collect_port_candidates,
    companion_group_key, decode_connection, decode_connector_properties, digest, InterfaceData,
    normal_hub_link, port_candidate, PowerStatus, read_connector_properties, ucsi_limitations,
)


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


class WindowsPortConnectorTests(unittest.TestCase):
    def props(self, index=3, length=None, flags=0, companion_index=0, companion_port=0, link="", reserved=0):
        text = (link + "\x00").encode("utf-16-le")
        total = length if length is not None else 16 + len(text)
        data = bytearray(max(total, 18))
        struct.pack_into("<IIIHH", data, 0, index, total, flags | reserved, companion_index, companion_port)
        data[16:16 + len(text)] = text
        return bytes(data[:total])

    def test_pack1_offsets_and_four_known_bit_mask(self):
        flags = PORT_USER_CONNECTABLE | PORT_HAS_MULTIPLE_COMPANIONS | PORT_CONNECTOR_IS_TYPE_C
        row = decode_connector_properties(self.props(flags=flags, companion_index=2,
                                                     companion_port=7, link=r"\\?\usb#hub"))
        self.assertEqual(row["connection_index"], 3)
        self.assertEqual(row["actual_length"], 16 + len((r"\\?\usb#hub" + "\x00").encode("utf-16-le")))
        self.assertTrue(row["user_connectable"])
        self.assertFalse(row["debug_capable"])
        self.assertTrue(row["multiple_companions"])
        self.assertTrue(row["type_c"])
        self.assertEqual(row["companion_index"], 2)
        self.assertEqual(row["companion_port"], 7)
        self.assertEqual(row["companion_link"], normal_hub_link(r"\\?\usb#hub"))
        self.assertEqual(row["reserved_bits"], 0)

    def test_reserved_bit_0x10_is_not_a_flag(self):
        row = decode_connector_properties(self.props(flags=PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C,
                                                     reserved=0x10))
        self.assertEqual(row["reserved_bits"], 0x10)
        self.assertTrue(row["user_connectable"])
        self.assertTrue(row["type_c"])
        self.assertFalse(row["debug_capable"])
        self.assertFalse(row["multiple_companions"])
        self.assertNotIn("ucsi", row)
        self.assertNotIn("connector_id", row)
        self.assertEqual(row["companion_port"], 0)

    def test_debug_port_is_rejected_and_user_connectable_is_required(self):
        flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C
        rows = [decode_connector_properties(self.props(3, flags=flags)),
                decode_connector_properties(self.props(4, flags=flags | PORT_DEBUG_CAPABLE)),
                decode_connector_properties(self.props(5, flags=PORT_CONNECTOR_IS_TYPE_C)),
                decode_connector_properties(self.props(6, flags=PORT_USER_CONNECTABLE))]
        self.assertEqual([p["number"] for p in collect_port_candidates(rows, "hub-a", 1)], [3])

    def test_truncated_reply_and_invalid_actual_length(self):
        with self.assertRaises(ValueError):
            decode_connector_properties(bytes(17))
        for bad in (0, 17, USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH + 1):
            data = bytearray(self.props(length=18))
            struct.pack_into("<I", data, 4, bad)
            with self.assertRaises(ValueError):
                decode_connector_properties(bytes(data))
        self.assertEqual(decode_connector_properties(self.props(length=18))["actual_length"], 18)
        self.assertEqual(decode_connector_properties(self.props(length=USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH))["actual_length"],
                         USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH)

    def test_two_stage_length_and_companion_index_loop(self):
        link = r"\\?\usb#companion"
        length = 16 + len((link + "\x00").encode("utf-16-le"))
        flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C | PORT_HAS_MULTIPLE_COMPANIONS
        calls = []

        def ioctl(handle, code, initial, size):
            calls.append((code, struct.unpack_from("<I", initial, 0)[0], size))
            self.assertEqual(code, IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES)
            self.assertEqual(handle, "hub-handle")
            if len(calls) == 1:
                self.assertEqual(size, USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH)
                return self.props(3, length=length, flags=flags, link=link, companion_port=1)
            companion = struct.unpack_from("<H", initial, 12)[0]
            self.assertEqual(size, length)
            self.assertEqual(struct.unpack_from("<I", initial, 4)[0], length)
            return self.props(3, length=length, flags=flags, companion_index=companion,
                              companion_port=1 if companion < 2 else 0, link=link)

        rows = read_connector_properties(ioctl, "hub-handle", 3)
        self.assertEqual(len(calls), 4)
        self.assertEqual([r["companion_index"] for r in rows], [0, 1, 2])
        self.assertEqual([r["companion_port"] for r in rows], [1, 1, 0])

    def test_companion_group_merges_and_prefers_type_c_row(self):
        link = r"\\?\USB#HUB-1"
        type_c = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C
        plain = PORT_USER_CONNECTABLE
        rows = [decode_connector_properties(self.props(2, flags=plain | PORT_HAS_MULTIPLE_COMPANIONS,
                                                      companion_index=0, companion_port=4, link=link)),
                decode_connector_properties(self.props(7, flags=type_c | PORT_HAS_MULTIPLE_COMPANIONS,
                                                      companion_index=1, companion_port=4, link=link.upper().replace("/", "\\")))]
        self.assertEqual(companion_group_key(normal_hub_link(link), 4),
                         companion_group_key(normal_hub_link(link.upper()), 4))
        ports = collect_port_candidates(rows, "hub-a", 2)
        self.assertEqual(len(ports), 1)
        self.assertEqual(ports[0]["number"], 7)
        self.assertIn("Hub #2 Port 7", ports[0]["name"])

    def test_no_companion_rows_are_not_merged_across_hubs(self):
        flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C
        row_a = decode_connector_properties(self.props(1, flags=flags))
        row_b = decode_connector_properties(self.props(1, flags=flags))
        first = collect_port_candidates([row_a], "hub-a", 1)
        second = collect_port_candidates([row_b], "hub-b", 2)
        self.assertEqual(len(first), 1)
        self.assertEqual(len(second), 1)
        self.assertNotEqual(first[0]["id"], second[0]["id"])

    def test_conflicting_group_key_keeps_rows_separate_with_source_note(self):
        flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C
        c1 = decode_connector_properties(self.props(1, flags=flags, companion_port=4, link=r"\\?\A"))
        c2 = decode_connector_properties(self.props(9, flags=flags, companion_port=4, link=r"\\?\B"))
        self.assertNotEqual(companion_group_key(c1["companion_link"], 4),
                            companion_group_key(c2["companion_link"], 4))
        # A hash collision is simulated by aliasing one companion key; the source
        # must then explain that rows were kept separate rather than merged.
        from cablescope.backends import windows as windows_module
        original = windows_module.companion_group_key
        windows_module.companion_group_key = lambda link, port: "collision"
        try:
            ports = collect_port_candidates([c1, c2], "hub-a", 1)
        finally:
            windows_module.companion_group_key = original
        self.assertEqual(len(ports), 2)
        self.assertEqual({p["number"] for p in ports}, {1, 9})
        self.assertTrue(all("冲突" in p["source"] for p in ports))

    def test_candidate_never_infers_connection_or_hardware_data(self):
        flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C
        rows = [decode_connector_properties(self.props(9, flags=flags))]
        port = collect_port_candidates(rows, "hub-a", 3)[0]
        for key in ("connected", "contract", "output_power", "input_power", "cable", "orientation",
                    "overcurrent_count", "plug_count", "liquid_detected"):
            self.assertIsNone(port[key])
        self.assertEqual(port["pin_configuration"], {})
        self.assertEqual(port["kind"], "USB-C")
        self.assertEqual(port["number"], 9)
        self.assertEqual(port["id"], digest("hub-a", "9"))
        self.assertNotIn("\\\\?\\", port["id"])
        snapshot = enrich({"power": {}, "ports": [port]})
        self.assertIsNone(snapshot["ports"][0]["pin_map"])
        self.assertIn("缺少完整链路上限", snapshot["ports"][0]["negotiation"]["notes"][0])
        self.assertIsNone(snapshot["ports"][0]["connected"])

    def test_raw_hub_paths_never_enter_snapshot_or_error_text(self):
        raw = r"\\?\usb#vid_1234#pid_5678#{f18a0e88-c30c-11d0-8815-00a0c906bed8}"
        flags = PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C
        row = decode_connector_properties(self.props(1, flags=flags, companion_port=2, link=raw))
        ports = collect_port_candidates([row], digest(normal_hub_link(raw)), 1)
        text = json.dumps({"ports": ports, "errors": ["Hub #1 Port 1 端口属性: 无效"]}, ensure_ascii=False)
        self.assertNotIn(raw.lower(), text.lower())
        self.assertNotIn("vid_1234", text.lower())
        self.assertEqual(len(ports[0]["id"]), 16)

    def test_ucsi_state_and_dynamic_limitations(self):
        self.assertEqual(classify_ucsi_state({"ucsi_state": "available"}), "available")
        self.assertEqual(classify_ucsi_state({"ucsi_state": "no_data_plane"}), "no_data_plane")
        self.assertEqual(classify_ucsi_state({"ucsi_state": "query_failed"}), "query_failed")
        for bad in ({}, {"ucsi_state": None}, {"ucsi_state": "unknown"}, {"ucsi_state": True}):
            self.assertEqual(classify_ucsi_state(bad), "query_failed")
        texts = [ucsi_limitations(s) for s in ("available", "no_data_plane", "query_failed")]
        self.assertEqual(len(set(texts)), 3)
        for text in texts:
            self.assertIn("未启用测试接口", text)
            self.assertIn("未修改注册表", text)
            self.assertIn("未重启设备", text)
            self.assertIn("功率", text)
            self.assertNotIn("已解析", text)
            self.assertNotIn("已读取 PD", text)
        self.assertIn("本机固件未暴露", texts[1])

    def test_backend_uses_dynamic_limitation_and_candidate_ports(self):
        backend = Backend.__new__(Backend)
        backend.inventory = {"devices": [], "batteries": [], "ucsi_state": "no_data_plane", "errors": []}
        backend.inventory_time = float("inf")
        candidate = port_candidate(decode_connector_properties(self.props(1, flags=PORT_USER_CONNECTABLE | PORT_CONNECTOR_IS_TYPE_C)),
                                   "hub-a", "Hub #1 Port 1")
        import cablescope.backends.windows as windows
        original = windows.usb_devices, windows.displays, windows.system_power
        windows.usb_devices = lambda: ([{"name": "USB device"}], [], [candidate])
        windows.displays = lambda: []
        windows.system_power = lambda errors: {}
        try:
            snapshot = backend.sample()
        finally:
            windows.usb_devices, windows.displays, windows.system_power = original
        self.assertEqual(snapshot["ports"], [candidate])
        self.assertIsNone(snapshot["ports"][0]["connected"])
        self.assertIn("本机固件未暴露", snapshot["limitations"][0])
        self.assertNotIn("首版", snapshot["limitations"][0])


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
