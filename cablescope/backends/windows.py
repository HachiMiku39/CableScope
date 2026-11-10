"""Windows 10/11: documented Win32, USB hub queries and read-only CIM."""
import ctypes as c
import json
import struct
import subprocess
import time
import uuid
from ..core import scaled, number

U8, U16, U32 = c.c_uint8, c.c_uint16, c.c_uint32


class PowerStatus(c.Structure):
    _fields_ = [("ac", U8), ("flags", U8), ("percent", U8), ("saver", U8),
                ("life", U32), ("full_life", U32)]


class InterfaceData(c.Structure):
    _fields_ = [("size", U32), ("guid", U8 * 16), ("flags", U32), ("reserved", c.c_size_t)]


def decode_connection(data, v2=None):
    # usbioctl.h uses pack(1): descriptor 18 bytes; status at offset 31.
    if len(data) < 35:
        raise ValueError("USB connection reply is shorter than 35 bytes")
    status = struct.unpack_from("<I", data, 31)[0]
    if status != 1:
        return None
    speed = {0: "1.5 Mbps", 1: "12 Mbps", 2: "480 Mbps", 3: "SuperSpeed（≥5 Gbps）"}.get(data[23], "未知")
    if v2 and len(v2) >= 16:
        flags = struct.unpack_from("<I", v2, 12)[0]
        if flags & 4:
            speed = "SuperSpeedPlus（≥10 Gbps，未确定 lane 数）"
        elif flags & 1:
            speed = "SuperSpeed（≥5 Gbps）"
    vid, pid = struct.unpack_from("<HH", data, 12)
    return {"vendor_id": vid, "product_id": pid, "speed": speed, "is_hub": bool(data[24]),
            "name": "USB %04X:%04X" % (vid, pid), "source": "USB Hub IOCTL EX / EX_V2；设备链路，不是线缆上限"}


