"""Windows 10/11: documented Win32, USB hub queries and read-only CIM."""
import ctypes as c
import hashlib
import json
import struct
import subprocess
import time
import uuid
from ..core import scaled, number
from ..analysis import mode_comparison

U8, U16, U32 = c.c_uint8, c.c_uint16, c.c_uint32

IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES = 0x220458
USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH = 18
USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH = 65536
PORT_USER_CONNECTABLE = 0x1
PORT_DEBUG_CAPABLE = 0x2
PORT_HAS_MULTIPLE_COMPANIONS = 0x4
PORT_CONNECTOR_IS_TYPE_C = 0x8
PORT_PROPERTIES_KNOWN_BITS = PORT_USER_CONNECTABLE | PORT_DEBUG_CAPABLE | PORT_HAS_MULTIPLE_COMPANIONS | PORT_CONNECTOR_IS_TYPE_C
UCSI_STATE = ("available", "no_data_plane", "query_failed")


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


def decode_connector_properties(data):
    # USB_PORT_CONNECTOR_PROPERTIES is pack(1); WCHAR companion link starts at 16.
    if len(data) < USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH:
        raise ValueError("USB port connector properties reply is shorter than 18 bytes")
    index, length = struct.unpack_from("<II", data, 0)
    properties = struct.unpack_from("<I", data, 8)[0]
    companion_index, companion_port = struct.unpack_from("<HH", data, 12)
    if not USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH <= length <= USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH:
        raise ValueError("USB port connector properties ActualLength 无效")
    if not 1 <= index <= 0xffffffff:
        raise ValueError("USB port connector properties ConnectionIndex 无效")
    tail = data[16:length] if len(data) >= length else b""
    if len(tail) % 2:
        raise ValueError("USB port connector properties WCHAR 长度无效")
    link = tail.decode("utf-16-le", errors="replace").split("\x00", 1)[0]
    flags = properties & PORT_PROPERTIES_KNOWN_BITS
    return {"connection_index": index, "actual_length": length,
            "user_connectable": bool(flags & PORT_USER_CONNECTABLE),
            "debug_capable": bool(flags & PORT_DEBUG_CAPABLE),
            "multiple_companions": bool(flags & PORT_HAS_MULTIPLE_COMPANIONS),
            "type_c": bool(flags & PORT_CONNECTOR_IS_TYPE_C),
            "reserved_bits": properties & ~PORT_PROPERTIES_KNOWN_BITS,
            "companion_index": companion_index, "companion_port": companion_port,
            "companion_link": normal_hub_link(link)}


def normal_hub_link(link):
    return link.strip().replace("/", "\\").casefold()


def digest(*parts):
    return hashlib.sha256("\x00".join(parts).encode("utf-8")).hexdigest()[:16]


def companion_group_key(link, port):
    return digest(link, str(port))


def port_candidate(row, hub_link, label, ambiguous=False, discriminator=""):
    port = row["connection_index"]
    source = "Hub IOCTL 端口属性（候选，未确认机身物理口）"
    if ambiguous:
        source += "；伴口分组键冲突，保留独立行以避免误合并"
    identifier = digest(hub_link, str(port)) if not discriminator else digest(hub_link, str(port), discriminator)
    return {"id": identifier, "name": "USB-C 候选 · " + label,
            "number": port, "contract": None, "output_power": None, "input_power": None,
            "kind": "USB-C", "connected": None, "orientation": None, "cable": None,
            "overcurrent_count": None, "plug_count": None, "liquid_detected": None,
            "pin_configuration": {}, "source": source}


def read_connector_properties(ioctl, handle, port):
    # Stage one reads ActualLength; stage two supplies the exact required byte count.
    first = decode_connector_properties(ioctl(handle, IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES,
                                              struct.pack("<I", port), USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH))
    rows = []
    companion = 0
    while True:
        initial = struct.pack("<IIIHH", port, first["actual_length"], 0, companion, 0)
        reply = ioctl(handle, IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES, initial, first["actual_length"])
        row = decode_connector_properties(reply)
        if row["connection_index"] != port:
            raise ValueError("USB port connector properties ConnectionIndex 不匹配")
        rows.append(row)
        if not row["multiple_companions"] or row["companion_port"] == 0 or companion >= 255:
            break
        companion += 1
    return rows


