//! 基于证据的 USB-C 逻辑路由图、显示模式比较、协商分列与被动输入路径回归。
//!
//! 与 `cablescope/analysis.py` 对齐：快照用 `serde_json::Value`（键序保留），
//! 所有用户可见中文文案逐字一致。数值判定统一走 `cablescope_core::py` 的
//! `number` / `py_round`，以保留 Python 的类型、银行家舍入与浮点表示语义。

use std::collections::VecDeque;

use cablescope_core::py;
use serde_json::{json, Map, Value};

const PIN_MAP_NOTE: &str =
    "面向母座的逻辑路由图；固定引脚仅标示用途，不代表逐针电气测量。未识别路由保留原始编号。";
const MAGSAFE_NOTE: &str = "MagSafe 五触点标准结构示意，不表示已逐针测量导通、电压或触点健康。";
const MODE_NOTE: &str = "比较的是操作系统列出的模式，不据此断言面板原生上限或 DP/HDMI 物理瓶颈。";
const NEGOTIATION_SOURCE: &str =
    "不同来源分列：电源公布值 / 线缆芯片声明 / 协商合同 / 系统输入遥测";
const MISSING_NOTE: &str = "缺少完整链路上限，不生成线缆瓶颈结论";
const RESISTANCE_NOTE: &str =
    "这是充电器、线缆、触点及内部路径共同影响的等效估计，不是线材直流电阻实测。";

/// Python `dict.get(key)`：键缺失或显式 `null` 都返回 `None`。
///
/// 注意：Python 的 `.get()` 只回退缺失键，但本项目对协商字段的所有读取都紧接
/// `or {}` / `number(...)` / `is None`，显式 `null` 与缺失的结果完全一致，
/// 因此在此统一归一化为 `None`。布尔 `False`、0、空字符串仍保留原值。
fn get<'a>(object: &'a Value, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|v| !v.is_null())
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `number(value)` 的 f64 视图；`number()` 保留整数 JSON 类型，
/// 但本模块所有数值都参与浮点比较/运算，故统一取 `f64`。
fn num(value: Option<&Value>) -> Option<f64> {
    value.and_then(py::number).and_then(|v| v.as_f64())
}

fn json_number(value: f64) -> Value {
    json!(value)
}

/// CPython 3.12+ `sum()` 对 float 使用 Neumaier 补偿累加（`_PyFloat_Sum`），
/// 朴素左折叠会在 `x/y/xx/yy/xy` 上差 1 ulp，足以让 `slope >= -0.8` 这类
/// 门槛边缘的判定翻转。此处复刻该补偿求和，只用于 `ResistanceTracker::fit`。
fn py_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut total = 0.0_f64;
    let mut compensation = 0.0_f64;
    for value in values {
        let t = total + value;
        if total.abs() >= value.abs() {
            compensation += (total - t) + value;
        } else {
            compensation += (value - t) + total;
        }
        total = t;
    }
    total + compensation
}

/// Python `==` 在松散数值上的语义：`None == None`、`1 == 1.0 == True`。
/// `get()` 已把缺失与显式 null 归一成 `None`，故 (None, None) 必须为真。
fn py_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => match (a, b) {
            (Value::Bool(x), Value::Bool(y)) => x == y,
            (Value::Bool(x), Value::Number(n)) => Some(if *x { 1.0 } else { 0.0 }) == n.as_f64(),
            (Value::Number(n), Value::Bool(y)) => n.as_f64() == Some(if *y { 1.0 } else { 0.0 }),
            (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
            _ => a == b,
        },
        _ => false,
    }
}

/// 把值归一成 Python 元组键可比较的形状：数字用 f64（1 与 1.0 相等），
/// bool 用 1.0/0.0，其它原样保留。
fn py_key(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Bool(b)) => json!(if *b { 1.0 } else { 0.0 }),
        Some(Value::Number(n)) => match n.as_f64().filter(|f| f.is_finite()) {
            Some(f) => json_number(f),
            None => Value::Number(n.clone()),
        },
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

/// Python `int(value)`：整数、有限浮点（向零截断）、bool 与十进制整数字符串。
///
/// 对应 `route()` 的 `try: int(config[key])`，`KeyError/ValueError/TypeError`
/// 一律落到 `"未知"`。JSON 数值无法表达 NaN/Inf；越界的浮点按 Python
/// `int(float)` 抛 `OverflowError` 处理为未知。大整数用 `i128` 保留，
/// 超过 `i128` 时按 Python 的任意精度整数转十进制文本输出路由编号。
enum PyInt {
    Small(i128),
    Big(String),
}

impl PyInt {
    fn is_zero(&self) -> bool {
        match self {
            PyInt::Small(n) => *n == 0,
            PyInt::Big(s) => s.bytes().all(|b| b == b'0' || b == b'-'),
        }
    }

    fn in_range(&self, lo: i128, hi: i128) -> bool {
        match self {
            PyInt::Small(n) => lo <= *n && *n <= hi,
            PyInt::Big(_) => false,
        }
    }

    fn small(&self) -> Option<i128> {
        match self {
            PyInt::Small(n) => Some(*n),
            PyInt::Big(_) => None,
        }
    }

    fn display(&self) -> String {
        match self {
            PyInt::Small(n) => n.to_string(),
            PyInt::Big(s) => s.clone(),
        }
    }
}

fn py_int(value: &Value) -> Option<PyInt> {
    match value {
        Value::Bool(b) => Some(PyInt::Small(i128::from(*b))),
        Value::Number(n) => {
            if let Some(v) = n.as_i64() {
                return Some(PyInt::Small(v as i128));
            }
            if let Some(v) = n.as_u64() {
                return Some(PyInt::Small(v as i128));
            }
            let f = n.as_f64()?;
            if !f.is_finite() || f.abs() >= 170_141_183_460_469_231_731_687_303_715_884_105_728.0 {
                return None;
            }
            Some(PyInt::Small(f.trunc() as i128))
        }
        Value::String(s) => parse_py_int_str(s),
        _ => None,
    }
}

