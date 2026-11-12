//! macOS 后端的平台无关映射层。
//!
//! 对应 `cablescope/backends/macos.py`：plist 字节 → `serde_json::Value`、
//! IORegistry 合成属性字典 → 快照（power/ports/devices/displays/display_links/fabric），
//! 以及 SMC 通道/合同匹配、DP 唯一父端口、输入功率分配、显示模式比较、
//! Thunderbolt 树展开等规则。所有函数都可在 Windows 上编译并用合成数据测试；
//! IOKit/CoreGraphics 调用与子进程放在 `ffi` 模块，仅在 macOS 编译。

use std::collections::HashMap;
use std::io::Cursor;

use cablescope_analysis::{mode_comparison, negotiation, pin_map};
use cablescope_core::{decode_identity, number, py, scaled, signed64};
use cablescope_smc::normal_uuid;
use plist::Value as PlistValue;
use serde_json::{json, Map, Value};

const POWER_SOURCE: &str =
    "AppleSmartBattery.PowerTelemetryData.SystemPowerIn（系统遥测，mW）；不代表墙端功率或单端口功率";
const PORT_SOURCE_SUFFIX: &str = " 注册表属性";
const DP_SOURCE: &str = "IOPortTransportStateDisplayPort；DSC 状态未解析";
const FABRIC_SOURCE: &str = "system_profiler SPThunderboltDataType（15 秒缓存）";
const INPUT_SCOPE: &str = "系统输入轨；仅一个有效输入合同对应此端口，不是独立端口电流传感器";
const BATTERY_SOURCE: &str = "AppleSMC PPBR 电池轨放电功率";

const PORT_CLASSES: [&str; 6] = [
    "AppleHPMInterfaceType10",
    "AppleHPMInterfaceType11",
    "AppleHPMInterfaceType12",
    "AppleHPMInterfaceType18",
    "AppleTCControllerType10",
    "AppleTCControllerType11",
];

const LIMITATIONS: [&str; 3] = [
    "SMC 仅在系统允许时显示；端口输出功率与输入合同分开展示，未测柔软度或实际吞吐。",
    "无 E-Marker 数据仅表示未读取到，不能据此判断为低速线或假线。",
    "显示模式不等于面板最大能力；首版不判定 DSC 或完整 DP 带宽。",
];

/// `CFPropertyListCreateData` 的 format 参数 200 = `kCFPropertyListBinaryFormat_v1_0`。
pub const PLIST_BINARY_FORMAT_V1_0: u64 = 200;