def group_ambiguous(items):
    identities = {(i["row"]["companion_link"], i["row"]["companion_port"]) for i in items}
    if len(identities) > 1:
        return True
    # A zero companion port carries no companion identity; do not merge across hubs.
    return identities == {("", 0)} and len({(i["hub_link"], i["hub_index"]) for i in items}) > 1


def collect_port_candidates(rows, hub_link, hub_index):
    groups = {}
    for row in rows:
        if not row["user_connectable"] or row["debug_capable"]:
            continue
        if row["companion_port"] == 0:
            key = digest("no-companion", hub_link, str(row["connection_index"]))
        else:
            key = companion_group_key(row["companion_link"], row["companion_port"])
        groups.setdefault(key, []).append({"hub_link": hub_link, "hub_index": hub_index, "row": row})
    result = []
    for items in groups.values():
        if group_ambiguous(items):
            for item in items:
                row = item["row"]
                if not row["type_c"]:
                    continue
                discriminator = "%d:%s:%d" % (row["companion_index"], row["companion_link"], row["companion_port"])
                result.append(port_candidate(row, item["hub_link"],
                                             "Hub #%d Port %d" % (item["hub_index"], row["connection_index"]),
                                             True, discriminator))
            continue
        chosen = max(items, key=lambda i: (i["row"]["type_c"], -i["row"]["companion_index"]))
        row, item = chosen["row"], chosen
        if row["type_c"]:
            result.append(port_candidate(row, item["hub_link"],
                                         "Hub #%d Port %d" % (item["hub_index"], row["connection_index"])))
    result.sort(key=lambda p: p["name"])
    return result


def ucsi_limitations(state):
    if state == "available":
        return "探测到 UCM / UCSI 设备节点；本工具仅做只读存在性探测，未读取或解析 UCSI 命令、PDO / RDO 与线缆身份。E-Marker、PD 档位、电源角色与单端口输入功率仍保持未知。Hub IOCTL 仅识别 Type-C 端口候选与设备枚举位置；未启用测试接口、未修改注册表、未重启设备。"
    if state == "no_data_plane":
        return "本机固件未暴露可读取的 UCSI 设备节点或 WMI 数据面；只读 Hub IOCTL 仅识别 Type-C 端口候选与设备枚举位置。E-Marker、PD 档位、电源角色与单端口输入功率保持未知。未启用测试接口、未修改注册表、未重启设备。"
    return "UCSI 数据面探测查询未能完成，无法确认是否存在可读取的设备节点；本次不据此推断 PD 合同、电源角色、E-Marker 或单端口输入功率。只读 Hub IOCTL 仍可识别 Type-C 端口候选；未启用测试接口、未修改注册表、未重启设备。"


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
    result, errors, hub_index, port_candidates = [], [], 0, []

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
                hub_link = digest(normal_hub_link(path))  # hashed; never persist the raw path
                for port in range(1, count + 1):
                    try:
                        rows = read_connector_properties(ioctl, handle, port)
                        port_candidates.extend(collect_port_candidates(rows, hub_link, hub_index))
                    except (OSError, ValueError) as exc:
                        errors.append("Hub #%d Port %d 端口属性: %s" % (hub_index, port, exc))
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
    return result, errors, port_candidates


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
            display = {"name": d.name + " · " + d.label, "width": width, "height": height,
                           "refresh_hz": hz if hz > 1 else None,
                           "source": "EnumDisplaySettingsW 当前及可选模式（刷新率为整数）"}
            modes = []
            for index in range(4096):
                candidate = c.create_string_buffer(220)
                struct.pack_into("<H", candidate, 68, 220)
                if not user.EnumDisplaySettingsW(d.name, index, candidate):
                    break
                w, h = struct.unpack_from("<II", candidate, 172)
                rate = struct.unpack_from("<I", candidate, 184)[0]
                item = {"width": w, "height": h, "refresh_hz": rate if rate > 1 else None}
                if item not in modes:
                    modes.append(item)
            display.update(mode_comparison(display, modes))
            result.append(display)
    return result


