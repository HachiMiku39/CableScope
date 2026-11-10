import ctypes as c
import struct
import unittest
from cablescope.analysis import ResistanceTracker, pin_map, mode_comparison, negotiation
from cablescope.backends.smc import Parameters, Reader, finite_float, normal_uuid


class SMCProtocolTests(unittest.TestCase):
    def test_driver_layout_matches_observed_eighty_byte_abi(self):
        self.assertEqual(c.sizeof(Parameters), 80)
        self.assertEqual(Parameters.info.offset, 28)
        self.assertEqual(Parameters.result.offset, 40)
        self.assertEqual(Parameters.command.offset, 42)
        self.assertEqual(Parameters.data.offset, 48)

    def test_no_write_command_or_unrelated_sensor_key(self):
        reader = Reader.__new__(Reader)
        request = Parameters()
        request.command = 6
        with self.assertRaises(ValueError):
            reader.call(request)
        with self.assertRaises(ValueError):
            reader.read("F0Ac")

    def test_smc_error_is_not_success_even_with_iokit_success(self):
        class IO:
            def IOConnectCallStructMethod(self, connection, selector, incoming, size, output, out_size):
                c.cast(output, c.POINTER(Parameters)).contents.result = 0x84
                return 0
        reader = Reader.__new__(Reader)
        reader.io, reader.connection = IO(), c.c_uint32(1)
        request = Parameters()
        request.command = 9
        self.assertIsNone(reader.call(request))

    def test_float_and_integer_endianness_are_distinct(self):
        self.assertAlmostEqual(finite_float(struct.pack("<f", 28.2)), 28.2, places=4)
        self.assertIsNone(finite_float(struct.pack("<f", float("nan"))))
        reader = Reader.__new__(Reader)
        reader.read = lambda key: (bytes.fromhex("00004e20"), "ui32")
        self.assertEqual(reader.unsigned("D1MV"), 20000)
        self.assertIsNone(reader.floating("VD0R", 0, 60))

    def test_uuid_join_requires_ascii_hex_not_position(self):
        self.assertEqual(normal_uuid("AA"*16), "aa"*16)
        self.assertIsNone(normal_uuid("0"*32))
        self.assertIsNone(normal_uuid("Ａ"*32))


class DiagnosticTests(unittest.TestCase):
    def test_pin_numbers_and_unknown_route_preserved(self):
        p = {"kind":"USB-C", "pin_configuration":{"tx1":1,"rx1":5,"tx2":0,"rx2":12,"sbu1":1,"sbu2":0}}
        rows = pin_map(p)["rows"]
        self.assertEqual(len({pin["id"] for row in rows for pin in row}), 24)
        self.assertEqual(rows[0][1]["signal"], "USB 3")
        self.assertEqual(rows[1][1]["signal"], "DP 0")
        self.assertEqual(rows[0][9]["signal"], "路由 12")
        self.assertEqual(rows[1][0]["id"], "B12")

    def test_magsafe_is_labelled_as_schematic(self):
        result = pin_map({"kind":"MagSafe 3"})
        self.assertEqual(len(result["rows"][0]), 5)
        self.assertIn("不表示已逐针", result["note"])

    def test_display_maximum_does_not_mix_two_modes(self):
        modes = [{"width":3840,"height":2160,"refresh_hz":60},{"width":1920,"height":1080,"refresh_hz":240}]
        result = mode_comparison(modes[1], modes)
        self.assertEqual(result["highest_mode"]["refresh_hz"], 60)
        self.assertEqual(result["highest_mode"]["width"], 3840)

    def test_global_charger_not_assigned_to_output_port(self):
        result = negotiation({"id":"out", "cable":{"power_ceiling_w":60}}, {"adapter_rated_w":140,"input_port_id":"in"})
        self.assertIsNone(result["source_reported_w"])
        self.assertIsNone(result["host_limit_w"])


class ResistanceTests(unittest.TestCase):
    def test_known_input_path_slope(self):
        samples = [(a, 20-.1*a+(.001 if i%2 else -.001)) for i,a in enumerate([.4+i*.07 for i in range(40)])]
        result = ResistanceTracker.fit(samples,20)
        self.assertEqual(result["status"], "estimate")
        self.assertAlmostEqual(result["milliohms"], 100, delta=1)
        self.assertGreater(result["r_squared"], .99)

    def test_flat_current_does_not_produce_estimate(self):
        result = ResistanceTracker.fit([(1,19.9)]*40,20)
        self.assertIsNone(result["milliohms"])
        self.assertEqual(result["status"], "insufficient_variation")

    def test_positive_slope_not_turned_into_resistance(self):
        result = ResistanceTracker.fit([(a,20+.1*a) for a in [.4+i*.07 for i in range(40)]],20)
        self.assertIsNone(result["milliohms"])
        self.assertEqual(result["status"], "unreliable")

    def test_contract_change_resets_samples_and_battery_clears(self):
        tracker = ResistanceTracker()
        snapshot = {"power":{"external_connected":True,"input_port_id":"a","contract_voltage_v":20,
                             "input_voltage_v":19.9,"input_current_a":1,"input_w":19.9,"independent_input_power":True},
                    "ports":[{"id":"a","plug_count":1}]}
        tracker.observe(snapshot)
        tracker.observe(snapshot)
        snapshot["ports"][0]["plug_count"] = 2
        self.assertEqual(tracker.observe(snapshot)["sample_count"],1)
        snapshot["power"]["external_connected"] = False
        self.assertEqual(tracker.observe(snapshot)["sample_count"],0)