/// Python `int(str)`：允许 ASCII 空白、正负号、下划线分组与 Unicode 十进制数字，
/// 但不允许小数点、指数、十六进制前缀或内部空白。
fn parse_py_int_str(s: &str) -> Option<PyInt> {
    let trimmed = s.trim_matches(|c: char| c.is_whitespace());
    if trimmed.is_empty() {
        return None;
    }
    let (negative, digits) = match trimmed.as_bytes()[0] {
        b'+' => (false, &trimmed[1..]),
        b'-' => (true, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty() {
        return None;
    }
    let mut decimal = String::with_capacity(digits.len());
    let mut previous_was_digit = false;
    for ch in digits.chars() {
        if ch == '_' {
            if !previous_was_digit {
                return None;
            }
            previous_was_digit = false;
            continue;
        }
        // Python `int()` 接受 Unicode Nd 十进制数字；Rust `to_digit(10)`
        // 只认 ASCII，故用 Nd 数字块起点表归一化。
        let digit = ch
            .to_digit(10)
            .or_else(|| py_decimal_digit(ch).map(u32::from))?;
        decimal.push(char::from(b'0' + digit as u8));
        previous_was_digit = true;
    }
    if !previous_was_digit {
        return None;
    }
    let normalized = decimal.trim_start_matches('0');
    let normalized = if normalized.is_empty() {
        "0"
    } else {
        normalized
    };
    let text = if negative && normalized != "0" {
        format!("-{normalized}")
    } else {
        normalized.to_string()
    };
    match text.parse::<i128>() {
        Ok(v) => Some(PyInt::Small(v)),
        Err(_) => Some(PyInt::Big(text)),
    }
}

/// Unicode `Nd`（十进制数字）各连续 10 码点块的零字符；`py -3 unicodedata`
/// 差分的 68 个块。块内偏移 0..9 就是该数字的值，跨块混用也按 Python 允许。
fn py_decimal_digit(ch: char) -> Option<u8> {
    const ZEROS: &[u32] = &[
        0x0030, 0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66,
        0x0CE6, 0x0D66, 0x0DE6, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946,
        0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0,
        0xA9F0, 0xAA50, 0xABF0, 0xFF10, 0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0,
        0x112F0, 0x11450, 0x114D0, 0x11650, 0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50,
        0x11DA0, 0x11F50, 0x16A60, 0x16AC0, 0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6,
        0x1E140, 0x1E2F0, 0x1E4F0, 0x1E950, 0x1FBF0,
    ];
    let cp = ch as u32;
    ZEROS
        .iter()
        .find(|zero| cp >= **zero && cp < **zero + 10)
        .map(|zero| (cp - *zero) as u8)
}

/// Python `isinstance(v, int)` 且 `v > 0`：bool 是 int 子类，`True == 1`；
/// 浮点（哪怕 1920.0）与字符串一律不算整数。Python 大整数任意精度，
/// 这里用 `f64` 值参与像素数比较以保留 `max` 键的浮点语义。
fn py_positive_int(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Bool(b) => (*b).then_some(1.0),
        Value::Number(n) => n
            .as_i64()
            .map(|v| v as f64)
            .or_else(|| n.as_u64().map(|v| v as f64))
            .filter(|v| *v > 0.0),
        _ => None,
    }
}

/// Python `m.get("refresh_hz") or 0`，随后与整数像素数相乘。
/// 非数值按 0，bool 按 1/0；`py::number` 排除 bool/字符串，故这里显式处理。
fn refresh_score(mode: &Value) -> f64 {
    let value = get(mode, "refresh_hz").unwrap_or(&Value::Null);
    if let Value::Bool(b) = value {
        return if *b { 1.0 } else { 0.0 };
    }
    num(Some(value)).unwrap_or(0.0)
}

fn pin(id: &str, signal: &str, class: &str) -> Value {
    json!({"id": id, "signal": signal, "class": class})
}

fn route(config: &Value, key: &str) -> (String, &'static str) {
    let Some(value) = config.get(key).filter(|v| !v.is_null()) else {
        return ("未知".to_string(), "unknown");
    };
    let Some(value) = py_int(value) else {
        return ("未知".to_string(), "unknown");
    };
    if value.is_zero() {
        return ("空闲".to_string(), "idle");
    }
    if key.starts_with("sbu") {
        return if value.in_range(1, 2) {
            ("DP AUX".to_string(), "dp")
        } else {
            (format!("路由 {}", value.display()), "unknown")
        };
    }
    if value.in_range(1, 4) {
        return ("USB 3".to_string(), "usb");
    }
    if let Some(n) = value.small().filter(|n| (5..=8).contains(n)) {
        return (format!("DP {}", n - 5), "dp");
    }
    (format!("路由 {}", value.display()), "unknown")
}

/// USB-C 24 引脚逻辑路由图；MagSafe 3 固定 5 触点，缺配置返回 `None`。
///
/// 不推断 Thunderbolt：未识别的路由编号原样显示为「路由 N」，无法解析时显示「未知」。
pub fn pin_map(port: &Value) -> Option<Value> {
    if port.get("kind").and_then(Value::as_str) == Some("MagSafe 3") {
        let signals = ["GND", "电源", "识别", "电源", "GND"];
        let row = signals
            .iter()
            .enumerate()
            .map(|(i, signal)| pin(&(i + 1).to_string(), signal, "fixed"))
            .collect::<Vec<_>>();
        return Some(json!({"rows": [row], "note": MAGSAFE_NOTE}));
    }
    // Python `config = port.get(...) or {}; if not config: return None`。
    // 真值但非对象的 config 会继续走 `route()`，其 `config[key]` 抛 TypeError
    // 并被捕获为「未知」，等价于用空对象读取全部键。
    let config = match port.get("pin_configuration") {
        Some(v) if py_truthy(v) => match v {
            Value::Object(o) => Value::Object(o.clone()),
            _ => empty_object(),
        },
        _ => return None,
    };
    let fixed = |value: &str| (value.to_string(), "fixed");
    let top = [
        fixed("GND"),
        route(&config, "tx1"),
        route(&config, "tx1"),
        fixed("VBUS"),
        fixed("CC"),
        fixed("USB 2"),
        fixed("USB 2"),
        route(&config, "sbu1"),
        fixed("VBUS"),
        route(&config, "rx2"),
        route(&config, "rx2"),
        fixed("GND"),
    ];
    let bottom = [
        fixed("GND"),
        route(&config, "rx1"),
        route(&config, "rx1"),
        fixed("VBUS"),
        route(&config, "sbu2"),
        fixed("USB 2"),
        fixed("USB 2"),
        fixed("CC"),
        fixed("VBUS"),
        route(&config, "tx2"),
        route(&config, "tx2"),
        fixed("GND"),
    ];
    let rows = [
        top.iter()
            .enumerate()
            .map(|(i, (s, cls))| pin(&format!("A{}", i + 1), s, cls))
            .collect::<Vec<_>>(),
        bottom
            .iter()
            .enumerate()
            .map(|(i, (s, cls))| pin(&format!("B{}", 12 - i), s, cls))
            .collect::<Vec<_>>(),
    ];
    Some(json!({"rows": rows, "note": PIN_MAP_NOTE}))
}

/// 比较系统列出的模式对；绝不把某个模式的像素与另一个模式的刷新率拼在一起。
///
/// Python `max()` 并列时保留**第一个**：此处用严格 `>` 迭代而非 `max_by_key`
/// （后者返回最后一个）。键为 `(width*height, refresh_hz or 0)`。
///
/// Python 的 `current["width"]` 缺键时抛 `KeyError`；Rust 版返回 `Result`，
/// `Err("current 缺少 width/height")` 保留该失败语义（见 `py_eq` 注释）。
pub fn mode_comparison(current: &Value, modes: &[Value]) -> Result<Value, &'static str> {
    // Python 列表推导的 `m.get(...)` 对非对象 m 抛 AttributeError；此处只在
    // 对象是 dict 时读取字段，非对象按缺字段过滤（不生成错误的模式列表）。
    let usable: Vec<&Value> = modes
        .iter()
        .filter(|m| {
            if !m.is_object() {
                return false;
            }
            let w = py_positive_int(get(m, "width"));
            let h = py_positive_int(get(m, "height"));
            w.is_some() && h.is_some()
        })
        .collect();
    if usable.is_empty() {
        return Ok(json!({
            "highest_mode": Value::Null,
            "offered_mode_count": 0,
            "verdict": "系统未提供可比较的模式列表"
        }));
    }
    // Python `current["width"]` / `current["height"]` 缺键会抛 KeyError；
    // 在函数入口就显式保留该失败语义（下游不把缺字段当成低分辨率）。
    let current_w = current.get("width").ok_or("current 缺少 width/height")?;
    let current_h = current.get("height").ok_or("current 缺少 width/height")?;
    let mut top = usable[0];
    let mut top_key = mode_key(top);
    for candidate in &usable[1..] {
        let key = mode_key(candidate);
        if key > top_key {
            top = candidate;
            top_key = key;
        }
    }
    // Python 直接比较 `(current["width"], current["height"])` 与最高模式的
    // 同名值：1 == 1.0 == True，不做整数类型过滤。
    let verdict = if !(py_eq(Some(current_w), get(top, "width"))
        && py_eq(Some(current_h), get(top, "height")))
    {
        "当前像素模式低于系统列出的最高分辨率；原因尚不能归属到线缆"
    } else {
        let current_hz = get(current, "refresh_hz").and_then(py::number);
        let top_hz = get(top, "refresh_hz").and_then(py::number);
        match (current_hz, top_hz) {
            (Some(c), Some(t)) => {
                let c = c.as_f64().unwrap_or(0.0);
                let t = t.as_f64().unwrap_or(0.0);
                if c >= t - 1.0 {
                    "当前达到系统列出的最高模式"
                } else {
                    "同分辨率下还有更高刷新率模式可选"
                }
            }
            _ => "当前分辨率匹配系统最高模式；刷新率无法比较",
        }
    };
    Ok(json!({
        "highest_mode": top,
        "offered_mode_count": usable.len(),
        "verdict": verdict,
        "note": MODE_NOTE
    }))
}

