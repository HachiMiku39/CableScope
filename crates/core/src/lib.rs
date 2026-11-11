//! CableScope 纯逻辑移植：数值清洗、E-Marker VDO 解码与诊断提示。
//!
//! 快照用 `serde_json::Value`（`preserve_order` 保持键序）表示；typed 结构留给
//! 后续波次。所有用户可见中文文案与 `cablescope/core.py` 逐字一致。

pub mod py;

use serde_json::{json, Map, Number, Value};

const IDENTITY_SOURCE: &str = "IOPortTransportComponentCCUSBPDSOPp.Metadata.VDOs";
const IDENTITY_NOTE: &str = "E-Marker 是芯片声明，不等于实测速率、线阻或认证结论。";

/// Python `number(value)`：拒绝 bool、字符串与非有限数值。
pub fn number(value: &Value) -> Option<Value> {
    py::number(value)
}

/// Python `scaled(value, divisor)`：`round(number(value) / divisor, 3)`。
///
/// Python 的 `round` 是银行家舍入且受浮点二进制表示影响，见 `py::py_round`。
pub fn scaled(value: &Value, divisor: f64) -> Option<Value> {
    let n = py::number_f64(value)?;
    if !divisor.is_finite() || divisor == 0.0 {
        return None;
    }
    let rounded = py::py_round(n / divisor, 3);
    Some(Value::Number(
        Number::from_f64(rounded).unwrap_or_else(|| Number::from(0)),
    ))
}

/// Python `signed64(value)`：≥ 2^63 时减 2^64；整数路径保持精确。
pub fn signed64(value: &Value) -> Option<Value> {
    py::signed64(value)
}

/// UTC 当前时间，格式 `%Y-%m-%dT%H:%M:%S+00:00`。
///
/// 对应 Python `datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")`。
pub use py::now;

fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    let arr = v.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let n = item.as_u64()?;
        if n > 255 {
            return None;
        }
        out.push(n as u8);
    }
    Some(out)
}