def usb_devices():
    setup, kernel = c.WinDLL("setupapi", use_last_error=True), c.WinDLL("kernel32", use_last_error=True)
    setup.SetupDiGetClassDevsW.argtypes = [c.c_void_p, c.c_wchar_p, c.c_void_p, U32]
    setup.SetupDiGetClassDevsW.restype = c.c_void_p
    setup.SetupDiEnumDeviceInterfaces.argtypes = [c.c_void_p, c.c_void_p, c.c_void_p, U32, c.POINTER(InterfaceData)]
    setup.SetupDiGetDeviceInterfaceDetailW.argtypes = [c.c_void_p, c.POINTER(InterfaceData), c.c_void_p, U32, c.POINTER(U32), c.c_void_p]
    setup.SetupDiDestroyDeviceInfoList.argtypes = [c.c_void_p]
    kernel.CreateFileW.argtypes = [c.c_wchar_p, U32, U32, c.c_void_p, U32, U32, c.c_void_p]
    kernel.CreateFileW.restype = c.c_void_p
    kernel.CloseHandle.argtypes = [c.c_void_p]
    kernel.DeviceIoControl.argtypes = [c.c_void_p, U32, c.c_void_p, U32, c.c_void_p, U32, c.POINTER(U32), c.c_void_p]
    guid = (U8 * 16).from_buffer_copy(uuid.UUID("f18a0e88-c30c-11d0-8815-00a0c906bed8").bytes_le)
    invalid = c.c_void_p(-1).value
    devs = setup.SetupDiGetClassDevsW(c.byref(guid), None, None, 0x12)
    if devs == invalid:
        raise c.WinError(c.get_last_error())
    result, errors, hub_index = [], [], 0

    def ioctl(handle, code, initial, size):
        buf = c.create_string_buffer(size)
        c.memmove(buf, initial, min(len(initial), size))
        count = U32()
        if not kernel.DeviceIoControl(handle, code, buf, size, buf, size, c.byref(count), None):
            raise c.WinError(c.get_last_error())
        return buf.raw[:count.value]

    try:
        while True:
            info = InterfaceData()
            info.size = c.sizeof(info)
            if not setup.SetupDiEnumDeviceInterfaces(devs, None, c.byref(guid), hub_index, c.byref(info)):
                code = c.get_last_error()
                if code != 259:
                    errors.append("USB hub enumeration WinError %s" % code)
                break
            hub_index += 1
            required = U32()
            setup.SetupDiGetDeviceInterfaceDetailW(devs, c.byref(info), None, 0, c.byref(required), None)
            if not 8 <= required.value <= 65536:
                errors.append("USB hub 接口长度无效")
                continue
            detail = c.create_string_buffer(required.value)
            struct.pack_into("<I", detail, 0, 8 if c.sizeof(c.c_void_p) == 8 else 6)
            if not setup.SetupDiGetDeviceInterfaceDetailW(devs, c.byref(info), detail, required.value, None, None):
                errors.append("USB hub detail WinError %s" % c.get_last_error())
                continue
            path = c.wstring_at(c.addressof(detail) + 4)
            # FILE_ANY_ACCESS query IOCTLs; no port resets or descriptor requests.
            handle = kernel.CreateFileW(path, 0, 3, None, 3, 0, None)
            if handle == invalid:
                errors.append("Hub #%d 无法打开（WinError %s）" % (hub_index, c.get_last_error()))
                continue
            try:
                # USB_HUB_INFORMATION_EX: HubType(4), HighestPortNumber(2).
                hub = ioctl(handle, 0x220454, b"", 80)
                if len(hub) < 6:
                    raise ValueError("hub information truncated")
                count = min(struct.unpack_from("<H", hub, 4)[0], 255)
                for port in range(1, count + 1):
                    try:
                        data = ioctl(handle, 0x220448, struct.pack("<I", port), 4096)
                        try:
                            v2 = ioctl(handle, 0x22045c, struct.pack("<IIII", port, 16, 4, 0), 16)
                        except OSError:
                            v2 = None
                        device = decode_connection(data, v2)
                        if device:
                            device["location"] = "Hub #%d / Port %d" % (hub_index, port)
                            result.append(device)
                    except (OSError, ValueError) as exc:
                        errors.append("Hub #%d Port %d: %s" % (hub_index, port, exc))
            except (OSError, ValueError) as exc:
                errors.append("Hub #%d: %s" % (hub_index, exc))
            finally:
                kernel.CloseHandle(handle)
    finally:
        setup.SetupDiDestroyDeviceInfoList(devs)
    return result, errors


def displays():
    user = c.WinDLL("user32", use_last_error=True)
    # Fixed-size public Win32 structs; WCHAR is 16 bits on Windows.
    class DisplayDevice(c.Structure):
        _fields_ = [("cb", U32), ("name", c.c_wchar * 32), ("label", c.c_wchar * 128),
                    ("flags", U32), ("device_id", c.c_wchar * 128), ("key", c.c_wchar * 128)]
    user.EnumDisplayDevicesW.argtypes = [c.c_wchar_p, U32, c.POINTER(DisplayDevice), U32]
    user.EnumDisplaySettingsW.argtypes = [c.c_wchar_p, U32, c.c_void_p]
    result = []
    for i in range(64):
        d = DisplayDevice()
        d.cb = c.sizeof(d)
        if not user.EnumDisplayDevicesW(None, i, c.byref(d), 0):
            break
        if not d.flags & 1:
            continue
        mode = c.create_string_buffer(220)  # sizeof(DEVMODEW)
        struct.pack_into("<H", mode, 68, 220)
        if user.EnumDisplaySettingsW(d.name, 0xffffffff, mode):
            width, height = struct.unpack_from("<II", mode, 172)
            hz = struct.unpack_from("<I", mode, 184)[0]
            result.append({"name": d.name + " · " + d.label, "width": width, "height": height,
                           "refresh_hz": hz if hz > 1 else None,
                           "source": "EnumDisplaySettingsW 当前桌面模式（刷新率为整数）；未验证物理链路"})
    return result


