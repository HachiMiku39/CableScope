"""Read-only AppleSMC power keys.

ABI and power-key interpretation adapted from Darryl Morley's MIT-licensed
SMCPowerReader (WhatCable v1.5.0); see licenses/WhatCable-MIT.txt.
Only read-key (5) and get-key-info (9) commands are available here.
"""
import ctypes as c
import math
import re
import struct
import threading


class Version(c.Structure):
    _fields_ = [("major", c.c_uint8), ("minor", c.c_uint8), ("build", c.c_uint8),
                ("reserved", c.c_uint8), ("release", c.c_uint16)]


class PLimit(c.Structure):
    _fields_ = [("version", c.c_uint16), ("length", c.c_uint16), ("cpu", c.c_uint32),
                ("gpu", c.c_uint32), ("memory", c.c_uint32)]


class KeyInfo(c.Structure):
    _fields_ = [("size", c.c_uint32), ("type", c.c_uint32), ("attributes", c.c_uint8)]


class Parameters(c.Structure):
    _fields_ = [("key", c.c_uint32), ("version", Version), ("limits", PLimit), ("info", KeyInfo),
                ("result", c.c_uint8), ("status", c.c_uint8), ("command", c.c_uint8),
                ("data32", c.c_uint32), ("data", c.c_uint8 * 32)]


def finite_float(data):
    if len(data) != 4:
        return None
    value = struct.unpack("<f", data)[0]
    return value if math.isfinite(value) else None


def normal_uuid(value):
    result = str(value or "").replace("-", "").lower()
    return result if re.fullmatch(r"[0-9a-f]{32}", result) and set(result) != {"0"} else None