/// Python `max` 的比较键 `(width*height, refresh_hz or 0)`：像素乘积在 Python
/// 是任意精度整数，这里按 JSON 数值的 `f64` 视图计算，与后续 `>=` 判定一致。
fn mode_key(mode: &Value) -> (f64, f64) {
    let w = py_positive_int(get(mode, "width")).unwrap_or(0.0);
    let h = py_positive_int(get(mode, "height")).unwrap_or(0.0);
    (w * h, refresh_score(mode))
}

/// 电源公布值 / 线缆芯片声明 / 协商合同 / 系统输入遥测分列；`host_limit_w` 恒为 None。
///
/// 所有数值判定走 `py::number`（排除 bool/字符串）；百分比用 `format!("{:.0}")`
/// 复刻 Python `"%.0f" %%` 的 C printf half-even 舍入。
pub fn negotiation(port: &Value, power: &Value) -> Value {
    let empty = empty_object();
    let contract = object_or_empty(get(port, "contract"), &empty);
    let cable = object_or_empty(get(port, "cable"), &empty);
    let live = object_or_empty(get(port, "input_power"), &empty);
    // Python `==` 允许 None == None（两者都缺 id 时也算输入端口），且 1 == 1.0 == True。
    let is_input = py_eq(get(power, "input_port_id"), get(port, "id"));
    let source_w = if is_input {
        get(power, "adapter_rated_w").cloned()
    } else {
        None
    };
    let cable_w = get(cable, "power_ceiling_w").cloned();
    let contract_w = get(contract, "watts").cloned();
    let mut notes: Vec<Value> = Vec::new();

    let live_w = get(live, "watts");
    let live_n = live_w.and_then(py::number);
    let contract_n = contract_w.as_ref().and_then(py::number);
    if is_input {
        if let (Some(l), Some(c)) = (live_n.as_ref(), contract_n.as_ref()) {
            let l = l.as_f64().unwrap_or(0.0);
            let c = c.as_f64().unwrap_or(0.0);
            if c > 0.0 {
                // `py -3` 差分：12.5→12%、13.5→14%、74.7126→75%、49.75→50%、
                // 26.75→27%；`%.0f` 是 C printf 的 half-even，不是 Python round()。
                notes.push(json!(format!(
                    "实际输入为合同上限的 {:.0}%；电量、温控与系统负载会影响取电",
                    100.0 * l / c
                )));
            }
        }
    }
    let source_n = source_w.as_ref().and_then(py::number);
    let cable_n = cable_w.as_ref().and_then(py::number);
    if let (Some(cw), Some(sw)) = (cable_n.as_ref(), source_n.as_ref()) {
        let cw = cw.as_f64().unwrap_or(0.0);
        let sw = sw.as_f64().unwrap_or(0.0);
        if cw < sw {
            notes.push(json!(
                "线缆声明上限低于电源公布值，存在容量差异；尚需电脑与电源完整档位确认实际瓶颈"
            ));
        }
    }
    if let (Some(cw), Some(cn)) = (cable_n.as_ref(), contract_n.as_ref()) {
        let cw = cw.as_f64().unwrap_or(0.0);
        let cn = cn.as_f64().unwrap_or(0.0);
        if cn > cw * 1.05 {
            notes.push(json!(
                "合同超过已解析的线缆声明上限，请核对 E-Marker 格式、端口归属与协议版本"
            ));
        }
    }
    let notes = if notes.is_empty() {
        json!([MISSING_NOTE])
    } else {
        Value::Array(notes)
    };
    json!({
        "source_reported_w": source_w,
        "cable_declared_w": cable_w,
        "contract_w": contract_w,
        "contract_voltage_v": get(contract, "voltage_v").cloned(),
        "contract_current_a": get(contract, "current_a").cloned(),
        "actual_input_w": live_w.cloned(),
        "host_limit_w": Value::Null,
        "notes": notes,
        "source": NEGOTIATION_SOURCE
    })
}

/// Python `port.get("contract") or {}`：空/假值回退到空对象，非空对象原样使用。
fn object_or_empty<'a>(value: Option<&'a Value>, empty: &'a Value) -> &'a Value {
    match value {
        Some(v @ Value::Object(_)) => v,
        Some(other) if py_truthy(other) => other,
        _ => empty,
    }
}

/// Python `tuple` 键：端口 id、插拔次数、线缆指纹、`round(cv, 1)`。
/// 用 `serde_json::Value` 数组表达，便于与 JSON 快照里的任意值比较。
type TrackerKey = Vec<Value>;

/// 被动输入路径等效电阻回归；不制造负载、不修改 PD 设置。
///
/// `samples` 是 `VecDeque<(current_a, voltage_v)>`，容量 120（Python `deque(maxlen=120)`）。
pub struct ResistanceTracker {
    key: Option<TrackerKey>,
    samples: VecDeque<(f64, f64)>,
}