PS_SCRIPT = r"""
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$b = @(); $devices = @(); $errors = @()
try { $b = @(Get-CimInstance -Namespace root/wmi -ClassName BatteryStatus | Select-Object PowerOnline,Charging,Discharging,ChargeRate,DischargeRate,Voltage) } catch { $errors += 'BatteryStatus unavailable' }
try { $devices = @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\*' } | Select-Object FriendlyName,InstanceId,Status) } catch { $errors += 'PnP inventory unavailable' }
@{ batteries=$b; devices=$devices; errors=$errors } | ConvertTo-Json -Depth 5 -Compress
"""


class Backend:
    def __init__(self):
        self.inventory = {"devices": [], "batteries": [], "errors": []}
        self.inventory_time = -100

    def sample(self):
        errors = []
        kernel = c.WinDLL("kernel32", use_last_error=True)
        kernel.GetSystemPowerStatus.argtypes = [c.POINTER(PowerStatus)]
        state = PowerStatus()
        ok = kernel.GetSystemPowerStatus(c.byref(state))
        power = {"external_connected": (state.ac == 1) if ok and state.ac != 255 else None,
                 "charging": bool(state.flags & 8) if ok and state.flags != 255 and not state.flags & 128 else None,
                 "battery_percent": state.percent if ok and state.percent != 255 and not state.flags & 128 else None,
                 "input_w": None, "adapter_rated_w": None, "input_voltage_v": None, "input_current_a": None,
                 "battery_net_w": None, "profiles": [], "source": "GetSystemPowerStatus；输入功率未公开"}
        if not ok:
            errors.append("GetSystemPowerStatus: WinError %s" % c.get_last_error())
        if time.monotonic() - self.inventory_time >= 15:
            self.inventory_time = time.monotonic()
            try:
                proc = subprocess.run(["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", PS_SCRIPT],
                                      capture_output=True, timeout=12, check=True, creationflags=0x08000000)
                self.inventory = json.loads(proc.stdout.decode("utf-8-sig"))
            except Exception as exc:
                self.inventory = {"devices": [], "batteries": [], "errors": ["CIM/PnP: %s" % exc]}
        errors.extend(self.inventory.get("errors", []))
        b = self.inventory.get("batteries", [])
        if len(b) == 1:
            row = b[0]
            charge, discharge = number(row.get("ChargeRate")), number(row.get("DischargeRate"))
            if row.get("Charging") and charge is not None and 0 <= charge < 0xffffffff:
                power["battery_net_w"] = scaled(row["ChargeRate"], 1000)
            elif row.get("Discharging") and discharge is not None and 0 <= discharge < 0xffffffff:
                power["battery_net_w"] = -scaled(row["DischargeRate"], 1000)
            power["battery_source"] = "root/wmi BatteryStatus，电池净充放电（15 秒缓存），不是 USB 输入功率"
        try:
            devices, usb_errors = usb_devices()
            errors.extend(usb_errors)
        except Exception as exc:
            devices = []
            errors.append("USB Hub: %s" % exc)
        # Retain friendly-name inventory even if hub access is denied.
        if not devices:
            devices = [{"name": d.get("FriendlyName") or "USB 设备", "speed": "未知", "vendor": "",
                        "source": "Get-PnpDevice（15 秒缓存）；设备存在不证明其使用 USB-C"}
                       for d in self.inventory.get("devices", [])]
        try:
            screens = displays()
        except Exception as exc:
            screens = []
            errors.append("显示器: %s" % exc)
        return {"platform": "Windows", "power": power, "ports": [], "devices": devices, "fabric": [],
                "displays": screens, "errors": errors,
                "limitations": ["首版未启用 UCSI 测试接口或安装驱动：E-Marker、PD 档位、单端口输入功率未知。",
                                "USB Hub 端口不等于机身 USB-C 接口；当前速率不等于线缆最大能力。",
                                "电池净功率与适配器输入功率不同；未测线阻、柔软度或实际吞吐。"]}
