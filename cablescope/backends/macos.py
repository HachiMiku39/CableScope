"""Read IORegistry properties using public IOKit entry points. No USB traffic."""
import ctypes as c
import json
import plistlib
import subprocess
import time

from ..core import decode_identity, scaled, signed64, number


class Registry:
    def __init__(self):
        self.io = c.CDLL("/System/Library/Frameworks/IOKit.framework/IOKit")
        self.cf = c.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        specs = [
            (self.io, "IOServiceMatching", [c.c_char_p], c.c_void_p),
            (self.io, "IOServiceGetMatchingServices", [c.c_uint, c.c_void_p, c.POINTER(c.c_uint)], c.c_int),
            (self.io, "IOIteratorNext", [c.c_uint], c.c_uint),
            (self.io, "IORegistryEntryCreateCFProperties", [c.c_uint, c.POINTER(c.c_void_p), c.c_void_p, c.c_uint], c.c_int),
            (self.io, "IORegistryEntryGetRegistryEntryID", [c.c_uint, c.POINTER(c.c_uint64)], c.c_int),
            (self.io, "IOObjectRelease", [c.c_uint], c.c_int),
            (self.cf, "CFPropertyListCreateData", [c.c_void_p, c.c_void_p, c.c_long, c.c_ulong, c.c_void_p], c.c_void_p),
            (self.cf, "CFDataGetLength", [c.c_void_p], c.c_long),
            (self.cf, "CFDataGetBytePtr", [c.c_void_p], c.c_void_p),
            (self.cf, "CFRelease", [c.c_void_p], None),
        ]
        for lib, name, args, rest in specs:
            fn = getattr(lib, name)
            fn.argtypes, fn.restype = args, rest

    def services(self, name):
        iterator = c.c_uint()
        error = self.io.IOServiceGetMatchingServices(0, self.io.IOServiceMatching(name.encode()), c.byref(iterator))
        if error:
            raise OSError("IOKit matching %s: 0x%x" % (name, error & 0xffffffff))
        result = []
        try:
            while True:
                obj = self.io.IOIteratorNext(iterator)
                if not obj:
                    break
                props, data = c.c_void_p(), None
                try:
                    if self.io.IORegistryEntryCreateCFProperties(obj, c.byref(props), None, 0):
                        continue
                    data = self.cf.CFPropertyListCreateData(None, props, 200, 0, None)
                    if not data:
                        continue
                    p = plistlib.loads(c.string_at(self.cf.CFDataGetBytePtr(data), self.cf.CFDataGetLength(data)))
                    entry_id = c.c_uint64()
                    self.io.IORegistryEntryGetRegistryEntryID(obj, c.byref(entry_id))
                    p["_registry_id"] = str(entry_id.value)
                    result.append(p)
                finally:
                    if data:
                        self.cf.CFRelease(data)
                    if props:
                        self.cf.CFRelease(props)
                    self.io.IOObjectRelease(obj)
        finally:
            self.io.IOObjectRelease(iterator)
        return result


def display_modes():
    cg = c.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
    cg.CGGetActiveDisplayList.argtypes = [c.c_uint32, c.POINTER(c.c_uint32), c.POINTER(c.c_uint32)]
    cg.CGGetActiveDisplayList.restype = c.c_int32
    cg.CGDisplayCopyDisplayMode.argtypes = [c.c_uint32]
    cg.CGDisplayCopyDisplayMode.restype = c.c_void_p
    cg.CGDisplayModeRelease.argtypes = [c.c_void_p]
    for name in ("CGDisplayModeGetPixelWidth", "CGDisplayModeGetPixelHeight"):
        getattr(cg, name).argtypes = [c.c_void_p]
        getattr(cg, name).restype = c.c_size_t
    cg.CGDisplayModeGetRefreshRate.argtypes = [c.c_void_p]
    cg.CGDisplayModeGetRefreshRate.restype = c.c_double
    cg.CGDisplayIsBuiltin.argtypes = [c.c_uint32]
    cg.CGDisplayIsBuiltin.restype = c.c_uint32
    ids, count = (c.c_uint32 * 64)(), c.c_uint32()
    err = cg.CGGetActiveDisplayList(64, ids, c.byref(count))
    if err:
        raise OSError("CoreGraphics display query failed: %s" % err)
    result = []
    for identifier in list(ids)[:count.value]:
        mode = cg.CGDisplayCopyDisplayMode(identifier)
        if not mode:
            continue
        try:
            hz = cg.CGDisplayModeGetRefreshRate(mode)
            result.append({"name": ("内置显示器" if cg.CGDisplayIsBuiltin(identifier) else "外接显示器") + " #%s" % identifier,
                           "width": cg.CGDisplayModeGetPixelWidth(mode), "height": cg.CGDisplayModeGetPixelHeight(mode),
                           "refresh_hz": round(hz, 2) if hz > 1 else None,
                           "source": "CoreGraphics 当前像素模式；未验证 DP/HDMI 物理带宽或 DSC"})
        finally:
            cg.CGDisplayModeRelease(mode)
    return result