impl Default for ResistanceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ResistanceTracker {
    pub fn new() -> Self {
        Self {
            key: None,
            samples: VecDeque::with_capacity(120),
        }
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// 观察一份快照；证据不足时清空状态并返回 `unavailable`。
    pub fn observe(&mut self, snapshot: &Value) -> Value {
        let empty = empty_object();
        let power = object_or_empty(get(snapshot, "power"), &empty);
        let ports = match get(snapshot, "ports") {
            Some(Value::Array(a)) => a.as_slice(),
            _ => &[],
        };
        let port = ports
            .iter()
            .find(|p| py_eq(get(p, "id"), get(power, "input_port_id")))
            .unwrap_or(&empty);
        let cv = get(power, "contract_voltage_v");
        let v = get(power, "input_voltage_v");
        let a = get(power, "input_current_a");
        let cv_n = num(cv);
        let v_n = num(v);
        let a_n = num(a);
        let missing = || {
            json!({
                "status": "unavailable", "milliohms": Value::Null,
                "sample_count": 0, "note": RESISTANCE_NOTE
            })
        };
        let port_ok = py_truthy(port);
        let external = power.get("external_connected") == Some(&Value::Bool(true));
        let independent = py_truthy(get(power, "independent_input_power").unwrap_or(&Value::Null));
        if !port_ok || !external || !independent || cv_n.is_none() || v_n.is_none() || a_n.is_none()
        {
            self.key = None;
            self.samples.clear();
            return missing();
        }
        let cv_value = cv_n.unwrap();
        let v_value = v_n.unwrap();
        let a_value = a_n.unwrap();
        let cable = object_or_empty(get(port, "cable"), &empty);
        let key = vec![
            py_key(port.get("id")),
            py_key(get(port, "plug_count")),
            py_key(get(cable, "fingerprint")),
            py_key(Some(&json_number(py::py_round(cv_value, 1)))),
        ];
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.samples.clear();
        }
        let watts = num(get(power, "input_w"));
        let rejected = !(0.3..=6.0).contains(&a_value)
            || !(cv_value - 1.5..=cv_value + 0.5).contains(&v_value)
            || watts.is_none()
            || (watts.unwrap() - v_value * a_value).abs() > 2.0f64.max(0.15 * watts.unwrap());
        if rejected {
            return json!({
                "status": "sample_rejected", "milliohms": Value::Null,
                "sample_count": self.samples.len(), "note": RESISTANCE_NOTE
            });
        }
        if self.samples.len() == 120 {
            self.samples.pop_front();
        }
        self.samples.push_back((a_value, v_value));
        let samples: Vec<(f64, f64)> = self.samples.iter().copied().collect();
        let mut result = Self::fit(&samples, cv_value);
        if let Value::Object(ref mut map) = result {
            map.insert("note".into(), json!(RESISTANCE_NOTE));
        }
        result
    }

    /// 当前键下的样本数（Python `len(self.samples)`），用于外部观察队列容量。
    pub fn samples(&self) -> &VecDeque<(f64, f64)> {
        &self.samples
    }

    /// 对 `(电流, 电压)` 样本做最小二乘回归；门槛与中文状态逐字对齐 Python。
    pub fn fit(samples: &[(f64, f64)], contract_v: f64) -> Value {
        let mut result = json!({
            "status": "sampling", "milliohms": Value::Null,
            "sample_count": samples.len(), "r_squared": Value::Null
        });
        if samples.len() < 24 {
            return result;
        }
        let currents = samples.iter().map(|(a, _)| *a);
        let min_a = currents.clone().fold(f64::INFINITY, f64::min);
        let max_a = currents.fold(f64::NEG_INFINITY, f64::max);
        if max_a - min_a < 0.75 {
            result["status"] = json!("insufficient_variation");
            return result;
        }
        let n = samples.len() as f64;
        // CPython 3.12+ 的 `sum()` 对 float 走 Neumaier 补偿累加；见 `py_sum`。
        let x = py_sum(samples.iter().map(|(a, _)| *a)) / n;
        let y = py_sum(samples.iter().map(|(_, v)| *v)) / n;
        let xx = py_sum(samples.iter().map(|(a, _)| (a - x) * (a - x)));
        let yy = py_sum(samples.iter().map(|(_, v)| (v - y) * (v - y)));
        let xy = py_sum(samples.iter().map(|(a, v)| (a - x) * (v - y)));
        if xx <= 1e-9 || yy <= 1e-9 {
            result["status"] = json!("insufficient_variation");
            return result;
        }
        let slope = xy / xx;
        let r2 = xy * xy / (xx * yy);
        let intercept = y - slope * x;
        result["r_squared"] = json_number(py::py_round(r2, 3));
        result["current_span_a"] = json_number(py::py_round(max_a - min_a, 2));
        if !(slope >= -0.8 && slope < -0.001)
            || r2 < 0.8
            || !(contract_v - 1.5..=contract_v + 1.0).contains(&intercept)
        {
            result["status"] = json!("unreliable");
            return result;
        }
        result["status"] = json!("estimate");
        result["milliohms"] = json_number(py::py_round(-slope * 1000.0, 1));
        result
    }
}

