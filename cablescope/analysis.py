"""Evidence-based diagnostics and connector diagrams; no licensing dependency."""
import math
from collections import deque
from .core import number


def pin_map(port):
    if port.get("kind") == "MagSafe 3":
        return {"rows": [[{"id": str(i+1), "signal": signal, "class": "fixed"}
                           for i, signal in enumerate(["GND", "电源", "识别", "电源", "GND"])]],
                "note": "MagSafe 五触点标准结构示意，不表示已逐针测量导通、电压或触点健康。"}
    config = port.get("pin_configuration") or {}
    if not config:
        return None
    def route(key):
        try:
            value = int(config[key])
        except (KeyError, ValueError, TypeError):
            return "未知", "unknown"
        if value == 0:
            return "空闲", "idle"
        if key.startswith("sbu"):
            return ("DP AUX", "dp") if value in (1, 2) else ("路由 %d" % value, "unknown")
        if value in (1, 2, 3, 4):
            return "USB 3", "usb"
        if value in (5, 6, 7, 8):
            return "DP %d" % (value-5), "dp"
        return "路由 %d" % value, "unknown"
    fixed = lambda value: (value, "fixed")
    top = [fixed("GND"), route("tx1"), route("tx1"), fixed("VBUS"), fixed("CC"), fixed("USB 2"),
           fixed("USB 2"), route("sbu1"), fixed("VBUS"), route("rx2"), route("rx2"), fixed("GND")]
    bottom = [fixed("GND"), route("rx1"), route("rx1"), fixed("VBUS"), route("sbu2"), fixed("USB 2"),
              fixed("USB 2"), fixed("CC"), fixed("VBUS"), route("tx2"), route("tx2"), fixed("GND")]
    rows = [[{"id": "A%d" % (i+1), "signal": s, "class": cls} for i, (s, cls) in enumerate(top)],
            [{"id": "B%d" % (12-i), "signal": s, "class": cls} for i, (s, cls) in enumerate(bottom)]]
    return {"rows": rows, "note": "面向母座的逻辑路由图；固定引脚仅标示用途，不代表逐针电气测量。未识别路由保留原始编号。"}


def mode_comparison(current, modes):
    """Compare actual mode pairs; never combine one mode's pixels with another's Hz."""
    usable = [m for m in modes if isinstance(m.get("width"), int) and isinstance(m.get("height"), int)
              and m["width"] > 0 and m["height"] > 0]
    if not usable:
        return {"highest_mode": None, "offered_mode_count": 0, "verdict": "系统未提供可比较的模式列表"}
    top = max(usable, key=lambda m: (m["width"] * m["height"], m.get("refresh_hz") or 0))
    if (current["width"], current["height"]) != (top["width"], top["height"]):
        verdict = "当前像素模式低于系统列出的最高分辨率；原因尚不能归属到线缆"
    elif number(current.get("refresh_hz")) is not None and number(top.get("refresh_hz")) is not None:
        verdict = "当前达到系统列出的最高模式" if current["refresh_hz"] >= top["refresh_hz"]-1 else "同分辨率下还有更高刷新率模式可选"
    else:
        verdict = "当前分辨率匹配系统最高模式；刷新率无法比较"
    return {"highest_mode": top, "offered_mode_count": len(usable), "verdict": verdict,
            "note": "比较的是操作系统列出的模式，不据此断言面板原生上限或 DP/HDMI 物理瓶颈。"}