class Backend:
    def __init__(self):
        self.registry = Registry()
        self.fabric, self.fabric_time, self.fabric_error = [], -100, None

    def sample(self):
        r = self.registry
        errors = []

        def read(name):
            try:
                return r.services(name)
            except Exception as exc:
                errors.append("%s: %s" % (name, exc))
                return []

        batteries = read("AppleSmartBattery")
        b = batteries[0] if batteries else {}
        a, t = b.get("AdapterDetails", {}), b.get("PowerTelemetryData", {})
        ext = b.get("ExternalConnected")
        input_mw = number(t.get("SystemPowerIn"))
        input_w = scaled(input_mw, 1000) if ext is True and input_mw is not None and 0 <= input_mw < 1000000 else None
        current, volts = scaled(signed64(b.get("Amperage")), 1000), scaled(b.get("Voltage"), 1000)
        power = {
            "external_connected": ext, "charging": b.get("IsCharging"),
            "battery_percent": (round(b["CurrentCapacity"] / b["MaxCapacity"] * 100, 1)
                                if b.get("MaxCapacity", 0) > 0 and number(b.get("CurrentCapacity")) is not None else None),
            "input_w": input_w, "adapter_rated_w": a.get("Watts") if ext else None,
            "input_voltage_v": scaled(t.get("SystemVoltageIn"), 1000) if ext else None,
            "input_current_a": scaled(t.get("SystemCurrentIn"), 1000) if ext else None,
            "battery_net_w": round(current * volts, 3) if current is not None and volts is not None else None,
            "adapter_name": a.get("Name") if ext else None,
            "profiles": [{"voltage_v": scaled(p.get("MaxVoltage"), 1000), "current_a": scaled(p.get("MaxCurrent"), 1000)}
                         for p in a.get("UsbHvcMenu", [])] if ext else [],
            "source": "AppleSmartBattery.PowerTelemetryData.SystemPowerIn（系统遥测，mW）；不代表墙端功率或单端口功率",
        }
        identities = {}
        for p in read("IOPortTransportComponentCCUSBPDSOPp"):
            key = (p.get("ParentBuiltInPortTypeDescription"), p.get("ParentBuiltInPortNumber"))
            identities.setdefault(key, []).append(p)
        ports, seen = [], set()
        # IOService matching includes subclasses, including HPM/TC controllers.
        raw_ports = read("IOPort")
        if not raw_ports:
            for cls in ("AppleHPMInterfaceType10", "AppleHPMInterfaceType11", "AppleHPMInterfaceType12",
                        "AppleHPMInterfaceType18", "AppleTCControllerType10", "AppleTCControllerType11"):
                raw_ports.extend(read(cls))
        for p in raw_ports:
            kind, num = p.get("PortTypeDescription"), p.get("PortNumber")
            if kind not in ("USB-C", "MagSafe 3") or p["_registry_id"] in seen:
                continue
            seen.add(p["_registry_id"])
            ids = identities.get((kind, num), [])
            connected = p.get("ConnectionActive")
            ports.append({"id": p["_registry_id"], "name": p.get("PortDescription", "%s #%s" % (kind, num)),
                          "kind": kind, "connected": connected, "transports": p.get("TransportsActive", []),
                          "orientation": {1: "方向 1", 2: "方向 2"}.get(p.get("PlugOrientation"), "未知"),
                          "cable": decode_identity(ids[0]) if connected is True and len(ids) == 1 else None,
                          "overcurrent_count": p.get("Overcurrent Count"), "plug_count": p.get("Plug Event Count"),
                          "liquid_detected": p.get("LDCM_LiquidDetected"),
                          "pin_configuration": p.get("Pin Configuration", {}),
                          "source": p.get("IOClass", "IOPort") + " 注册表属性"})
        ports.sort(key=lambda p: p["name"])
        devices = []
        for p in read("IOUSBHostDevice"):
            # 'Device Speed' is the IOUSB enumeration; bcdUSB is a descriptor version, not speed.
            speed = {0: "1.5 Mbps", 1: "12 Mbps", 2: "480 Mbps", 3: "5 Gbps", 4: "SuperSpeedPlus（速率未知）"}.get(p.get("Device Speed"), "未知")
            devices.append({"name": p.get("USB Product Name", "USB 设备"), "vendor": p.get("USB Vendor Name", ""),
                            "vendor_id": p.get("idVendor"), "product_id": p.get("idProduct"),
                            "speed": speed, "source": "IOUSBHostDevice.Device Speed；未映射到物理 USB-C 口"})
        try:
            displays = display_modes()
        except Exception as exc:
            displays = []
            errors.append("显示器: %s" % exc)
        if time.monotonic() - self.fabric_time >= 15:
            self.fabric_time = time.monotonic()
            try:
                proc = subprocess.run(["/usr/sbin/system_profiler", "-json", "SPThunderboltDataType"],
                                      capture_output=True, timeout=12, check=True)
                self.fabric = self._fabric(json.loads(proc.stdout).get("SPThunderboltDataType", []))
                self.fabric_error = None
            except Exception as exc:
                self.fabric, self.fabric_error = [], "Thunderbolt: %s" % exc
        if self.fabric_error:
            errors.append(self.fabric_error)
        return {"platform": "macOS", "power": power, "ports": ports, "devices": devices,
                "displays": displays, "fabric": self.fabric, "errors": errors,
                "limitations": ["未读取 SMC 单端口电流；未测线阻、柔软度或实际传输吞吐。",
                                "无 E-Marker 数据仅表示未读取到，不能据此判断为低速线或假线。",
                                "显示模式不等于面板最大能力；首版不判定 DSC 或完整 DP 带宽。"]}

    @staticmethod
    def _fabric(nodes, parent=""):
        result = []
        for n in nodes:
            path = parent + "/" + n.get("_name", "Thunderbolt")
            if parent:
                links = [v.get("current_speed_key") for k, v in n.items()
                         if k.startswith("receptacle_upstream") and isinstance(v, dict)]
                result.append({"name": n.get("device_name_key", n.get("_name")), "path": path,
                               "mode": n.get("mode_key", "未知"), "speed": next((x for x in links if x), "未知"),
                               "source": "system_profiler SPThunderboltDataType（15 秒缓存）"})
            result.extend(Backend._fabric(n.get("_items", []), path))
        return result