/// 把二进制 plist（CFPropertyListCreateData 的输出）解析为 JSON 值。
///
/// 逐类型对应 Python `plistlib.loads`：Data 转成字节数组（E-Marker VDO blobs
/// 需要保留字节），String/Real/Integer/Boolean/Array/Dictionary 直接对应；
/// Date 用 XML plist 的 RFC3339 文本表示，Uid 用十进制字符串表示（两者在
/// IORegistry 属性中不参与后续判定，保持可见而不丢失）。
pub fn plist_bytes_to_json(bytes: &[u8]) -> Result<Value, String> {
    let value = PlistValue::from_reader(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    Ok(plist_to_json(value))
}

pub fn plist_to_json(value: PlistValue) -> Value {
    match value {
        PlistValue::Array(items) => Value::Array(items.into_iter().map(plist_to_json).collect()),
        PlistValue::Dictionary(items) => {
            let mut map = Map::new();
            for (key, item) in items {
                map.insert(key, plist_to_json(item));
            }
            Value::Object(map)
        }
        PlistValue::Boolean(v) => Value::Bool(v),
        PlistValue::Data(bytes) => Value::Array(bytes.into_iter().map(|b| json!(b)).collect()),
        PlistValue::Date(date) => json!(date.to_xml_format()),
        PlistValue::Real(v) => serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        PlistValue::Integer(v) => v
            .as_signed()
            .map(|n| json!(n))
            .or_else(|| v.as_unsigned().map(|n| json!(n)))
            .unwrap_or_else(|| json!(v.to_string())),
        PlistValue::String(v) => json!(v),
        PlistValue::Uid(uid) => json!(uid.get().to_string()),
        other => json!(format!("{other:?}")),
    }
}

/// 从 HPM 父节点属性字典取 `UUID` 并按 `normal_uuid` 规范化。
/// 对应 `Registry.controller_uuid()` 最终返回的 `normal_uuid(p.get("UUID"))`。
pub fn uuid_from_properties(properties: &Value) -> Option<String> {
    match properties.get("UUID") {
        Some(Value::String(text)) => normal_uuid(text),
        _ => None,
    }
}

/// 父节点链判定：`IOObjectGetClass` 结果是否以 `AppleHPMDevice` 开头。
/// 对应 `if not name.value.startswith(b"AppleHPMDevice"): continue`。
pub fn is_hpm_device_class(class_name: &[u8]) -> bool {
    class_name.starts_with(b"AppleHPMDevice")
}

/// `services()` 读出的 IORegistry 属性字典：包含 FFI 层额外附加的内部字段。
#[derive(Debug, Clone, Default)]
pub struct RegistryData {
    pub batteries: Vec<Value>,
    pub batteries_present: bool,
    pub identities: Vec<Value>,
    pub ports: Vec<Value>,
    pub displayport: Vec<Value>,
    pub usb_devices: Vec<Value>,
    pub port_class_results: Vec<(String, Vec<Value>)>,
    pub errors: Vec<String>,
}

/// CoreGraphics 查询的平台无关抽象。FFI 层按显示器 ID 填充当前模式与全部模式；
/// 纯逻辑层只负责名称、刷新率舍入、去重和 `mode_comparison`。
#[derive(Debug, Clone)]
pub struct DisplayQuery {
    pub identifier: u32,
    pub builtin: bool,
    pub current: Value,
    pub modes: Vec<Value>,
}

/// Python `round(hz, 2) if hz > 1 else None`。
fn refresh_hz(rate: f64) -> Value {
    if rate > 1.0 {
        Value::Number(
            serde_json::Number::from_f64(py::py_round(rate, 2))
                .unwrap_or_else(|| serde_json::Number::from(0)),
        )
    } else {
        Value::Null
    }
}

/// 由合成/真实的 CoreGraphics 查询结果生成 `displays[]`。
/// 不跨模式拼分辨率与刷新率：比较完全交给 `analysis::mode_comparison`。
pub fn display_modes_from_queries(queries: &[DisplayQuery]) -> Vec<Value> {
    let mut result = Vec::new();
    for query in queries {
        let name = format!(
            "{} #{}",
            if query.builtin {
                "内置显示器"
            } else {
                "外接显示器"
            },
            query.identifier
        );
        let mut display = Map::new();
        display.insert("name".into(), json!(name));
        for key in ["width", "height"] {
            display.insert(
                key.into(),
                query.current.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        let rate = number(query.current.get("refresh_hz").unwrap_or(&Value::Null))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        display.insert("refresh_hz".into(), refresh_hz(rate));
        display.insert(
            "source".into(),
            json!("CoreGraphics 当前像素模式与系统提供的模式列表"),
        );
        let mut modes: Vec<Value> = Vec::new();
        for mode in query.modes.iter().take(4096) {
            let entry = json!({
                "width": mode.get("width").cloned().unwrap_or(Value::Null),
                "height": mode.get("height").cloned().unwrap_or(Value::Null),
                "refresh_hz": refresh_hz(
                    number(mode.get("refresh_hz").unwrap_or(&Value::Null))
                        .and_then(|v| v.as_f64()).unwrap_or(0.0)
                )
            });
            if !modes.contains(&entry) {
                modes.push(entry);
            }
        }
        let current = Value::Object(display.clone());
        let comparison = mode_comparison(&current, &modes).unwrap_or_else(|_| {
            json!({
                "highest_mode": Value::Null,
                "offered_mode_count": 0,
                "verdict": "当前像素模式缺少宽度或高度"
            })
        });
        if let Value::Object(fields) = comparison {
            for (key, value) in fields {
                display.insert(key, value);
            }
        }
        result.push(Value::Object(display));
    }
    result
}

/// `IOUSBHostDevice.Device Speed` → 中文速率文案；bcdUSB 是描述符版本，不映射。
pub fn usb_devices_from_properties(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .map(|p| {
            let speed = match p.get("Device Speed") {
                Some(Value::Number(n)) => match n.as_i64() {
                    Some(0) => "1.5 Mbps",
                    Some(1) => "12 Mbps",
                    Some(2) => "480 Mbps",
                    Some(3) => "5 Gbps",
                    Some(4) => "SuperSpeedPlus（速率未知）",
                    _ => "未知",
                },
                _ => "未知",
            };
            json!({
                "name": p.get("USB Product Name").cloned().unwrap_or(json!("USB 设备")),
                "vendor": p.get("USB Vendor Name").cloned().unwrap_or(json!("")),
                "vendor_id": p.get("idVendor").cloned().unwrap_or(Value::Null),
                "product_id": p.get("idProduct").cloned().unwrap_or(Value::Null),
                "speed": speed,
                "source": "IOUSBHostDevice.Device Speed；未映射到物理 USB-C 口"
            })
        })
        .collect()
}

/// 递归展开 `system_profiler -json SPThunderboltDataType` 的 `_items` 树。
/// 只有非根节点产生记录；`receptacle_upstream*` 子字典里第一个非空
/// `current_speed_key` 作为链路速率，缺失保持「未知」。
pub fn fabric_from_profiler(nodes: &[Value]) -> Vec<Value> {
    fn walk(nodes: &[Value], parent: &str, out: &mut Vec<Value>) {
        for node in nodes {
            let name = node
                .get("_name")
                .and_then(Value::as_str)
                .unwrap_or("Thunderbolt");
            let path = format!("{parent}/{name}");
            if !parent.is_empty() {
                let mut speed = Value::Null;
                if let Some(fields) = node.as_object() {
                    for (key, value) in fields {
                        if !key.starts_with("receptacle_upstream") {
                            continue;
                        }
                        if let Some(candidate) = value
                            .as_object()
                            .and_then(|m| m.get("current_speed_key"))
                            .filter(|v| py_truthy(v))
                        {
                            speed = candidate.clone();
                            break;
                        }
                    }
                }
                out.push(json!({
                    "name": node.get("device_name_key")
                        .or_else(|| node.get("_name"))
                        .cloned().unwrap_or(Value::Null),
                    "path": path,
                    "mode": node.get("mode_key").cloned().unwrap_or(json!("未知")),
                    "speed": if speed.is_null() { json!("未知") } else { speed },
                    "source": FABRIC_SOURCE
                }));
            }
            walk(
                node.get("_items")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                &path,
                out,
            );
        }
    }
    let mut result = Vec::new();
    walk(nodes, "", &mut result);
    result
}

pub fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.get(key)
}

/// Python `dict.get(key) or {}`：假值回退到空对象。
fn object_or_empty<'a>(value: Option<&'a Value>, empty: &'a Value) -> &'a Value {
    match value {
        Some(v) if py_truthy(v) => v,
        _ => empty,
    }
}

/// 构造 `power` 字典（Python `Backend.sample()` 的电池/遥测部分）。
/// `batteries_present` 对应 Python 的 `not batteries`：是否至少读到一个
/// `AppleSmartBattery` 服务；单独用空属性字典不能表达这一点。
pub fn power_from_battery(battery: &Value, telemetry: &Value) -> Value {
    power_from_battery_with_presence(battery, telemetry, py_truthy(battery))
}

pub fn power_from_battery_with_presence(
    battery: &Value,
    telemetry: &Value,
    batteries_present: bool,
) -> Value {
    let empty = Value::Object(Map::new());
    let a = object_or_empty(get(battery, "AdapterDetails"), &empty);
    let t = object_or_empty(get(battery, "PowerTelemetryData"), &empty);
    let ext = get(battery, "ExternalConnected")
        .cloned()
        .unwrap_or(Value::Null);
    let ext_true = ext == Value::Bool(true);
    let input_mw = get(t, "SystemPowerIn").and_then(number);
    let input_w = if ext_true {
        input_mw
            .as_ref()
            .and_then(|v| v.as_f64())
            .filter(|n| (0.0..1_000_000.0).contains(n))
            .and_then(|_| scaled(&input_mw.clone().unwrap(), 1000.0))
    } else {
        None
    };
    let current = get(battery, "Amperage")
        .and_then(signed64)
        .and_then(|v| scaled(&v, 1000.0));
    let volts = get(battery, "Voltage").and_then(|v| scaled(v, 1000.0));
    let battery_percent = match (
        get(battery, "MaxCapacity").and_then(number),
        get(battery, "CurrentCapacity").and_then(number),
    ) {
        (Some(max), Some(now)) if max.as_f64().unwrap_or(0.0) > 0.0 => Some(json!(py::py_round(
            now.as_f64().unwrap_or(0.0) / max.as_f64().unwrap_or(0.0) * 100.0,
            1
        ))),
        _ => None,
    };
    let battery_net_w = match (&current, &volts) {
        (Some(c), Some(v)) => Some(json!(py::py_round(
            c.as_f64().unwrap_or(0.0) * v.as_f64().unwrap_or(0.0),
            3
        ))),
        _ => None,
    };
    let profiles = if ext_true {
        get(a, "UsbHvcMenu")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|p| {
                json!({
                    "voltage_v": get(p, "MaxVoltage").and_then(|v| scaled(v, 1000.0)),
                    "current_a": get(p, "MaxCurrent").and_then(|v| scaled(v, 1000.0))
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let value_if_external = |key: &str| -> Value {
        if ext_true {
            get(a, key).cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        }
    };
    let mut power = json!({
        "external_connected": ext,
        "charging": get(battery, "IsCharging").cloned().unwrap_or(Value::Null),
        "battery_percent": battery_percent,
        "input_w": input_w,
        "adapter_rated_w": value_if_external("Watts"),
        "input_voltage_v": if ext_true {
            get(t, "SystemVoltageIn").and_then(|v| scaled(v, 1000.0))
        } else {
            None
        },
        "input_current_a": if ext_true {
            get(t, "SystemCurrentIn").and_then(|v| scaled(v, 1000.0))
        } else {
            None
        },
        "battery_net_w": battery_net_w,
        "adapter_name": value_if_external("Name"),
        "profiles": Value::Array(profiles),
        "source": POWER_SOURCE
    });
    let system = get(telemetry, "system");
    let independent = system
        .and_then(|s| get(s, "watts"))
        .filter(|w| !w.is_null());
    if let (Some(system), Some(_)) = (system, independent) {
        if ext_true || !batteries_present {
            if let Value::Object(fields) = &mut power {
                fields.insert(
                    "input_w".into(),
                    get(system, "watts").cloned().unwrap_or(Value::Null),
                );
                fields.insert(
                    "input_voltage_v".into(),
                    get(system, "voltage_v").cloned().unwrap_or(Value::Null),
                );
                fields.insert(
                    "input_current_a".into(),
                    get(system, "current_a").cloned().unwrap_or(Value::Null),
                );
                fields.insert(
                    "source".into(),
                    json!(format!(
                        "{}（DC 输入，非墙端交流功率）",
                        get(system, "source").and_then(Value::as_str).unwrap_or("")
                    )),
                );
            }
        }
    }
    if let Value::Object(fields) = &mut power {
        fields.insert(
            "smc_status".into(),
            get(telemetry, "status").cloned().unwrap_or(Value::Null),
        );
        fields.insert(
            "independent_input_power".into(),
            json!(system
                .and_then(|s| get(s, "independent_power"))
                .map(py_truthy)
                .unwrap_or(false)),
        );
        if ext == Value::Bool(false) {
            if let Some(discharge) = get(telemetry, "battery_discharge_w").filter(|v| !v.is_null())
            {
                fields.insert(
                    "battery_net_w".into(),
                    json!(-discharge.as_f64().unwrap_or(0.0)),
                );
                fields.insert("battery_source".into(), json!(BATTERY_SOURCE));
            }
        }
    }
    power
}

/// 按 `(ParentBuiltInPortTypeDescription, ParentBuiltInPortNumber)` 为 E-Marker
/// 身份分组；与 Python `identities.setdefault(key, []).append(p)` 一致。
pub fn identity_lookup(rows: &[Value]) -> HashMap<String, Vec<Value>> {
    let mut result: HashMap<String, Vec<Value>> = HashMap::new();
    for row in rows {
        let key = json!([
            get(row, "ParentBuiltInPortTypeDescription")
                .cloned()
                .unwrap_or(Value::Null),
            get(row, "ParentBuiltInPortNumber")
                .cloned()
                .unwrap_or(Value::Null)
        ]);
        let serialized = serde_json::to_string(&key).unwrap_or_default();
        result.entry(serialized).or_default().push(row.clone());
    }
    result
}

fn lookup_key(kind: &Value, number: &Value) -> String {
    serde_json::to_string(&json!([kind, number])).unwrap_or_default()
}

fn uuid_key(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or("\u{0}__missing__")
        .to_string()
}

fn strip_private(row: &Value) -> Value {
    let mut clean = Map::new();
    if let Value::Object(fields) = row {
        for (key, value) in fields {
            if !key.starts_with('_') {
                clean.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(clean)
}

/// IORegistry 原始属性 → `ports[]`，与 `macos.py` 字段和中文文案逐字一致。
/// 只有 `USB-C` / `MagSafe 3` 且注册表 ID 首次出现时进入列表；
/// 合同/输出功率要求 `connected is True` 且 UUID 匹配恰好一个 SMC 行。
pub fn ports_from_properties(
    raw_ports: &[Value],
    identities: &[Value],
    telemetry: &Value,
) -> Vec<Value> {
    let identity_map = identity_lookup(identities);
    let mut contract_lookup: HashMap<String, Vec<Value>> = HashMap::new();
    let mut channel_lookup: HashMap<String, Vec<Value>> = HashMap::new();
    for row in get(telemetry, "contracts")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let key = uuid_key(get(row, "_uuid"));
        contract_lookup.entry(key).or_default().push(row.clone());
    }
    for row in get(telemetry, "channels")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let key = uuid_key(get(row, "_uuid"));
        channel_lookup.entry(key).or_default().push(row.clone());
    }
    let mut ports = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in raw_ports {
        let kind = get(p, "PortTypeDescription")
            .cloned()
            .unwrap_or(Value::Null);
        let num = get(p, "PortNumber").cloned().unwrap_or(Value::Null);
        let kind_text = kind.as_str().unwrap_or("");
        if !matches!(kind_text, "USB-C" | "MagSafe 3") {
            continue;
        }
        let id_key = get(p, "_registry_id").cloned().unwrap_or(Value::Null);
        if !seen.insert(id_key.to_string()) {
            continue;
        }
        let connected = get(p, "ConnectionActive").cloned().unwrap_or(Value::Null);
        let uuid = get(p, "_controller_uuid").cloned().unwrap_or(Value::Null);
        let uuid_key = uuid_key(Some(&uuid));
        let matches = contract_lookup
            .get(&uuid_key)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let channels = channel_lookup
            .get(&uuid_key)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let contract = if matches.len() == 1 && connected == Value::Bool(true) {
            strip_private(&matches[0])
        } else {
            Value::Null
        };
        let output = if channels.len() == 1 && connected == Value::Bool(true) {
            strip_private(&channels[0])
        } else {
            Value::Null
        };
        let ids = identity_map
            .get(&lookup_key(&kind, &num))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let cable = if connected == Value::Bool(true) && ids.len() == 1 {
            decode_identity(&ids[0])
        } else {
            Value::Null
        };
        let orientation = match get(p, "PlugOrientation").and_then(Value::as_i64) {
            Some(1) => "方向 1",
            Some(2) => "方向 2",
            _ => "未知",
        };
        ports.push(json!({
            "id": id_key,
            "name": get(p, "PortDescription").cloned()
                .unwrap_or_else(|| json!(format!("{} #{}", kind_text, display_number(&num)))),
            "number": num,
            "contract": contract,
            "output_power": output,
            "kind": kind,
            "connected": connected,
            "transports": get(p, "TransportsActive").cloned().unwrap_or(json!([])),
            "orientation": orientation,
            "cable": cable,
            "overcurrent_count": get(p, "Overcurrent Count").cloned().unwrap_or(Value::Null),
            "plug_count": get(p, "Plug Event Count").cloned().unwrap_or(Value::Null),
            "liquid_detected": get(p, "LDCM_LiquidDetected").cloned().unwrap_or(Value::Null),
            "pin_configuration": get(p, "Pin Configuration").cloned().unwrap_or(json!({})),
            "source": json!(format!("{}{}", get(p, "IOClass").and_then(Value::as_str)
                .unwrap_or("IOPort"), PORT_SOURCE_SUFFIX))
        }));
    }
    ports.sort_by(|a, b| display_value(a.get("name")).cmp(&display_value(b.get("name"))));
    ports
}

fn display_number(value: &Value) -> String {
    display_value(Some(value))
}

fn display_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(v)) => if *v { "True" } else { "False" }.into(),
        Some(Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
    }
}

/// IORegistry DP 节点 → `display_links[]`，并把链接挂到唯一匹配的父端口。
/// 父端口用 `(kind, number)` 精确匹配；匹配不唯一时不分配（不猜测）。
pub fn attach_display_links(ports: &mut [Value], nodes: &[Value]) -> Vec<Value> {
    let mut links = Vec::new();
    for node in nodes {
        let link = json!({
            "active": get(node, "Active").cloned().unwrap_or(Value::Null),
            "lane_count": get(node, "LaneCount").cloned().unwrap_or(Value::Null),
            "max_lane_count": get(node, "MaxLaneCount").cloned().unwrap_or(Value::Null),
            "link_rate": get(node, "LinkRate").cloned().unwrap_or(Value::Null),
            "link_rate_description": get(node, "LinkRateDescription").cloned().unwrap_or(Value::Null),
            "tunneled": get(node, "Tunneled").cloned().unwrap_or(Value::Null),
            "name": get(node, "TransportDescription").cloned().unwrap_or(json!("DisplayPort")),
            "source": DP_SOURCE
        });
        links.push(link.clone());
        let kind = get(node, "ParentBuiltInPortTypeDescription")
            .cloned()
            .unwrap_or(Value::Null);
        let num = get(node, "ParentBuiltInPortNumber")
            .cloned()
            .unwrap_or(Value::Null);
        let indices: Vec<usize> = ports
            .iter()
            .enumerate()
            .filter(|(_, p)| get(p, "kind") == Some(&kind) && get(p, "number") == Some(&num))
            .map(|(i, _)| i)
            .collect();
        if indices.len() == 1 {
            let port = &mut ports[indices[0]];
            if let Value::Object(fields) = port {
                let entry = fields
                    .entry("displayport")
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(list) = entry {
                    list.push(link);
                }
            }
        }
    }
    links
}

/// 唯一有效输入合同 → 系统输入轨；修改 `power` 并给该端口写 `input_power`。
/// 条件：`external_connected is True` 且恰有一个带合同、label 不含 "host" 的端口。
pub fn assign_input_power(power: &mut Value, ports: &mut [Value]) {
    let ext_true = get(power, "external_connected") == Some(&Value::Bool(true));
    if !ext_true {
        return;
    }
    let inbound: Vec<usize> = ports
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            py_truthy(get(p, "contract").unwrap_or(&Value::Null))
                && !get(p, "contract")
                    .and_then(|c| get(c, "label"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase()
                    .contains("host")
        })
        .map(|(i, _)| i)
        .collect();
    if inbound.len() != 1 {
        return;
    }
    let index = inbound[0];
    let id = get(&ports[index], "id").cloned().unwrap_or(Value::Null);
    let contract = get(&ports[index], "contract")
        .cloned()
        .unwrap_or(Value::Null);
    if let Value::Object(fields) = power {
        fields.insert("input_port_id".into(), id);
        fields.insert(
            "contract_w".into(),
            get(&contract, "watts").cloned().unwrap_or(Value::Null),
        );
        fields.insert(
            "contract_voltage_v".into(),
            get(&contract, "voltage_v").cloned().unwrap_or(Value::Null),
        );
        fields.insert(
            "contract_current_a".into(),
            get(&contract, "current_a").cloned().unwrap_or(Value::Null),
        );
    }
    let input_power = json!({
        "watts": get(power, "input_w").cloned().unwrap_or(Value::Null),
        "voltage_v": get(power, "input_voltage_v").cloned().unwrap_or(Value::Null),
        "current_a": get(power, "input_current_a").cloned().unwrap_or(Value::Null),
        "source": get(power, "source").cloned().unwrap_or(Value::Null),
        "scope": INPUT_SCOPE
    });
    if let Value::Object(fields) = &mut ports[index] {
        fields.insert("input_power".into(), input_power);
    }
}

/// 完整的平台无关采样：FFI 只负责把 IORegistry/CoreGraphics/system_profiler
/// 数据装入 [`RegistryData`]，本函数按 Python 顺序生成同一份快照。
pub struct BackendLogic {
    pub fabric: Vec<Value>,
    pub fabric_error: Option<String>,
}

impl Default for BackendLogic {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendLogic {
    pub fn new() -> Self {
        Self {
            fabric: Vec::new(),
            fabric_error: None,
        }
    }

    pub fn set_fabric(&mut self, fabric: Vec<Value>, error: Option<String>) {
        self.fabric = fabric;
        self.fabric_error = error;
    }

    pub fn sample(&self, data: &RegistryData, telemetry: &Value, displays: &[Value]) -> Value {
        let mut errors = data.errors.clone();
        if let Some(error) = &self.fabric_error {
            errors.push(error.clone());
        }
        let battery = data
            .batteries
            .first()
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let mut power = power_from_battery_with_presence(
            &battery,
            telemetry,
            data.batteries_present || !data.batteries.is_empty(),
        );
        let mut raw_ports = data.ports.clone();
        if raw_ports.is_empty() {
            for (class, rows) in &data.port_class_results {
                if PORT_CLASSES.contains(&class.as_str()) {
                    raw_ports.extend(rows.iter().cloned());
                }
            }
        }
        let mut ports = ports_from_properties(&raw_ports, &data.identities, telemetry);
        let links = attach_display_links(&mut ports, &data.displayport);
        assign_input_power(&mut power, &mut ports);
        // enrich 生成 pin_map/negotiation；Python 由 engine 调用，这里保持后端
        // 纯逻辑也能产出完整 ports[] schema，便于波次差分。
        for port in &mut ports {
            if let Value::Object(fields) = port {
                let view = Value::Object(fields.clone());
                fields.insert("pin_map".into(), pin_map(&view).unwrap_or(Value::Null));
                fields.insert("negotiation".into(), negotiation(&view, &power));
            }
        }
        json!({
            "platform": "macOS",
            "power": power,
            "ports": Value::Array(ports),
            "devices": Value::Array(usb_devices_from_properties(&data.usb_devices)),
            "displays": Value::Array(displays.to_vec()),
            "display_links": Value::Array(links),
            "fabric": Value::Array(self.fabric.clone()),
            "errors": Value::Array(errors.into_iter().map(Value::String).collect()),
            "limitations": Value::Array(LIMITATIONS.iter().map(|s| json!(s)).collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary_plist(value: PlistValue) -> Vec<u8> {
        let mut bytes = Vec::new();
        value.to_writer_binary(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn binary_plist_converts_all_property_types_including_data() {
        let mut dict = plist::dictionary::Dictionary::new();
        dict.insert("Name".into(), PlistValue::String("USB-C".into()));
        dict.insert("PortNumber".into(), PlistValue::Integer(3.into()));
        dict.insert("Voltage".into(), PlistValue::Real(20.5));
        dict.insert("ConnectionActive".into(), PlistValue::Boolean(true));
        dict.insert(
            "VDOs".into(),
            PlistValue::Data(vec![0x12, 0x34, 0x56, 0x78]),
        );
        dict.insert(
            "Items".into(),
            PlistValue::Array(vec![
                PlistValue::String("a".into()),
                PlistValue::Integer((-7_i64).into()),
            ]),
        );
        let bytes = binary_plist(PlistValue::Dictionary(dict));
        assert_eq!(&bytes[..8], b"bplist00");
        let value = plist_bytes_to_json(&bytes).unwrap();
        assert_eq!(value["Name"], json!("USB-C"));
        assert_eq!(value["PortNumber"], json!(3));
        assert_eq!(value["Voltage"], json!(20.5));
        assert_eq!(value["ConnectionActive"], json!(true));
        assert_eq!(value["VDOs"], json!([0x12, 0x34, 0x56, 0x78]));
        assert_eq!(value["Items"], json!(["a", -7]));
        assert_eq!(PLIST_BINARY_FORMAT_V1_0, 200);
        assert!(plist_bytes_to_json(b"not a plist").is_err());
    }

    #[test]
    fn uuid_normalization_and_hpm_parent_class() {
        assert_eq!(
            uuid_from_properties(&json!({"UUID": "00112233-4455-6677-8899-AABBCCDDEEFF"})),
            Some("00112233445566778899aabbccddeeff".into())
        );
        assert_eq!(
            uuid_from_properties(&json!({"UUID": &"0".repeat(32)})),
            None
        );
        assert_eq!(
            uuid_from_properties(&json!({"UUID": &"Ａ".repeat(32)})),
            None
        );
        assert_eq!(uuid_from_properties(&json!({"UUID": 42})), None);
        assert_eq!(uuid_from_properties(&json!({})), None);
        assert!(is_hpm_device_class(b"AppleHPMDeviceType9"));
        assert!(!is_hpm_device_class(b"IOPort"));
    }

    #[test]
    fn power_uses_system_telemetry_only_with_evidence() {
        let battery = json!({
            "ExternalConnected": true,
            "IsCharging": false,
            "CurrentCapacity": 80,
            "MaxCapacity": 100,
            "Amperage": (18_446_744_073_709_551_615_u64 - 999),
            "Voltage": 12000,
            "AdapterDetails": {"Watts": 140, "Name": "适配器",
                                "UsbHvcMenu": [{"MaxVoltage": 20000, "MaxCurrent": 5000}]},
            "PowerTelemetryData": {"SystemPowerIn": 94500, "SystemVoltageIn": 20000,
                                   "SystemCurrentIn": 4725}
        });
        let telemetry = json!({
            "status": "AppleSMC 只读连接可用",
            "system": {"watts": 94.5, "voltage_v": 20.0, "current_a": 4.7,
                       "independent_power": true, "source": "AppleSMC VD0R / ID0R / PDTR"},
            "battery_discharge_w": 12.5
        });
        let power = power_from_battery(&battery, &telemetry);
        assert_eq!(power["external_connected"], json!(true));
        assert_eq!(power["battery_percent"], json!(80.0));
        assert_eq!(power["adapter_rated_w"], json!(140));
        assert_eq!(
            power["profiles"][0],
            json!({"voltage_v": 20.0, "current_a": 5.0})
        );
        assert_eq!(power["input_w"], json!(94.5));
        assert_eq!(power["input_voltage_v"], json!(20.0));
        assert_eq!(power["smc_status"], json!("AppleSMC 只读连接可用"));
        assert_eq!(power["independent_input_power"], json!(true));
        assert_eq!(
            power["source"],
            json!("AppleSMC VD0R / ID0R / PDTR（DC 输入，非墙端交流功率）")
        );
        // `ext is True` 时即使没有电池遥测，SMC 系统功率也会覆盖 input_w；
        // 这里用 external_connected=None 模拟未接电源，验证 SMC 不被误用。
        let cable_only = power_from_battery(
            &json!({}),
            &json!({
                "status": "尚未读取",
                "system": {"watts": 20.0, "voltage_v": 5.0, "current_a": 4.0,
                           "independent_power": false, "source": "x"}
            }),
        );
        assert_eq!(cable_only["input_w"], json!(20.0));
        let no_external = power_from_battery_with_presence(
            &json!({"ExternalConnected": true}),
            &json!({"status": "尚未读取", "system": Value::Null}),
            true,
        );
        assert_eq!(no_external["input_w"], Value::Null);
        assert_eq!(no_external["profiles"], json!([]));
        let on_battery = power_from_battery(
            &json!({"ExternalConnected": false}),
            &json!({
                "status": "可用", "system": Value::Null, "battery_discharge_w": 12.5
            }),
        );
        assert_eq!(on_battery["battery_net_w"], json!(-12.5));
        assert_eq!(on_battery["battery_source"], json!(BATTERY_SOURCE));
        assert_eq!(on_battery["adapter_rated_w"], Value::Null);
    }

    fn telemetry_with(uuid: &str) -> Value {
        json!({
            "status": "AppleSMC 只读连接可用",
            "system": {"watts": 90.0, "voltage_v": 20.0, "current_a": 4.5,
                       "independent_power": true, "source": "AppleSMC VD0R / ID0R / PDTR"},
            "channels": [{"_uuid": uuid, "watts": 5.0, "voltage_v": 5.0, "current_a": 1.0,
                          "present": true, "direction": "out", "source": "遥测"}],
            "contracts": [{"_uuid": uuid, "watts": 100.0, "voltage_v": 20.0,
                           "current_a": 5.0, "label": "USB-C", "source": "合同"}],
            "battery_discharge_w": Value::Null
        })
    }

    fn port_props(uuid: Option<&str>, kind: &str, num: i64, id: &str) -> Value {
        json!({
            "_registry_id": id,
            "PortTypeDescription": kind,
            "PortNumber": num,
            "PortDescription": format!("{kind} 端口 {num}"),
            "ConnectionActive": true,
            "PlugOrientation": 2,
            "TransportsActive": ["USB4"],
            "_controller_uuid": uuid,
            "Overcurrent Count": 0,
            "Plug Event Count": 3,
            "LDCM_LiquidDetected": false,
            "Pin Configuration": {"tx1": 1, "rx1": 5},
            "IOClass": "IOPort"
        })
    }

    #[test]
    fn ports_match_hpm_uuid_single_contract_and_cable_identity() {
        let uuid = "00112233445566778899aabbccddeeff";
        let props = vec![
            port_props(Some(uuid), "USB-C", 1, "100"),
            port_props(None, "MagSafe 3", 2, "200"),
            port_props(Some(uuid), "Thunderbolt", 3, "300"),
            port_props(Some(uuid), "USB-C", 1, "100"),
        ];
        let identities = vec![json!({
            "ParentBuiltInPortTypeDescription": "USB-C",
            "ParentBuiltInPortNumber": 1,
            "Specification Revision": 3,
            "Metadata": {"VDOs": [[0x34, 0x12, 0x00, 0x18], [0,0,0,0],
                                  [1,0,0x78,0x56], [0x42,0,0,0]]}
        })];
        let ports = ports_from_properties(&props, &identities, &telemetry_with(uuid));
        assert_eq!(ports.len(), 2, "重复 registry id 与非 USB-C/MagSafe 被排除");
        let usb_c = ports.iter().find(|p| p["kind"] == "USB-C").unwrap();
        assert_eq!(usb_c["id"], json!("100"));
        assert_eq!(usb_c["number"], json!(1));
        assert_eq!(usb_c["orientation"], json!("方向 2"));
        assert_eq!(usb_c["transports"], json!(["USB4"]));
        assert_eq!(usb_c["source"], json!("IOPort 注册表属性"));
        assert_eq!(usb_c["contract"]["watts"], json!(100.0));
        assert!(usb_c["contract"].get("_uuid").is_none());
        assert_eq!(usb_c["output_power"]["watts"], json!(5.0));
        assert!(usb_c["output_power"].get("_uuid").is_none());
        assert_eq!(usb_c["cable"]["current_a"], json!(5));
        assert_eq!(usb_c["cable"]["power_ceiling_w"], json!(100));
        let magsafe = ports.iter().find(|p| p["kind"] == "MagSafe 3").unwrap();
        assert_eq!(magsafe["contract"], Value::Null);
        assert_eq!(magsafe["output_power"], Value::Null);
        assert_eq!(magsafe["cable"], Value::Null);
    }

    #[test]
    fn ambiguous_uuid_or_disconnected_port_does_not_get_contract_or_cable() {
        let uuid = "aabbccddeeff00112233445566778899";
        let mut telemetry = telemetry_with(uuid);
        telemetry["contracts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"_uuid": uuid, "watts": 60.0, "label": "second"}));
        let props = vec![port_props(Some(uuid), "USB-C", 1, "1")];
        let ports = ports_from_properties(&props, &[], &telemetry);
        assert_eq!(ports[0]["contract"], Value::Null);
        let mut disconnected = port_props(Some(uuid), "USB-C", 1, "2");
        disconnected["ConnectionActive"] = json!(false);
        let ports = ports_from_properties(&[disconnected], &[], &telemetry_with(uuid));
        assert_eq!(ports[0]["contract"], Value::Null);
        assert_eq!(ports[0]["output_power"], Value::Null);
        assert_eq!(ports[0]["cable"], Value::Null);
    }

    #[test]
    fn displayport_link_attaches_only_to_unique_parent_port() {
        let uuid = "00112233445566778899aabbccddeeff";
        let props = vec![
            port_props(Some(uuid), "USB-C", 1, "1"),
            port_props(Some(uuid), "USB-C", 2, "2"),
        ];
        let mut ports = ports_from_properties(&props, &[], &telemetry_with(uuid));
        let nodes = vec![
            json!({"ParentBuiltInPortTypeDescription": "USB-C", "ParentBuiltInPortNumber": 1,
                   "Active": true, "LaneCount": 4, "MaxLaneCount": 4, "LinkRate": 20,
                   "LinkRateDescription": "HBR2", "Tunneled": false,
                   "TransportDescription": "DisplayPort"}),
            json!({"ParentBuiltInPortTypeDescription": "USB-C", "ParentBuiltInPortNumber": 9,
                   "Active": false}),
        ];
        let links = attach_display_links(&mut ports, &nodes);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0]["name"], json!("DisplayPort"));
        assert_eq!(links[0]["source"], json!(DP_SOURCE));
        assert_eq!(ports[0]["displayport"][0]["lane_count"], json!(4));
        assert!(ports[1].get("displayport").is_none());
        assert!(links[1]["name"] == json!("DisplayPort"));
    }

    #[test]
    fn input_power_is_assigned_only_for_one_non_host_contract() {
        let uuid = "00112233445566778899aabbccddeeff";
        let props = vec![port_props(Some(uuid), "USB-C", 1, "in")];
        let mut ports = ports_from_properties(&props, &[], &telemetry_with(uuid));
        let mut power = power_from_battery(
            &json!({"ExternalConnected": true, "PowerTelemetryData": {"SystemPowerIn": 90000}}),
            &telemetry_with(uuid),
        );
        assign_input_power(&mut power, &mut ports);
        assert_eq!(power["input_port_id"], json!("in"));
        assert_eq!(power["contract_w"], json!(100.0));
        assert_eq!(power["contract_voltage_v"], json!(20.0));
        assert_eq!(ports[0]["input_power"]["watts"], json!(90.0));
        assert_eq!(ports[0]["input_power"]["scope"], json!(INPUT_SCOPE));
        // 两个输入合同（或 label 含 host）时不分配。
        let mut two = ports_from_properties(&props, &[], &telemetry_with(uuid));
        two.push(
            ports_from_properties(
                &[port_props(Some(uuid), "USB-C", 2, "in2")],
                &[],
                &telemetry_with(uuid),
            )[0]
            .clone(),
        );
        let mut power = power.clone();
        power.as_object_mut().unwrap().remove("input_port_id");
        assign_input_power(&mut power, &mut two);
        assert!(power.get("input_port_id").is_none());
        let mut host = ports_from_properties(&props, &[], &telemetry_with(uuid));
        host[0]["contract"]["label"] = json!("host port");
        let mut power = power_from_battery(
            &json!({"ExternalConnected": true}),
            &json!({
                "status": "可用", "system": Value::Null
            }),
        );
        assign_input_power(&mut power, &mut host);
        assert!(power.get("input_port_id").is_none());
        // 未接外部电源时同样不分配。
        let mut power = json!({"external_connected": false});
        assign_input_power(&mut power, &mut ports);
        assert!(power.get("input_port_id").is_none());
    }

    #[test]
    fn display_modes_do_not_mix_refresh_and_resolution() {
        let query = DisplayQuery {
            identifier: 1234,
            builtin: true,
            current: json!({"width": 1920, "height": 1080, "refresh_hz": 240.0}),
            modes: vec![
                json!({"width": 3840, "height": 2160, "refresh_hz": 60.0}),
                json!({"width": 1920, "height": 1080, "refresh_hz": 240.0}),
                json!({"width": 3840, "height": 2160, "refresh_hz": 60.0}),
            ],
        };
        let displays = display_modes_from_queries(&[query]);
        assert_eq!(displays[0]["name"], json!("内置显示器 #1234"));
        assert_eq!(displays[0]["refresh_hz"], json!(240.0));
        assert_eq!(displays[0]["offered_mode_count"], json!(2));
        assert_eq!(displays[0]["highest_mode"]["width"], json!(3840));
        assert_eq!(displays[0]["highest_mode"]["refresh_hz"], json!(60.0));
        assert_eq!(
            displays[0]["verdict"],
            json!("当前像素模式低于系统列出的最高分辨率；原因尚不能归属到线缆")
        );
        let external = display_modes_from_queries(&[DisplayQuery {
            identifier: 2,
            builtin: false,
            current: json!({"width": 3024, "height": 1964, "refresh_hz": 0.0}),
            modes: vec![],
        }]);
        assert_eq!(external[0]["name"], json!("外接显示器 #2"));
        assert_eq!(external[0]["refresh_hz"], Value::Null);
        assert_eq!(external[0]["offered_mode_count"], json!(0));
    }

    #[test]
    fn usb_device_speed_and_fabric_tree() {
        let devices = usb_devices_from_properties(&[
            json!({"USB Product Name": "Hub", "USB Vendor Name": "V",
                   "idVendor": 0x1234, "idProduct": 0x5678, "Device Speed": 3}),
            json!({"Device Speed": 4}),
            json!({"Device Speed": 9}),
        ]);
        assert_eq!(devices[0]["speed"], json!("5 Gbps"));
        assert_eq!(devices[0]["name"], json!("Hub"));
        assert_eq!(devices[1]["speed"], json!("SuperSpeedPlus（速率未知）"));
        assert_eq!(devices[2]["speed"], json!("未知"));
        assert_eq!(devices[2]["name"], json!("USB 设备"));
        assert_eq!(
            devices[0]["source"],
            json!("IOUSBHostDevice.Device Speed；未映射到物理 USB-C 口")
        );
        let profiler = json!([{
            "_name": "root",
            "_items": [{
                "_name": "Dock",
                "device_name_key": "扩展坞",
                "mode_key": "Thunderbolt 4",
                "receptacle_upstream1": {"current_speed_key": "40 Gb/s"},
                "_items": [{"_name": "Screen", "mode_key": "DP"}]
            }]
        }]);
        let fabric = fabric_from_profiler(profiler.as_array().unwrap());
        assert_eq!(fabric.len(), 2);
        assert_eq!(fabric[0]["name"], json!("扩展坞"));
        assert_eq!(fabric[0]["path"], json!("/root/Dock"));
        assert_eq!(fabric[0]["speed"], json!("40 Gb/s"));
        assert_eq!(fabric[0]["source"], json!(FABRIC_SOURCE));
        assert_eq!(fabric[1]["speed"], json!("未知"));
    }

    #[test]
    fn full_snapshot_shape_and_enrich_fields() {
        let uuid = "00112233445566778899aabbccddeeff";
        let mut data = RegistryData::default();
        data.batteries.push(json!({"ExternalConnected": true}));
        data.ports.push(port_props(Some(uuid), "USB-C", 1, "1"));
        data.displayport.push(json!({
            "ParentBuiltInPortTypeDescription": "USB-C",
            "ParentBuiltInPortNumber": 1,
            "Active": true
        }));
        data.usb_devices.push(json!({"Device Speed": 2}));
        data.errors.push("显示器: 模拟错误".into());
        let mut logic = BackendLogic::new();
        logic.set_fabric(
            vec![json!({"name": "Fabric", "path": "/x", "mode": "TB"})],
            None,
        );
        let snapshot = logic.sample(&data, &telemetry_with(uuid), &[]);
        assert_eq!(snapshot["platform"], json!("macOS"));
        assert_eq!(
            snapshot["ports"][0]["pin_map"]["rows"][0]
                .as_array()
                .unwrap()
                .len(),
            12
        );
        assert_eq!(
            snapshot["ports"][0]["negotiation"]["contract_w"],
            json!(100.0)
        );
        assert_eq!(
            snapshot["ports"][0]["displayport"][0]["active"],
            json!(true)
        );
        assert_eq!(snapshot["devices"][0]["speed"], json!("480 Mbps"));
        assert_eq!(snapshot["fabric"][0]["name"], json!("Fabric"));
        assert_eq!(snapshot["errors"], json!(["显示器: 模拟错误"]));
        assert_eq!(snapshot["limitations"].as_array().unwrap().len(), 3);
        assert_eq!(snapshot["limitations"][0], json!(LIMITATIONS[0]));
        let telemetry = json!({
            "status": "此系统未提供 AppleSMC",
            "system": Value::Null,
            "channels": [],
            "contracts": []
        });
        let snapshot = BackendLogic::new().sample(&RegistryData::default(), &telemetry, &[]);
        let power = &snapshot["power"];
        assert_eq!(power["smc_status"], json!("此系统未提供 AppleSMC"));
        assert_eq!(power["independent_input_power"], json!(false));
        assert_eq!(power["input_w"], Value::Null);
        assert_eq!(power["profiles"], json!([]));
        assert_eq!(power["adapter_rated_w"], Value::Null);
        assert_eq!(power["source"], json!(POWER_SOURCE));
        assert!(power.get("battery_discharge_w").is_none());
        // 空 IOPort 时回退到 HPM/TC 控制器类（按 macos.py 的六个类名）。
        let mut fallback = RegistryData::default();
        fallback.port_class_results.push((
            "AppleTCControllerType11".into(),
            vec![port_props(None, "MagSafe 3", 9, "9")],
        ));
        let snapshot = BackendLogic::new().sample(&fallback, &telemetry_with(uuid), &[]);
        assert_eq!(snapshot["ports"][0]["name"], json!("MagSafe 3 端口 9"));
    }
}