/// 解码 PD3 线缆 E-Marker 的已知字段；含糊值显式保留为"未知/保留编码"。
///
/// `Metadata.VDOs` 的每个 blob 用 JSON 字节数组表示（对应 Python `bytes`）：
/// 只有 4 字节的 blob 会进入 `vdo_hex`，但 fingerprint 会拼进所有 blob 的原始字节。
pub fn decode_identity(props: &Value) -> Value {
    let empty = Value::Object(Map::new());
    let meta = props.get("Metadata").unwrap_or(&empty);
    let blobs: Vec<Value> = meta
        .get("VDOs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let words: Vec<u32> = blobs
        .iter()
        .filter_map(bytes_of)
        .filter(|b| b.len() == 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();

    let fallback = |key: &str, default: Value| -> Value {
        props
            .get(key)
            .or_else(|| meta.get(key))
            .cloned()
            .unwrap_or(default)
    };

    let mut result = Map::new();
    result.insert("vendor_id".into(), fallback("Vendor ID", Value::Null));
    result.insert("product_id".into(), fallback("Product ID", Value::Null));
    result.insert(
        "type".into(),
        fallback("Product Type Description", json!("未知")),
    );
    result.insert(
        "vdo_hex".into(),
        Value::Array(words.iter().map(|w| json!(format!("{w:08x}"))).collect()),
    );
    result.insert("current_a".into(), Value::Null);
    result.insert("power_ceiling_w".into(), Value::Null);
    result.insert("speed".into(), json!("未解析"));
    result.insert("source".into(), json!(IDENTITY_SOURCE));
    result.insert("note".into(), json!(IDENTITY_NOTE));

    if words.len() < 4 || props.get("Specification Revision").and_then(py::number_f64) != Some(3.0)
    {
        return Value::Object(result);
    }
    let kind = (words[0] >> 27) & 7;
    if kind != 3 && kind != 4 {
        return Value::Object(result);
    }
    let w = words[3];
    let speed = match w & 7 {
        0 => "USB 2.0 · 480 Mbps",
        1 => "USB · 5 Gbps",
        2 => "USB · 10 Gbps",
        3 => "20 / 40 Gbps（PD 版本有歧义）",
        4 => "USB4 · 80 Gbps",
        _ => "保留编码，未知",
    };
    result.insert("speed".into(), json!(speed));
    let current_a = match (w >> 5) & 3 {
        1 => Some(3),
        2 => Some(5),
        _ => None,
    };
    result.insert(
        "current_a".into(),
        current_a.map(|v| json!(v)).unwrap_or(Value::Null),
    );
    // PD3 passive cable: 50V insulation is not 50V delivered (EPR tops at 48V).
    let voltage_code = (w >> 9) & 3;
    if kind == 3 {
        if let Some(a) = current_a {
            let power = if voltage_code == 0 {
                Some(a * 20)
            } else if voltage_code == 3 && w & (1 << 17) != 0 {
                Some(a * 48)
            } else {
                None
            };
            if let Some(p) = power {
                result.insert("power_ceiling_w".into(), json!(p));
            }
        }
    }
    // Python: hexdigest()[:16]，即对全部 bytes blob 的拼接取摘要前 16 个
    // 十六进制字符；非 4 字节 blob 也参与，但不会进入 vdo_hex。
    let all_blobs: Vec<Vec<u8>> = blobs.iter().filter_map(bytes_of).collect();
    result.insert("fingerprint".into(), json!(py::fingerprint(&all_blobs)));
    Value::Object(result)
}

/// 基于证据的只读诊断提示；证据不足时保持"未知"，不推断"线缆是瓶颈"。
pub fn diagnostics(snapshot: &Value) -> Value {
    let mut notes: Vec<Value> = Vec::new();
    let empty_obj = Value::Object(Map::new());
    let power = snapshot.get("power").unwrap_or(&empty_obj);
    if power.get("external_connected") == Some(&Value::Bool(true))
        && power.get("charging") == Some(&Value::Bool(false))
    {
        notes.push(json!({
            "level": "info", "title": "已接电源，电池未充电",
            "text": "可能是电量、温度或充电策略所致，单凭这个状态不能判断线缆故障。"
        }));
    }
    let empty_ports: Vec<Value> = Vec::new();
    let ports = snapshot
        .get("ports")
        .and_then(Value::as_array)
        .unwrap_or(&empty_ports);
    for port in ports {
        let name = port.get("name").and_then(Value::as_str).unwrap_or("");
        if port.get("liquid_detected") == Some(&Value::Bool(true)) {
            notes.push(json!({
                "level": "warn", "title": format!("{name} 报告液体检测"),
                "text": "这是控制器报告状态，请检查接口。"
            }));
        }
        let zero = Value::from(0);
        let overcurrent = port.get("overcurrent_count").unwrap_or(&zero);
        // Python `if port.get("overcurrent_count", 0)` 的真值判断：非零数值、
        // 非空字符串/数组/对象为真；bool 也是 int 子类，但 false/0 为假。
        if py_truthy(overcurrent) {
            notes.push(json!({
                "level": "warn", "title": format!("{name} 有过流累计计数"),
                "text": "累计计数不一定发生在当前线缆上，请结合插拔记录核对。"
            }));
        }
    }
    notes.push(json!({
        "level": "info", "title": "瓶颈结论需要完整链路证据",
        "text": "当前 USB 链路速度、线缆声明和设备上限是不同的数据。缺少任何一项时，不判定线缆限制速度。"
    }));
    Value::Array(notes)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn identity(cable: u32, revision: i64, kind: u32) -> Value {
        // 对应 tests/test_core.py 的 identity()：四个小端 u32 VDO blob。
        let words = [(kind << 27) | 0x1234, 0, 0x5678_0001, cable];
        json!({
            "Specification Revision": revision,
            "Metadata": {"VDOs": words.map(|w| w.to_le_bytes().to_vec())}
        })
    }

    // tests/test_core.py::PDTests 的全部 core 相关用例。
    #[test]
    fn test_little_endian_5a_100w() {
        let result = decode_identity(&identity((2 << 5) | 2, 3, 3));
        assert_eq!(result["current_a"], json!(5));
        assert_eq!(result["power_ceiling_w"], json!(100));
        assert!(
            result["speed"].as_str().unwrap().contains("10 Gbps"),
            "{}",
            result["speed"]
        );
        // `py -3` 差分对照：vdo_hex=["18001234","00000000","56780001","00000042"]，
        // fingerprint="6894529626a02878"，speed="USB · 10 Gbps"。
        assert_eq!(
            result["vdo_hex"],
            json!(["18001234", "00000000", "56780001", "00000042"])
        );
        assert_eq!(result["fingerprint"], json!("6894529626a02878"));
        assert_eq!(result["speed"], json!("USB · 10 Gbps"));
    }

    #[test]
    fn test_epr_is_240_not_250() {
        let result = decode_identity(&identity((2 << 5) | (3 << 9) | (1 << 17) | 4, 3, 3));
        assert_eq!(result["power_ceiling_w"], json!(240));
        // `py -3` 对照：power_ceiling_w=240，fingerprint="83785dd59a49d894"，
        // speed="USB4 · 80 Gbps"。
        assert_eq!(result["fingerprint"], json!("83785dd59a49d894"));
        assert_eq!(result["speed"], json!("USB4 · 80 Gbps"));
    }

    #[test]
    fn test_ambiguous_speed_stays_ambiguous() {
        let result = decode_identity(&identity(3, 3, 3));
        assert!(result["speed"].as_str().unwrap().contains("歧义"));
        // `py -3` 对照：speed="20 / 40 Gbps（PD 版本有歧义）"，
        // fingerprint="2f7aeed0a3386813"。
        assert_eq!(result["speed"], json!("20 / 40 Gbps（PD 版本有歧义）"));
        assert_eq!(result["fingerprint"], json!("2f7aeed0a3386813"));
    }

    #[test]
    fn test_unknown_current_not_assumed_3a() {
        for bits in [0, 3] {
            let result = decode_identity(&identity(bits << 5, 3, 3));
            assert_eq!(result["current_a"], Value::Null);
            assert_eq!(result["power_ceiling_w"], Value::Null);
        }
        // `py -3` 对照：bits=0 时 fingerprint="4ff31feafa02fe16"，
        // bits=3 时 fingerprint="db582316cbb8485f"。
        assert_eq!(
            decode_identity(&identity(0, 3, 3))["fingerprint"],
            json!("4ff31feafa02fe16")
        );
        assert_eq!(
            decode_identity(&identity(3 << 5, 3, 3))["fingerprint"],
            json!("db582316cbb8485f")
        );
    }

    #[test]
    fn test_pd2_layout_is_not_decoded_as_pd3() {
        let result = decode_identity(&identity(0xFFFF_FFFF, 2, 3));
        assert_eq!(result["current_a"], Value::Null);
        assert_eq!(result["speed"], json!("未解析"));
        assert!(result.get("fingerprint").is_none());
    }

    #[test]
    fn test_active_cable_power_not_guessed() {
        let result = decode_identity(&identity((2 << 5) | (3 << 9), 3, 4));
        assert_eq!(result["power_ceiling_w"], Value::Null);
        assert_eq!(result["current_a"], json!(5));
    }

    #[test]
    fn test_deprecated_voltage_not_claimed_240w() {
        let result = decode_identity(&identity((2 << 5) | (1 << 9), 3, 3));
        assert_eq!(result["power_ceiling_w"], Value::Null);
        // `py -3` 对照：fingerprint="8c9e5bbea187b8f2"。
        assert_eq!(result["fingerprint"], json!("8c9e5bbea187b8f2"));
    }

    #[test]
    fn test_boolean_nan_are_not_measurements() {
        assert_eq!(number(&json!(true)), None);
        assert_eq!(number(&Value::from(f64::NAN)), None);
        // Python 原断言：signed64((1 << 64) - 1000) == -1000。2^64 超出
        // serde_json 的 u64 范围，故用 u64 可表示的等价大整数复刻。
        assert_eq!(
            signed64(&json!(18_446_744_073_709_551_615_u64 - 999)),
            Some(json!(-1000))
        );
    }

    #[test]
    fn test_no_cable_bottleneck_invented() {
        let notes = diagnostics(&json!({
            "power": {"external_connected": true, "charging": false}, "ports": []
        }));
        assert!(notes[0]["text"].as_str().unwrap().contains("不能判断"));
        // `py -3` 逐字对照。
        assert_eq!(
            notes[0],
            json!({
                "level": "info", "title": "已接电源，电池未充电",
                "text": "可能是电量、温度或充电策略所致，单凭这个状态不能判断线缆故障。"
            })
        );
    }

    // 逐函数移植的补充用例：number/scaled/signed64/now/diagnostics/decode_identity。
    #[test]
    fn scaled_uses_python_round_and_rejects_bad_input() {
        // `py -3` 对照：scaled(1234,1000)=1.234，scaled(1234.5,1000)=1.234
        // （银行家舍入），scaled(1e20,1000)=1e17，scaled(-1234.5,1000)=-1.234；
        // scaled(inf/True/"5"/None,1000) 均为 None。
        assert_eq!(scaled(&json!(1234), 1000.0), Some(json!(1.234)));
        assert_eq!(scaled(&json!(1234.5), 1000.0), Some(json!(1.234)));
        assert_eq!(scaled(&json!(1e20), 1000.0), Some(json!(1e17)));
        assert_eq!(scaled(&json!(-1234.5), 1000.0), Some(json!(-1.234)));
        assert_eq!(scaled(&Value::from(f64::INFINITY), 1000.0), None);
        assert_eq!(scaled(&json!(true), 1000.0), None);
        assert_eq!(scaled(&json!("5"), 1000.0), None);
        assert_eq!(scaled(&Value::Null, 1000.0), None);
        assert_eq!(scaled(&json!(1), 0.0), None);
        assert_eq!(scaled(&json!(1), f64::INFINITY), None);
    }

    #[test]
    fn now_matches_python_isoformat_seconds() {
        let value = now();
        // Python 对照：len==25，末 6 字符为 "+00:00"，含 "T"，秒位无小数。
        assert_eq!(value.len(), 25, "{value}");
        assert!(value.ends_with("+00:00"), "{value}");
        assert_eq!(value.as_bytes()[4], b'-');
        assert_eq!(value.as_bytes()[7], b'-');
        assert_eq!(value.as_bytes()[10], b'T');
        assert_eq!(value.as_bytes()[13], b':');
        assert_eq!(value.as_bytes()[16], b':');
        assert!(value[..19]
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == 'T' || c == ':'));
    }

    #[test]
    fn decode_identity_preserves_metadata_and_reserved_encodings() {
        // `py -3` 对照：props 无 Vendor ID 时回退到 Metadata["Vendor ID"]=1，
        // type 回退到 "X"，vdo_hex=[]，speed="未解析"，且没有 fingerprint。
        let props =
            json!({"Metadata": {"VDOs": [], "Vendor ID": 1, "Product Type Description": "X"}});
        let result = decode_identity(&props);
        assert_eq!(result["vendor_id"], json!(1));
        assert_eq!(result["product_id"], Value::Null);
        assert_eq!(result["type"], json!("X"));
        assert_eq!(result["vdo_hex"], json!([]));
        assert_eq!(result["speed"], json!("未解析"));
        assert_eq!(result["source"], json!(IDENTITY_SOURCE));
        assert_eq!(result["note"], json!(IDENTITY_NOTE));
        assert!(result.get("fingerprint").is_none());
        // Python dict.get 只在键缺失时回退；显式 null 会原样保留。
        let result = decode_identity(&json!({
            "Vendor ID": Value::Null, "Metadata": {"Vendor ID": 7}
        }));
        assert_eq!(result["vendor_id"], Value::Null);
        let result = decode_identity(&identity(7, 3, 3));
        assert_eq!(result["speed"], json!("保留编码，未知"));
        let result = decode_identity(&identity(0, 3, 5));
        assert_eq!(result["speed"], json!("未解析"));
        assert!(result.get("fingerprint").is_none());
        // `py -3` 对照：props["Specification Revision"]=3.0（float）也会解码，
        // 因为 Python 的 `3.0 == 3` 为真。
        let mut props = identity(0, 3, 3);
        props["Specification Revision"] = json!(3.0);
        assert_eq!(
            decode_identity(&props)["speed"],
            json!("USB 2.0 · 480 Mbps")
        );
    }

    #[test]
    fn fingerprint_hashes_all_byte_blobs_not_only_four_byte_words() {
        // Python 把 blobs 中所有 bytes 拼进 sha256；非 4 字节 blob 不进 vdo_hex。
        let mut props = identity(0, 3, 3);
        props["Metadata"]["VDOs"]
            .as_array_mut()
            .unwrap()
            .push(json!([1, 2, 3]));
        let result = decode_identity(&props);
        assert_eq!(result["vdo_hex"].as_array().unwrap().len(), 4);
        let all_blobs: Vec<Vec<u8>> = props["Metadata"]["VDOs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(bytes_of)
            .collect();
        assert_eq!(result["fingerprint"], json!(py::fingerprint(&all_blobs)));
        // `py -3` 差分对照：hashlib.sha256(b"".join(blobs)).hexdigest()[:16]
        // == "97b124c18e20a41a"。
        assert_eq!(result["fingerprint"], json!("97b124c18e20a41a"));
    }

    #[test]
    fn diagnostics_reports_ports_without_inventing_a_bottleneck() {
        let notes = diagnostics(&json!({
            "power": {"external_connected": true, "charging": false},
            "ports": [
                {"name": "A", "liquid_detected": true, "overcurrent_count": 0},
                {"name": "B", "overcurrent_count": 2}
            ]
        }));
        // `py -3` 逐字对照（4 条）。
        assert_eq!(
            notes,
            json!([
                {"level": "info", "title": "已接电源，电池未充电",
                 "text": "可能是电量、温度或充电策略所致，单凭这个状态不能判断线缆故障。"},
                {"level": "warn", "title": "A 报告液体检测",
                 "text": "这是控制器报告状态，请检查接口。"},
                {"level": "warn", "title": "B 有过流累计计数",
                 "text": "累计计数不一定发生在当前线缆上，请结合插拔记录核对。"},
                {"level": "info", "title": "瓶颈结论需要完整链路证据",
                 "text": "当前 USB 链路速度、线缆声明和设备上限是不同的数据。缺少任何一项时，不判定线缆限制速度。"}
            ])
        );
    }
}