class Reader:
    def __init__(self):
        if c.sizeof(Parameters) != 80 or Parameters.data.offset != 48 or Parameters.command.offset != 42:
            raise RuntimeError("Unexpected AppleSMC ABI layout")
        self.io = c.CDLL("/System/Library/Frameworks/IOKit.framework/IOKit")
        self.system = c.CDLL("/usr/lib/libSystem.B.dylib")
        self.connection = c.c_uint32()
        self.lock = threading.RLock()
        self.info_cache = {}
        self.status = "尚未读取"
        declarations = [
            ("IOServiceMatching", [c.c_char_p], c.c_void_p),
            ("IOServiceGetMatchingService", [c.c_uint32, c.c_void_p], c.c_uint32),
            ("IOServiceOpen", [c.c_uint32, c.c_uint32, c.c_uint32, c.POINTER(c.c_uint32)], c.c_int32),
            ("IOServiceClose", [c.c_uint32], c.c_int32),
            ("IOObjectRelease", [c.c_uint32], c.c_int32),
            ("IOConnectCallStructMethod", [c.c_uint32, c.c_uint32, c.c_void_p, c.c_size_t,
                                           c.c_void_p, c.POINTER(c.c_size_t)], c.c_int32),
        ]
        for name, args, rest in declarations:
            fn = getattr(self.io, name)
            fn.argtypes, fn.restype = args, rest

    def open(self):
        with self.lock:
            if self.connection.value:
                return True
            service = self.io.IOServiceGetMatchingService(0, self.io.IOServiceMatching(b"AppleSMC"))
            if not service:
                self.status = "此系统未提供 AppleSMC"
                return False
            try:
                task = c.c_uint32.in_dll(self.system, "mach_task_self_").value
                error = self.io.IOServiceOpen(service, task, 0, c.byref(self.connection))
                if error:
                    self.status = "AppleSMC 只读连接不可用（0x%08x）" % (error & 0xffffffff)
                    self.connection.value = 0
                    return False
                self.status = "AppleSMC 只读连接可用"
                return True
            finally:
                self.io.IOObjectRelease(service)

    def close(self):
        with self.lock:
            if self.connection.value:
                self.io.IOServiceClose(self.connection)
                self.connection.value = 0

    def call(self, request):
        if request.command not in (5, 9):
            raise ValueError("Only read commands are supported")
        response, size = Parameters(), c.c_size_t(c.sizeof(Parameters))
        error = self.io.IOConnectCallStructMethod(self.connection, 2, c.byref(request), c.sizeof(request),
                                                  c.byref(response), c.byref(size))
        return response if error == 0 and size.value == 80 and response.result == 0 else None

    def read(self, key):
        if not re.fullmatch(r"VD0R|ID0R|PDTR|PPBR|D[1-4](UI|JV|JI|PR|MP|MV|MI|DE)", key):
            raise ValueError("Key is outside the power read allowlist")
        with self.lock:
            if not self.open():
                return None
            code = int.from_bytes(key.encode("ascii"), "big")
            info = self.info_cache.get(key)
            if info is None:
                request = Parameters()
                request.key, request.command = code, 9
                response = self.call(request)
                if response is None or not 1 <= response.info.size <= 32:
                    return None
                info = (response.info.size, response.info.type)
                self.info_cache[key] = info
            request = Parameters()
            request.key, request.command = code, 5
            request.info.size, request.info.type = info
            response = self.call(request)
            if response is None:
                return None
            return bytes(response.data[:info[0]]), info[1].to_bytes(4, "big").decode("ascii", "replace").strip()

    def floating(self, key, low, high):
        result = self.read(key)
        if not result or result[1] != "flt":
            return None
        value = finite_float(result[0])
        return round(value, 4) if value is not None and low <= value <= high else None

    def unsigned(self, key):
        result = self.read(key)
        if not result or result[1] not in ("ui8", "ui16", "ui32", "ui64"):
            return None
        return int.from_bytes(result[0], "big")

    def sample(self):
        with self.lock:
            if not self.open():
                return {"status": self.status, "system": None, "channels": [], "contracts": []}
            voltage, current = self.floating("VD0R", 0, 60), self.floating("ID0R", 0, 30)
            dedicated = self.floating("PDTR", 0, 1000)
            watts = dedicated if dedicated is not None else (round(voltage * current, 4) if voltage is not None and current is not None else None)
            system = {"voltage_v": voltage, "current_a": current, "watts": watts,
                      "independent_power": dedicated is not None,
                      "source": "AppleSMC VD0R / ID0R / " + ("PDTR" if dedicated is not None else "V×I（计算值）")}
            channels, contracts = [], []
            for index in range(1, 5):
                prefix = "D%d" % index
                identity = self.read(prefix + "UI")
                uuid = normal_uuid(identity[0].hex()) if identity else None
                if not uuid:
                    continue
                v, a = self.floating(prefix + "JV", 0, 60), self.floating(prefix + "JI", 0, 10)
                present = self.unsigned(prefix + "PR")
                channels.append({"_uuid": uuid, "voltage_v": v, "current_a": a,
                                 "watts": round(v * a, 4) if v is not None and a is not None else None,
                                 "present": present >= 1 if present is not None else None,
                                 "direction": "out", "source": "AppleSMC %sJV / %sJI（端口输出遥测）" % (prefix, prefix)})
                mw, mv, ma = self.unsigned(prefix + "MP"), self.unsigned(prefix + "MV"), self.unsigned(prefix + "MI")
                label = self.read(prefix + "DE")
                if mw is not None and 0 < mw <= 1000000:
                    contracts.append({"_uuid": uuid, "watts": round(mw / 1000, 3),
                                      "voltage_v": round(mv / 1000, 3) if mv is not None and 0 < mv <= 60000 else None,
                                      "current_a": round(ma / 1000, 3) if ma is not None and 0 < ma <= 10000 else None,
                                      "label": label[0].split(b"\0", 1)[0].decode("utf-8", "replace") if label else "",
                                      "source": "AppleSMC %sMP / MV / MI（协商上限，非实测功耗）" % prefix})
            return {"status": self.status, "system": system, "channels": channels, "contracts": contracts,
                    "battery_discharge_w": self.floating("PPBR", 0, 200)}