/// 为每个 port 写入 `pin_map` 与 `negotiation`。
///
/// Python 版**原地修改**并返回同一个 dict；Rust 版取 `&mut Value` 原地写入每个
/// port，返回 `()`（调用方持有的仍是同一对象，语义等价；避免克隆整个快照）。
/// 若后续波次需要链式调用，可改为返回 `&mut Value`。
pub fn enrich(snapshot: &mut Value) {
    let empty = empty_object();
    let power = object_or_empty(get(snapshot, "power"), &empty).clone();
    if let Some(Value::Array(ports)) = snapshot.get_mut("ports") {
        for port in ports.iter_mut() {
            if let Value::Object(map) = port {
                // Python 先写 pin_map 再算 negotiation，键序也随之固定；
                // negotiation 只读 contract/cable/input_power，与 pin_map 无依赖。
                let view = Value::Object(map.clone());
                let pins = pin_map(&view);
                map.insert("pin_map".into(), pins.unwrap_or(Value::Null));
                map.insert("negotiation".into(), negotiation(&view, &power));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mode(w: i64, h: i64, hz: i64) -> Value {
        json!({"width": w, "height": h, "refresh_hz": hz})
    }

    // tests/test_features.py::DiagnosticTests 的 analysis 用例。
    #[test]
    fn test_pin_numbers_and_unknown_route_preserved() {
        let port = json!({
            "kind": "USB-C",
            "pin_configuration": {"tx1": 1, "rx1": 5, "tx2": 0, "rx2": 12, "sbu1": 1, "sbu2": 0}
        });
        let result = pin_map(&port).unwrap();
        let rows = result["rows"].as_array().unwrap();
        let ids: std::collections::HashSet<&str> = rows
            .iter()
            .flat_map(|row| row.as_array().unwrap())
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 24);
        assert_eq!(rows[0][1]["signal"], json!("USB 3"));
        assert_eq!(rows[1][1]["signal"], json!("DP 0"));
        assert_eq!(rows[0][9]["signal"], json!("路由 12"));
        assert_eq!(rows[1][0]["id"], json!("B12"));
        // `py -3` 逐字对照：整张逻辑路由图（含 note）。
        assert_eq!(
            result,
            json!({
                "rows": [
                    [{"id":"A1","signal":"GND","class":"fixed"},{"id":"A2","signal":"USB 3","class":"usb"},
                     {"id":"A3","signal":"USB 3","class":"usb"},{"id":"A4","signal":"VBUS","class":"fixed"},
                     {"id":"A5","signal":"CC","class":"fixed"},{"id":"A6","signal":"USB 2","class":"fixed"},
                     {"id":"A7","signal":"USB 2","class":"fixed"},{"id":"A8","signal":"DP AUX","class":"dp"},
                     {"id":"A9","signal":"VBUS","class":"fixed"},{"id":"A10","signal":"路由 12","class":"unknown"},
                     {"id":"A11","signal":"路由 12","class":"unknown"},{"id":"A12","signal":"GND","class":"fixed"}],
                    [{"id":"B12","signal":"GND","class":"fixed"},{"id":"B11","signal":"DP 0","class":"dp"},
                     {"id":"B10","signal":"DP 0","class":"dp"},{"id":"B9","signal":"VBUS","class":"fixed"},
                     {"id":"B8","signal":"空闲","class":"idle"},{"id":"B7","signal":"USB 2","class":"fixed"},
                     {"id":"B6","signal":"USB 2","class":"fixed"},{"id":"B5","signal":"CC","class":"fixed"},
                     {"id":"B4","signal":"VBUS","class":"fixed"},{"id":"B3","signal":"空闲","class":"idle"},
                     {"id":"B2","signal":"空闲","class":"idle"},{"id":"B1","signal":"GND","class":"fixed"}]
                ],
                "note": PIN_MAP_NOTE
            })
        );
    }

    #[test]
    fn test_magsafe_is_labelled_as_schematic() {
        let result = pin_map(&json!({"kind": "MagSafe 3"})).unwrap();
        assert_eq!(result["rows"][0].as_array().unwrap().len(), 5);
        assert!(result["note"].as_str().unwrap().contains("不表示已逐针"));
        // `py -3` 逐字对照：5 触点 id 为 "1".."5"，class 全为 fixed。
        assert_eq!(
            result,
            json!({
                "rows": [[
                    {"id":"1","signal":"GND","class":"fixed"},
                    {"id":"2","signal":"电源","class":"fixed"},
                    {"id":"3","signal":"识别","class":"fixed"},
                    {"id":"4","signal":"电源","class":"fixed"},
                    {"id":"5","signal":"GND","class":"fixed"}
                ]],
                "note": MAGSAFE_NOTE
            })
        );
        assert_eq!(pin_map(&json!({"kind": "USB-C"})), None);
        assert_eq!(
            pin_map(&json!({"kind": "USB-C", "pin_configuration": {}})),
            None
        );
    }

    #[test]
    fn test_display_maximum_does_not_mix_two_modes() {
        let modes = vec![mode(3840, 2160, 60), mode(1920, 1080, 240)];
        let result = mode_comparison(&modes[1], &modes).unwrap();
        assert_eq!(result["highest_mode"]["refresh_hz"], json!(60));
        assert_eq!(result["highest_mode"]["width"], json!(3840));
        assert_eq!(result["offered_mode_count"], json!(2));
        // `py -3` 逐字对照：高像素模式的 60Hz 不被低像素模式的 240Hz 替换。
        assert_eq!(
            result["verdict"],
            json!("当前像素模式低于系统列出的最高分辨率；原因尚不能归属到线缆")
        );
        assert_eq!(result["note"], json!(MODE_NOTE));
    }

    #[test]
    fn test_global_charger_not_assigned_to_output_port() {
        let port = json!({"id": "out", "cable": {"power_ceiling_w": 60}});
        let power = json!({"adapter_rated_w": 140, "input_port_id": "in"});
        let result = negotiation(&port, &power);
        assert_eq!(result["source_reported_w"], Value::Null);
        assert_eq!(result["host_limit_w"], Value::Null);
        assert_eq!(result["cable_declared_w"], json!(60));
        // `py -3` 逐字对照：无实测/合同时只保留证据不足 note。
        assert_eq!(result["notes"], json!([MISSING_NOTE]));
        assert_eq!(result["source"], json!(NEGOTIATION_SOURCE));
    }

    // tests/test_features.py::ResistanceTests 的 analysis 用例。
    #[test]
    fn test_known_input_path_slope() {
        let samples = slope_samples(40, -0.1, 20.0, true);
        let result = ResistanceTracker::fit(&samples, 20.0);
        assert_eq!(result["status"], json!("estimate"));
        assert!(
            (result["milliohms"].as_f64().unwrap() - 100.0).abs() <= 1.0,
            "{}",
            result["milliohms"]
        );
        assert!(result["r_squared"].as_f64().unwrap() > 0.99);
        // `py -3` 逐字对照（40 点）：milliohms=99.9, r_squared=1.0,
        // current_span_a=2.73, sample_count=40, status="estimate"。
        assert_eq!(
            result,
            json!({
                "status": "estimate", "milliohms": 99.9, "sample_count": 40,
                "r_squared": 1.0, "current_span_a": 2.73
            })
        );
    }

    #[test]
    fn test_flat_current_does_not_produce_estimate() {
        let samples = vec![(1.0, 19.9); 40];
        let result = ResistanceTracker::fit(&samples, 20.0);
        assert_eq!(result["milliohms"], Value::Null);
        assert_eq!(result["status"], json!("insufficient_variation"));
        // `py -3` 对照：电流全相同 -> max-min=0 < .75，且没有 r_squared/span。
        assert_eq!(
            result,
            json!({
                "status": "insufficient_variation", "milliohms": Value::Null,
                "sample_count": 40, "r_squared": Value::Null
            })
        );
    }

    #[test]
    fn test_positive_slope_not_turned_into_resistance() {
        let samples = slope_samples(40, 0.1, 20.0, false);
        let result = ResistanceTracker::fit(&samples, 20.0);
        assert_eq!(result["milliohms"], Value::Null);
        assert_eq!(result["status"], json!("unreliable"));
        // `py -3` 对照：正斜率触发 `not -.8 <= slope < -.001`。
        assert_eq!(
            result,
            json!({
                "status": "unreliable", "milliohms": Value::Null, "sample_count": 40,
                "r_squared": 1.0, "current_span_a": 2.73
            })
        );
    }

    #[test]
    fn test_contract_change_resets_samples_and_battery_clears() {
        let mut tracker = ResistanceTracker::new();
        let mut snapshot = json!({
            "power": {"external_connected": true, "input_port_id": "a", "contract_voltage_v": 20,
                      "input_voltage_v": 19.9, "input_current_a": 1, "input_w": 19.9,
                      "independent_input_power": true},
            "ports": [{"id": "a", "plug_count": 1}]
        });
        // `py -3` 对照：连续两次 observe 的 sample_count 为 1、2，status="sampling"。
        assert_eq!(tracker.observe(&snapshot)["sample_count"], json!(1));
        assert_eq!(tracker.observe(&snapshot)["sample_count"], json!(2));
        snapshot["ports"][0]["plug_count"] = json!(2);
        assert_eq!(tracker.observe(&snapshot)["sample_count"], json!(1));
        snapshot["power"]["external_connected"] = json!(false);
        let cleared = tracker.observe(&snapshot);
        assert_eq!(cleared["sample_count"], json!(0));
        assert_eq!(cleared["status"], json!("unavailable"));
        assert_eq!(cleared["note"], json!(RESISTANCE_NOTE));
        assert_eq!(tracker.sample_count(), 0);
    }

    // 差分与边界增强：`py -3 -c` 生成的向量写入注释与断言。
    #[test]
    fn mode_comparison_max_tie_keeps_first_python_semantics() {
        // Python `max()` 并列取**第一个**；Rust `max_by_key` 会取最后一个，
        // 因此实现用严格 `>` 迭代。`py -3` 差分：
        //   [(1920,1080,60,"first"), (1080,1920,60,"second")]
        //   -> highest_mode.tag == "first"。
        let modes = json!([
            {"width": 1920, "height": 1080, "refresh_hz": 60, "tag": "first"},
            {"width": 1080, "height": 1920, "refresh_hz": 60, "tag": "second"}
        ]);
        let result = mode_comparison(&modes[0], modes.as_array().unwrap()).unwrap();
        assert_eq!(result["highest_mode"]["tag"], json!("first"));
        assert_eq!(result["verdict"], json!("当前达到系统列出的最高模式"));
        // 同像素但更高刷新率时按 key 第二项胜出：`py -3` -> tag="second"。
        let modes = json!([
            {"width": 100, "height": 100, "refresh_hz": 30, "tag": "first"},
            {"width": 100, "height": 100, "refresh_hz": 120, "tag": "second"}
        ]);
        let result = mode_comparison(&modes[0], modes.as_array().unwrap()).unwrap();
        assert_eq!(result["highest_mode"]["tag"], json!("second"));
        assert_eq!(result["verdict"], json!("同分辨率下还有更高刷新率模式可选"));
        // bool 刷新率参与 `max` 的 `or 0`（True -> 1），但 `number(True)` 为 None。
        let modes = json!([
            {"width": 100, "height": 100, "refresh_hz": true, "tag": "first"},
            {"width": 100, "height": 100, "tag": "second"}
        ]);
        let result = mode_comparison(&modes[1], modes.as_array().unwrap()).unwrap();
        assert_eq!(result["highest_mode"]["tag"], json!("first"));
        assert_eq!(
            result["verdict"],
            json!("当前分辨率匹配系统最高模式；刷新率无法比较")
        );
    }

    #[test]
    fn mode_comparison_bool_width_and_no_modes_python_vectors() {
        // `py -3 -c` 确认 `isinstance(True, int) == True`，故 bool 宽高可用：
        //   mode_comparison({"width":True,"height":True}, [{"width":True,"height":True}])
        //   -> offered_mode_count=1, verdict="当前分辨率匹配系统最高模式；刷新率无法比较"
        let modes = json!([{"width": true, "height": true}]);
        let result = mode_comparison(&modes[0], modes.as_array().unwrap()).unwrap();
        assert_eq!(result["offered_mode_count"], json!(1));
        assert_eq!(
            result["highest_mode"],
            json!({"width": true, "height": true})
        );
        assert_eq!(
            result["verdict"],
            json!("当前分辨率匹配系统最高模式；刷新率无法比较")
        );
        // 浮点宽高不是 int：`py -3` -> offered_mode_count=0。
        let modes = json!([{"width": 1920.0, "height": 1080}]);
        let result = mode_comparison(&modes[0], modes.as_array().unwrap()).unwrap();
        assert_eq!(result["offered_mode_count"], json!(0));
        assert_eq!(result["highest_mode"], Value::Null);
        assert_eq!(result["verdict"], json!("系统未提供可比较的模式列表"));
        let result = mode_comparison(&json!({}), &[]).unwrap();
        assert_eq!(result["verdict"], json!("系统未提供可比较的模式列表"));
        // Python `current["width"]` 缺键抛 KeyError；Rust 用 Err 保留。
        let modes = json!([{"width": true, "height": true}]);
        assert_eq!(
            mode_comparison(&json!({}), modes.as_array().unwrap()),
            Err("current 缺少 width/height")
        );
    }

    #[test]
    fn mode_comparison_never_mixes_refresh_across_modes() {
        // `py -3` 对照：同分辨率 current_hz=59/60 -> "当前达到系统列出的最高模式"；
        // 58 -> "同分辨率下还有更高刷新率模式可选"；缺 refresh_hz -> "刷新率无法比较"。
        let modes = vec![mode(3840, 2160, 60)];
        for (hz, verdict) in [
            (59, "当前达到系统列出的最高模式"),
            (58, "同分辨率下还有更高刷新率模式可选"),
        ] {
            let result = mode_comparison(&mode(3840, 2160, hz), &modes).unwrap();
            assert_eq!(result["verdict"], json!(verdict));
        }
        let result = mode_comparison(&json!({"width": 3840, "height": 2160}), &modes).unwrap();
        assert_eq!(
            result["verdict"],
            json!("当前分辨率匹配系统最高模式；刷新率无法比较")
        );
        // Python 的 `1 == 1.0 == True`：`py -3` 对 current width=1920.0 仍判为匹配。
        let result = mode_comparison(
            &json!({"width": 1920.0, "height": 1080, "refresh_hz": 60}),
            &[mode(1920, 1080, 60)],
        )
        .unwrap();
        assert_eq!(result["verdict"], json!("当前达到系统列出的最高模式"));
    }

    #[test]
    fn negotiation_percentage_uses_c_printf_half_even() {
        // `py -3 -c` 差分向量（"%.0f" % (100*live/contract)）：
        //   12.5/100 -> "12%"，13.5/100 -> "14%"，2.5/100 -> "2%"，
        //   99.5/100 -> "100%"，100.5/100 -> "100%"，65/87 -> "75%"，
        //   9.95/20 -> "50%"，1/3 -> "33%"，2.675/10 -> "27%"，0/100 -> "0%"。
        // 这是 C printf 的 half-even（12.5 -> 12 而 13.5 -> 14），
        // 与 Python `round()` 不同；Rust `format!("{:.0}", x)` 逐值一致。
        let cases = [
            (12.5, 100.0, "12%"),
            (13.5, 100.0, "14%"),
            (2.5, 100.0, "2%"),
            (99.5, 100.0, "100%"),
            (100.5, 100.0, "100%"),
            (65.0, 87.0, "75%"),
            (9.95, 20.0, "50%"),
            (1.0, 3.0, "33%"),
            (2.675, 10.0, "27%"),
            (0.0, 100.0, "0%"),
        ];
        for (live, contract, expected) in cases {
            let port = json!({
                "id": "a", "contract": {"watts": contract},
                "input_power": {"watts": live}
            });
            let notes = negotiation(
                &port,
                &json!({"input_port_id": "a", "adapter_rated_w": 140}),
            )["notes"]
                .clone();
            assert_eq!(
                notes[0],
                json!(format!(
                    "实际输入为合同上限的 {expected}；电量、温控与系统负载会影响取电"
                )),
                "{live}/{contract}"
            );
        }
    }

    #[test]
    fn negotiation_separates_sources_and_keeps_evidence_boundary() {
        // `py -3` 逐字对照：输入端口 65W 实测 / 100W 合同 / 60W 线缆声明 /
        // 140W 电源公布值 -> 三条 note 同时出现，host_limit_w 恒为 null。
        let port = json!({
            "id": "a", "contract": {"watts": 100, "voltage_v": 20, "current_a": 5},
            "cable": {"power_ceiling_w": 60}, "input_power": {"watts": 65}
        });
        let power = json!({"input_port_id": "a", "adapter_rated_w": 140});
        assert_eq!(
            negotiation(&port, &power),
            json!({
                "source_reported_w": 140, "cable_declared_w": 60, "contract_w": 100,
                "contract_voltage_v": 20, "contract_current_a": 5, "actual_input_w": 65,
                "host_limit_w": Value::Null,
                "notes": [
                    "实际输入为合同上限的 65%；电量、温控与系统负载会影响取电",
                    "线缆声明上限低于电源公布值，存在容量差异；尚需电脑与电源完整档位确认实际瓶颈",
                    "合同超过已解析的线缆声明上限，请核对 E-Marker 格式、端口归属与协议版本"
                ],
                "source": NEGOTIATION_SOURCE
            })
        );
        // `py -3` 对照：合同 100 与线缆 96 时 `100 > 96*1.05` 为假，只剩两条 note。
        let port = json!({
            "id": "a", "contract": {"watts": 100}, "cable": {"power_ceiling_w": 96},
            "input_power": {"watts": 65}
        });
        let notes = negotiation(&port, &power)["notes"].clone();
        assert_eq!(notes.as_array().unwrap().len(), 2);
        // bool 不是 number()：`contract_w=false` 时百分比/合同比较都不触发。
        let port = json!({
            "id": "a", "contract": {"watts": false}, "cable": {"power_ceiling_w": 60},
            "input_power": {"watts": 65}
        });
        assert_eq!(
            negotiation(&port, &power)["notes"],
            json!(["线缆声明上限低于电源公布值，存在容量差异；尚需电脑与电源完整档位确认实际瓶颈"])
        );
        // 缺证据时保持中文原文，绝不生成线缆瓶颈结论。
        assert_eq!(
            negotiation(&json!({"id": "a"}), &json!({"input_port_id": "a"}))["notes"],
            json!([MISSING_NOTE])
        );
    }

    #[test]
    fn pin_map_unknown_routes_and_python_int_conversion() {
        // `py -3 -c` 差分：`int(config[key])` 对 float 截断、bool、十进制字符串
        // 与 Unicode Nd 数字都接受；None/"x"/3.0/1e1/空串/数组/对象 -> 「未知」。
        let cases = [
            (json!("x"), "未知"),
            (Value::Null, "未知"),
            (json!(true), "USB 3"),
            (json!(false), "空闲"),
            (json!(1.9), "USB 3"),
            (json!("12"), "路由 12"),
            (json!(5), "DP 0"),
            (json!(9), "路由 9"),
            (json!(0), "空闲"),
            (json!("3.0"), "未知"),
            (json!("1e1"), "未知"),
            (json!(""), "未知"),
            (json!([]), "未知"),
            (json!({}), "未知"),
            (json!("١٢"), "路由 12"),
            (json!("１２"), "路由 12"),
        ];
        for (value, expected) in cases {
            let port = json!({"kind": "USB-C", "pin_configuration": {"tx1": value}});
            let result = pin_map(&port).unwrap();
            assert_eq!(result["rows"][0][1]["signal"], json!(expected));
        }
        // sbu 只在 1/2 时是 DP AUX，其它编号保留「路由 N」。
        let port = json!({"kind": "USB-C", "pin_configuration": {"sbu1": 3}});
        let result = pin_map(&port).unwrap();
        assert_eq!(result["rows"][0][7]["signal"], json!("路由 3"));
        // 缺 pin_configuration / 空对象返回 None；不猜 Thunderbolt。
        assert_eq!(pin_map(&json!({"kind": "USB-C"})), None);
        assert_eq!(
            pin_map(&json!({"kind": "USB-C", "pin_configuration": {}})),
            None
        );
        assert_eq!(
            pin_map(&json!({"kind": "USB-C", "pin_configuration": 7})).unwrap()["rows"][0][1]
                ["signal"],
            json!("未知")
        );
    }

    #[test]
    fn resistance_fit_thresholds_and_py_round() {
        // `py -3` 对照：23 点 -> "sampling"；24 点 -> "estimate" 且
        // current_span_a=1.61（round(..., 2)）；milliohms=99.9 由
        // `round(-slope*1000, 1)` 得到，r_squared=round(r2,3)=1.0。
        let samples = slope_samples(40, -0.1, 20.0, true);
        assert_eq!(
            ResistanceTracker::fit(&samples[..23], 20.0),
            json!({"status": "sampling", "milliohms": Value::Null,
                   "sample_count": 23, "r_squared": Value::Null})
        );
        assert_eq!(
            ResistanceTracker::fit(&samples[..24], 20.0),
            json!({"status": "estimate", "milliohms": 99.9, "sample_count": 24,
                   "r_squared": 1.0, "current_span_a": 1.61})
        );
        // slope 边界：`-.8 <= slope < -.001`。`py -3` 差分（用 Python 原式
        // `20 + slope*a` 生成 40 点）：-0.8 -> 800.0 mΩ、-0.001 -> 1.0 mΩ，
        // -0.0009 / -0.801 -> "unreliable"。注意基座用减法 `20 - r*a`（r 为正）
        // 才与 Python 的二元减法浮点累加一致，`20 + (-0.8)*a` 会差 1 ulp。
        for (r, expected) in [(0.8, 800.0), (0.001, 1.0)] {
            let samples = subtractive_samples(40, r, 20.0);
            let result = ResistanceTracker::fit(&samples, 20.0);
            assert_eq!(result["status"], json!("estimate"), "r={r}");
            assert_eq!(result["milliohms"], json!(expected), "r={r}");
        }
        for r in [0.0009, 0.801] {
            let samples = subtractive_samples(40, r, 20.0);
            assert_eq!(
                ResistanceTracker::fit(&samples, 20.0)["status"],
                json!("unreliable"),
                "r={r}"
            );
        }
        // 截距越界（25V 起点、合同 20V）同样判 unreliable。
        let samples = slope_samples(40, -0.1, 25.0, false);
        assert_eq!(
            ResistanceTracker::fit(&samples, 20.0)["status"],
            json!("unreliable")
        );
        // `py -3` 对照：round(99.95,1)=100.0，round(19.95,1)=19.9，
        // round(2.675,2)=2.67；core::py::py_round 已验证一致。
        assert_eq!(py::py_round(99.95, 1), 100.0);
        assert_eq!(py::py_round(19.95, 1), 19.9);
        assert_eq!(py::py_round(2.675, 2), 2.67);
    }

    #[test]
    fn observe_gates_capacity_and_key_reset() {
        // `py -3` 对照：130 次 observe 后 deque(maxlen=120) 只保留 120 个样本。
        let mut tracker = ResistanceTracker::new();
        let snapshot = observe_snapshot();
        for _ in 0..130 {
            tracker.observe(&snapshot);
        }
        assert_eq!(tracker.sample_count(), 120);
        assert_eq!(tracker.samples().capacity() >= 120, true);
        assert_eq!(tracker.observe(&snapshot)["sample_count"], json!(120));
        // `py -3` 对照：round(cv,1) 变化会重置样本（20.04 -> 20.0 不重置，
        // 20.06 -> 20.1 重置）；round 走 core::py::py_round。
        let mut tracker = ResistanceTracker::new();
        let mut snapshot = observe_snapshot();
        tracker.observe(&snapshot);
        snapshot["power"]["contract_voltage_v"] = json!(20.04);
        assert_eq!(tracker.observe(&snapshot)["sample_count"], json!(2));
        snapshot["power"]["contract_voltage_v"] = json!(20.06);
        assert_eq!(tracker.observe(&snapshot)["sample_count"], json!(1));
        // 坏样本被拒绝但保留已有样本；`py -3` status="sample_rejected"。
        snapshot["power"]["input_w"] = json!(100);
        let rejected = tracker.observe(&snapshot);
        assert_eq!(rejected["status"], json!("sample_rejected"));
        assert_eq!(rejected["sample_count"], json!(1));
        // 电流/电压门槛：0.3 与 6.0 合法，0.29/6.1、18.4/20.6 拒绝。
        snapshot["power"]["input_w"] = json!(19.9);
        for (a, v, ok) in [
            (0.3, 19.9, true),
            (6.0, 19.9, true),
            (0.29, 19.9, false),
            (6.1, 19.9, false),
            (1.0, 18.4, false),
            (1.0, 20.6, false),
        ] {
            snapshot["power"]["input_current_a"] = json!(a);
            snapshot["power"]["input_voltage_v"] = json!(v);
            snapshot["power"]["input_w"] = json!(a * v);
            let status = tracker.observe(&snapshot)["status"].clone();
            assert_eq!(status == json!("sample_rejected"), !ok, "{a}/{v}");
        }
    }

    #[test]
    fn enrich_writes_both_fields_in_place() {
        // `py -3` 对照：enrich 原地修改并返回同一对象；port 键序为
        // id, kind, pin_map, negotiation（preserve_order 下可逐字对照）。
        let mut snapshot = json!({
            "power": {"input_port_id": "a", "adapter_rated_w": 140},
            "ports": [{"id": "a", "kind": "MagSafe 3"}, {"id": "b"}]
        });
        enrich(&mut snapshot);
        let keys: Vec<&str> = snapshot["ports"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["id", "kind", "pin_map", "negotiation"]);
        assert_eq!(
            snapshot["ports"][0]["pin_map"]["rows"][0]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        assert_eq!(
            snapshot["ports"][0]["negotiation"]["source_reported_w"],
            json!(140)
        );
        assert_eq!(snapshot["ports"][1]["pin_map"], Value::Null);
        assert_eq!(
            snapshot["ports"][1]["negotiation"]["source_reported_w"],
            Value::Null
        );
        assert_eq!(
            snapshot["ports"][1]["negotiation"]["notes"],
            json!([MISSING_NOTE])
        );
        // `py -3` 逐字对照完整 enrichment 结果。
        assert_eq!(
            serde_json::to_string(&snapshot).unwrap(),
            serde_json::to_string(&json!({
                "power": {"input_port_id": "a", "adapter_rated_w": 140},
                "ports": [
                    {"id": "a", "kind": "MagSafe 3",
                     "pin_map": {"rows": [[{"id":"1","signal":"GND","class":"fixed"},
                                         {"id":"2","signal":"电源","class":"fixed"},
                                         {"id":"3","signal":"识别","class":"fixed"},
                                         {"id":"4","signal":"电源","class":"fixed"},
                                         {"id":"5","signal":"GND","class":"fixed"}]],
                                 "note": MAGSAFE_NOTE},
                     "negotiation": {"source_reported_w": 140, "cable_declared_w": Value::Null,
                                     "contract_w": Value::Null, "contract_voltage_v": Value::Null,
                                     "contract_current_a": Value::Null, "actual_input_w": Value::Null,
                                     "host_limit_w": Value::Null, "notes": [MISSING_NOTE],
                                     "source": NEGOTIATION_SOURCE}},
                    {"id": "b", "pin_map": Value::Null,
                     "negotiation": {"source_reported_w": Value::Null, "cable_declared_w": Value::Null,
                                     "contract_w": Value::Null, "contract_voltage_v": Value::Null,
                                     "contract_current_a": Value::Null, "actual_input_w": Value::Null,
                                     "host_limit_w": Value::Null, "notes": [MISSING_NOTE],
                                     "source": NEGOTIATION_SOURCE}}
                ]
            }))
            .unwrap()
        );
    }

    /// 与 `tests/test_features.py` 的样本构造逐字对应：
    /// `[(a, 20-.1*a+(.001 if i%2 else -.001)) for i,a in enumerate([.4+i*.07 ...])]`。
    /// `slope` / `base` 参数用于门槛边界差分；`jitter=true` 时按 Python 的
    /// `20-.1*a±.001` 交替抖动，保证浮点中间值与 Python 完全一致。
    fn slope_samples(count: usize, slope: f64, base: f64, jitter: bool) -> Vec<(f64, f64)> {
        (0..count)
            .map(|i| {
                let a = 0.4 + i as f64 * 0.07;
                let v = base
                    + slope * a
                    + if jitter {
                        if i % 2 == 0 {
                            -0.001
                        } else {
                            0.001
                        }
                    } else {
                        0.0
                    };
                (a, v)
            })
            .collect()
    }

    /// Python `[(a, 20 - r*a) for a in [.4+i*.07 for i in range(n)]]`：
    /// 用二元减法生成电压，保证门槛边缘的浮点表示与 `py -3` 一致。
    fn subtractive_samples(count: usize, r: f64, base: f64) -> Vec<(f64, f64)> {
        (0..count)
            .map(|i| {
                let a = 0.4 + i as f64 * 0.07;
                (a, base - r * a)
            })
            .collect()
    }

    fn observe_snapshot() -> Value {
        json!({
            "power": {"external_connected": true, "input_port_id": "a", "contract_voltage_v": 20,
                      "input_voltage_v": 19.9, "input_current_a": 1, "input_w": 19.9,
                      "independent_input_power": true},
            "ports": [{"id": "a", "plug_count": 1}]
        })
    }
}
