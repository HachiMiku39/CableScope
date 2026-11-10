import datetime
import hashlib
import math


def number(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return value if math.isfinite(value) else None


def scaled(value, divisor):
    n = number(value)
    return round(n / divisor, 3) if n is not None else None


def signed64(value):
    n = number(value)
    if n is None:
        return None
    return n - (1 << 64) if n >= (1 << 63) else n


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")


def decode_identity(props):
    """Decode only known PD3 cable fields; retain ambiguous values explicitly."""
    meta = props.get("Metadata", {})
    blobs = meta.get("VDOs", [])
    words = [int.from_bytes(v, "little") for v in blobs
             if isinstance(v, bytes) and len(v) == 4]
    result = {
        "vendor_id": props.get("Vendor ID", meta.get("Vendor ID")),
        "product_id": props.get("Product ID", meta.get("Product ID")),
        "type": props.get("Product Type Description", meta.get("Product Type Description", "未知")),
        "vdo_hex": ["%08x" % w for w in words],
        "current_a": None, "power_ceiling_w": None, "speed": "未解析",
        "source": "IOPortTransportComponentCCUSBPDSOPp.Metadata.VDOs",
        "note": "E-Marker 是芯片声明，不等于实测速率、线阻或认证结论。",
    }
    if len(words) < 4 or props.get("Specification Revision") != 3:
        return result
    kind = (words[0] >> 27) & 7
    if kind not in (3, 4):
        return result
    w = words[3]
    result["speed"] = {0: "USB 2.0 · 480 Mbps", 1: "USB · 5 Gbps", 2: "USB · 10 Gbps",
                       3: "20 / 40 Gbps（PD 版本有歧义）", 4: "USB4 · 80 Gbps"}.get(w & 7, "保留编码，未知")
    result["current_a"] = {1: 3, 2: 5}.get((w >> 5) & 3)
    # PD3 passive cable: 50V insulation is not 50V delivered (EPR tops at 48V).
    voltage_code = (w >> 9) & 3
    if kind == 3 and result["current_a"] is not None:
        if voltage_code == 0:
            result["power_ceiling_w"] = result["current_a"] * 20
        elif voltage_code == 3 and w & (1 << 17):
            result["power_ceiling_w"] = result["current_a"] * 48
    result["fingerprint"] = hashlib.sha256(b"".join(v for v in blobs if isinstance(v, bytes))).hexdigest()[:16]
    return result


def diagnostics(snapshot):
    notes = []
    power = snapshot.get("power", {})
    if power.get("external_connected") is True and power.get("charging") is False:
        notes.append({"level": "info", "title": "已接电源，电池未充电",
                      "text": "可能是电量、温度或充电策略所致，单凭这个状态不能判断线缆故障。"})
    for port in snapshot.get("ports", []):
        if port.get("liquid_detected") is True:
            notes.append({"level": "warn", "title": port["name"] + " 报告液体检测",
                          "text": "这是控制器报告状态，请检查接口。"})
        if port.get("overcurrent_count", 0):
            notes.append({"level": "warn", "title": port["name"] + " 有过流累计计数",
                          "text": "累计计数不一定发生在当前线缆上，请结合插拔记录核对。"})
    notes.append({"level": "info", "title": "瓶颈结论需要完整链路证据",
                  "text": "当前 USB 链路速度、线缆声明和设备上限是不同的数据。缺少任何一项时，不判定线缆限制速度。"})
    return notes