PS_SCRIPT = r"""
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$b = @(); $devices = @(); $errors = @(); $ucsi_state = 'query_failed'
try { $b = @(Get-CimInstance -Namespace root/wmi -ClassName BatteryStatus | Select-Object PowerOnline,Charging,Discharging,ChargeRate,DischargeRate,Voltage) } catch { $errors += 'BatteryStatus unavailable' }
try { $devices = @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\*' } | Select-Object FriendlyName,InstanceId,Status) } catch { $errors += 'PnP inventory unavailable' }
try {
  $ucm = @()
  try { $ucm = @(Get-PnpDevice -Class UCM -ErrorAction Stop) } catch { if ($_.FullyQualifiedErrorId -notlike 'CmdletizationQuery_NotFound_PNPClass*') { throw } }
  $wmi = @()
  foreach ($ns in @('root/wmi','root/cimv2')) { $wmi += @(Get-CimClass -Namespace $ns -ClassName '*UCSI*' -ErrorAction Stop | Select-Object -First 1 Namespace,ClassName) }
  $named = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | Where-Object { $_.FriendlyName -match 'UCSI|UCM' -or $_.InstanceId -match 'UCSI|UCM' } | Select-Object -First 1 FriendlyName,InstanceId,Class)
  if ($ucm.Count -gt 0 -or $wmi.Count -gt 0 -or $named.Count -gt 0) { $ucsi_state = 'available' } else { $ucsi_state = 'no_data_plane' }
} catch { $ucsi_state = 'query_failed'; $errors += 'UCSI data plane probe unavailable' }
@{ batteries=$b; devices=$devices; ucsi_state=$ucsi_state; errors=$errors } | ConvertTo-Json -Depth 5 -Compress
"""


def classify_ucsi_state(inventory):
    state = inventory.get("ucsi_state")
    return state if state in UCSI_STATE else "query_failed"


def system_power(errors):
    kernel = c.WinDLL("kernel32", use_last_error=True)
    kernel.GetSystemPowerStatus.argtypes = [c.POINTER(PowerStatus)]
    state = PowerStatus()
    ok = kernel.GetSystemPowerStatus(c.byref(state))
    if not ok:
        errors.append("GetSystemPowerStatus: WinError %s" % c.get_last_error())
    return {"external_connected": (state.ac == 1) if ok and state.ac != 255 else None,
            "charging": bool(state.flags & 8) if ok and state.flags != 255 and not state.flags & 128 else None,
            "battery_percent": state.percent if ok and state.percent != 255 and not state.flags & 128 else None,
            "input_w": None, "adapter_rated_w": None, "input_voltage_v": None, "input_current_a": None,
            "battery_net_w": None, "profiles": [], "source": "GetSystemPowerStatus；输入功率未公开"}


class Backend:
    def __init__(self):
        self.inventory = {"devices": [], "batteries": [], "ucsi_state": "query_failed", "errors": []}
        self.inventory_time = -100

    def sample(self):
        errors = []
        power = system_power(errors)
        if time.monotonic() - self.inventory_time >= 15:
            self.inventory_time = time.monotonic()
            try:
                proc = subprocess.run(["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", PS_SCRIPT],
                                      capture_output=True, timeout=12, check=True, creationflags=0x08000000)
                self.inventory = json.loads(proc.stdout.decode("utf-8-sig"))
            except Exception as exc:
                self.inventory = {"devices": [], "batteries": [], "ucsi_state": "query_failed",
                                  "errors": ["CIM/PnP: %s" % exc]}
        errors.extend(self.inventory.get("errors", []))
        ucsi_state = classify_ucsi_state(self.inventory)
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
            devices, usb_errors, ports = usb_devices()
            errors.extend(usb_errors)
        except Exception as exc:
            devices, ports = [], []
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
        return {"platform": "Windows", "power": power, "ports": ports, "devices": devices, "fabric": [],
                "displays": screens, "errors": errors,
                "limitations": [ucsi_limitations(ucsi_state),
                                "USB Hub 端口不等于机身 USB-C 接口；端口候选状态未知，设备存在不证明物理连接态，当前速率不等于线缆最大能力。",
                                "电池净功率与适配器输入功率不同；未测线阻、柔软度或实际吞吐。"]}