def negotiation(port, power):
    contract, cable = port.get("contract") or {}, port.get("cable") or {}
    is_input = power.get("input_port_id") == port.get("id")
    live = port.get("input_power") or {}
    source_w = power.get("adapter_rated_w") if is_input else None
    cable_w = cable.get("power_ceiling_w")
    contract_w = contract.get("watts")
    notes = []
    if is_input and number(live.get("watts")) is not None and number(contract_w) is not None and contract_w > 0:
        notes.append("实际输入为合同上限的 %.0f%%；电量、温控与系统负载会影响取电" % (100*live["watts"]/contract_w))
    if number(cable_w) is not None and number(source_w) is not None and cable_w < source_w:
        notes.append("线缆声明上限低于电源公布值，存在容量差异；尚需电脑与电源完整档位确认实际瓶颈")
    if number(cable_w) is not None and number(contract_w) is not None and contract_w > cable_w*1.05:
        notes.append("合同超过已解析的线缆声明上限，请核对 E-Marker 格式、端口归属与协议版本")
    return {"source_reported_w": source_w, "cable_declared_w": cable_w, "contract_w": contract_w,
            "contract_voltage_v": contract.get("voltage_v"), "contract_current_a": contract.get("current_a"),
            "actual_input_w": live.get("watts"), "host_limit_w": None,
            "notes": notes or ["缺少完整链路上限，不生成线缆瓶颈结论"],
            "source": "不同来源分列：电源公布值 / 线缆芯片声明 / 协商合同 / 系统输入遥测"}


class ResistanceTracker:
    """Passive input-path regression; never causes load or changes PD settings."""
    def __init__(self):
        self.key = None
        self.samples = deque(maxlen=120)

    def observe(self, snapshot):
        power = snapshot.get("power", {})
        port = next((p for p in snapshot.get("ports", []) if p.get("id") == power.get("input_port_id")), None)
        cv, v, a = power.get("contract_voltage_v"), power.get("input_voltage_v"), power.get("input_current_a")
        note = "这是充电器、线缆、触点及内部路径共同影响的等效估计，不是线材直流电阻实测。"
        missing = {"status": "unavailable", "milliohms": None, "sample_count": 0, "note": note}
        if not port or power.get("external_connected") is not True or not power.get("independent_input_power") or not all(number(x) is not None for x in (cv, v, a)):
            self.key = None
            self.samples.clear()
            return missing
        key = (port["id"], port.get("plug_count"), (port.get("cable") or {}).get("fingerprint"), round(cv, 1))
        if key != self.key:
            self.key, self.samples = key, deque(maxlen=120)
        watts = number(power.get("input_w"))
        if not .3 <= a <= 6 or not cv-1.5 <= v <= cv+.5 or watts is None or abs(watts-v*a) > max(2, .15*watts):
            return {**missing, "status": "sample_rejected", "sample_count": len(self.samples)}
        self.samples.append((a, v))
        return {**self.fit(list(self.samples), cv), "note": note}

    @staticmethod
    def fit(samples, contract_v):
        result = {"status": "sampling", "milliohms": None, "sample_count": len(samples), "r_squared": None}
        if len(samples) < 24:
            return result
        currents, volts = zip(*samples)
        if max(currents)-min(currents) < .75:
            return {**result, "status": "insufficient_variation"}
        x, y = sum(currents)/len(samples), sum(volts)/len(samples)
        xx = sum((a-x)**2 for a in currents)
        yy = sum((v-y)**2 for v in volts)
        xy = sum((a-x)*(v-y) for a, v in samples)
        if xx <= 1e-9 or yy <= 1e-9:
            return {**result, "status": "insufficient_variation"}
        slope, r2 = xy/xx, xy*xy/(xx*yy)
        intercept = y-slope*x
        result.update({"r_squared": round(r2, 3), "current_span_a": round(max(currents)-min(currents), 2)})
        if not -.8 <= slope < -.001 or r2 < .8 or not contract_v-1.5 <= intercept <= contract_v+1:
            return {**result, "status": "unreliable"}
        return {**result, "status": "estimate", "milliohms": round(-slope*1000, 1)}


def enrich(snapshot):
    power = snapshot.get("power", {})
    for port in snapshot.get("ports", []):
        port["pin_map"] = pin_map(port)
        port["negotiation"] = negotiation(port, power)
    return snapshot
