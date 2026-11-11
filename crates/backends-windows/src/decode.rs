//! `cablescope/backends/windows.py` 中 Win32 与 USB IOCTL 回复的纯解码逻辑。
//!
//! 此处不引入 Windows API：所有回复都是字节切片，便于用合成字节复刻 Python
//! 的 `WindowsReplyTests` 与 `WindowsPortConnectorTests`。`usbioctl.h` 的
//! 端口连接器属性与连接信息结构是 `pack(1)`，因此不用 `#[repr(C)]` 结构体
//! 读取，而是逐字段按 Python `struct.unpack_from` 的偏移提取。

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES: u32 = 0x220458;
pub const USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH: usize = 18;
pub const USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH: usize = 65536;
pub const PORT_USER_CONNECTABLE: u32 = 0x1;
pub const PORT_DEBUG_CAPABLE: u32 = 0x2;
pub const PORT_HAS_MULTIPLE_COMPANIONS: u32 = 0x4;
pub const PORT_CONNECTOR_IS_TYPE_C: u32 = 0x8;
pub const PORT_PROPERTIES_KNOWN_BITS: u32 = PORT_USER_CONNECTABLE
    | PORT_DEBUG_CAPABLE
    | PORT_HAS_MULTIPLE_COMPANIONS
    | PORT_CONNECTOR_IS_TYPE_C;
pub const UCSI_STATES: [&str; 3] = ["available", "no_data_plane", "query_failed"];

pub const USB_HUB_INFORMATION_EX: u32 = 0x220454;
pub const IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX: u32 = 0x220448;
pub const IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2: u32 = 0x22045c;

const BATTERY_SOURCE: &str = "root/wmi BatteryStatus，电池净充放电（15 秒缓存），不是 USB 输入功率";
const POWER_SOURCE: &str = "GetSystemPowerStatus；输入功率未公开";
const DEVICE_SOURCE: &str = "USB Hub IOCTL EX / EX_V2；设备链路，不是线缆上限";
const CANDIDATE_SOURCE: &str = "Hub IOCTL 端口属性（候选，未确认机身物理口）";
const AMBIGUOUS_SOURCE_NOTE: &str = "；伴口分组键冲突，保留独立行以避免误合并";

fn u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn require_offset(data: &[u8], offset: usize, count: usize, message: &str) -> Result<(), String> {
    if data.len() < offset + count {
        Err(message.to_string())
    } else {
        Ok(())
    }
}

/// Python `hashlib.sha256("\0".join(parts).encode("utf-8")).hexdigest()[:16]`。
///
/// 与 `core::py::fingerprint` 不同：这里拼的是带 NUL 分隔符的 UTF-8 字符串，
/// 不是原始字节 blob；`windows.py` 自己保有一份同名 helper，必须逐字复刻。
pub fn digest(parts: &[&str]) -> String {
    let text = parts.join("\0");
    let hash = Sha256::digest(text.as_bytes());
    hash[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn companion_group_key(link: &str, port: u16) -> String {
    digest(&[link, &port.to_string()])
}

/// Python `link.strip().replace("/", "\\").casefold()`。
///
/// Windows 设备路径是 ASCII/基本拉丁字符，但 `casefold()` 对少数 Unicode
/// 字母会展开（ß→ss、ẞ→ss、İ→i̇），与 Rust `to_lowercase()` 不同。这里显式
/// 复刻这些展开，保证人工构造的非 ASCII 路径也按 Python 语义归一。
pub fn normal_hub_link(link: &str) -> String {
    link.trim()
        .replace('/', "\\")
        .chars()
        .flat_map(|ch| {
            let expanded: Box<dyn Iterator<Item = char>> = match ch {
                '\u{00df}' | '\u{1e9e}' => Box::new("ss".chars()),
                '\u{0130}' => Box::new("i\u{0307}".chars()),
                _ => Box::new(ch.to_lowercase()),
            };
            expanded.collect::<Vec<_>>()
        })
        .collect()
}

/// 从 `IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX` 的 pack(1) 回复读取设备。
///
/// 偏移：VID/PID 12..16、速度编码 23、is_hub 24、连接状态 31..35；V2 回复
/// 的 16 字节里只有偏移 12 的速度标志参与覆盖。
pub fn decode_connection(data: &[u8], v2: Option<&[u8]>) -> Result<Option<Value>, String> {
    require_offset(data, 31, 4, "USB connection reply is shorter than 35 bytes")?;
    if u32_le(data, 31) != 1 {
        return Ok(None);
    }
    let mut speed = match data[23] {
        0 => "1.5 Mbps",
        1 => "12 Mbps",
        2 => "480 Mbps",
        3 => "SuperSpeed（≥5 Gbps）",
        _ => "未知",
    }
    .to_string();
    if let Some(v2) = v2 {
        if v2.len() >= 16 {
            let flags = u32_le(v2, 12);
            if flags & 4 != 0 {
                speed = "SuperSpeedPlus（≥10 Gbps，未确定 lane 数）".to_string();
            } else if flags & 1 != 0 {
                speed = "SuperSpeed（≥5 Gbps）".to_string();
            }
        }
    }
    let vid = u16_le(data, 12);
    let pid = u16_le(data, 14);
    let mut row = Map::new();
    row.insert("vendor_id".into(), json!(vid));
    row.insert("product_id".into(), json!(pid));
    row.insert("speed".into(), json!(speed));
    row.insert("is_hub".into(), json!(data[24] != 0));
    row.insert("name".into(), json!(format!("USB {vid:04X}:{pid:04X}")));
    row.insert("source".into(), json!(DEVICE_SOURCE));
    Ok(Some(Value::Object(row)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorRow {
    pub connection_index: u32,
    pub actual_length: u32,
    pub user_connectable: bool,
    pub debug_capable: bool,
    pub multiple_companions: bool,
    pub type_c: bool,
    pub reserved_bits: u32,
    pub companion_index: u16,
    pub companion_port: u16,
    pub companion_link: String,
}

/// 解码 `USB_PORT_CONNECTOR_PROPERTIES`；字段顺序只影响错误检查，不改变快照键序。
pub fn decode_connector_properties(data: &[u8]) -> Result<ConnectorRow, String> {
    require_offset(
        data,
        0,
        USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH,
        "USB port connector properties reply is shorter than 18 bytes",
    )?;
    let index = u32_le(data, 0);
    let length = u32_le(data, 4) as usize;
    let properties = u32_le(data, 8);
    let companion_index = u16_le(data, 12);
    let companion_port = u16_le(data, 14);
    if !(USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH..=USB_PORT_CONNECTOR_PROPERTIES_MAX_LENGTH)
        .contains(&length)
    {
        return Err("USB port connector properties ActualLength 无效".to_string());
    }
    if index < 1 {
        return Err("USB port connector properties ConnectionIndex 无效".to_string());
    }
    let tail = if data.len() >= length {
        &data[16..length]
    } else {
        &[][..]
    };
    if tail.len() % 2 != 0 {
        return Err("USB port connector properties WCHAR 长度无效".to_string());
    }
    let units: Vec<u16> = tail
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let link = String::from_utf16_lossy(&units);
    let link = link.split('\0').next().unwrap_or("");
    let flags = properties & PORT_PROPERTIES_KNOWN_BITS;
    Ok(ConnectorRow {
        connection_index: index,
        actual_length: length as u32,
        user_connectable: flags & PORT_USER_CONNECTABLE != 0,
        debug_capable: flags & PORT_DEBUG_CAPABLE != 0,
        multiple_companions: flags & PORT_HAS_MULTIPLE_COMPANIONS != 0,
        type_c: flags & PORT_CONNECTOR_IS_TYPE_C != 0,
        reserved_bits: properties & !PORT_PROPERTIES_KNOWN_BITS,
        companion_index,
        companion_port,
        companion_link: normal_hub_link(link),
    })
}

/// 候选端口；`connected` 与全部硬件测量字段恒为 `null`，不推断物理连接态。
pub fn port_candidate(
    row: &ConnectorRow,
    hub_link: &str,
    label: &str,
    ambiguous: bool,
    discriminator: &str,
) -> Value {
    let port = row.connection_index;
    let port_text = port.to_string();
    let source = if ambiguous {
        format!("{CANDIDATE_SOURCE}{AMBIGUOUS_SOURCE_NOTE}")
    } else {
        CANDIDATE_SOURCE.to_string()
    };
    let mut parts = vec![hub_link, port_text.as_str()];
    if !discriminator.is_empty() {
        parts.push(discriminator);
    }
    json!({
        "id": digest(&parts),
        "name": format!("USB-C 候选 · {label}"),
        "number": port,
        "contract": Value::Null,
        "output_power": Value::Null,
        "input_power": Value::Null,
        "kind": "USB-C",
        "connected": Value::Null,
        "orientation": Value::Null,
        "cable": Value::Null,
        "overcurrent_count": Value::Null,
        "plug_count": Value::Null,
        "liquid_detected": Value::Null,
        "pin_configuration": {},
        "source": source
    })
}

/// 一个已打开的 Hub；`id` 交给 `HubIo::ioctl`，`hub_link` 是路径哈希后的
/// 不可识别组键（原始设备路径永不进入快照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hub {
    pub id: String,
    pub hub_link: String,
    pub index: u32,
}

/// 「句柄打开 + DeviceIoControl」的可注入边界：真机实现走 `ffi::WinHubIo`，
/// 测试用合成字节实现。Python 测试注入的是假 `ioctl` 闭包，语义在此保持一致。
pub trait HubIo {
    /// SetupDi 枚举并逐个打开 Hub 句柄；返回的软错误放进 `errors`。
    fn hubs(&mut self) -> Result<(Vec<Hub>, Vec<String>), String>;
    fn ioctl(
        &mut self,
        handle: &str,
        code: u32,
        initial: &[u8],
        size: usize,
    ) -> Result<Vec<u8>, String>;
    fn close(&mut self, handle: &str);
}

pub type SharedHubIo = Box<dyn HubIo + Send>;

/// 逐个 Hub 查询端口属性与设备连接信息；错误逐条降级为 Python 文案。
pub fn usb_devices(io: &mut dyn HubIo) -> (Vec<Value>, Vec<String>, Vec<Value>) {
    let (hubs, mut errors) = match io.hubs() {
        Ok(result) => result,
        Err(error) => return (Vec::new(), vec![format!("USB Hub: {error}")], Vec::new()),
    };
    let mut devices = Vec::new();
    let mut port_candidates = Vec::new();
    for hub in hubs {
        let hub_info = match io.ioctl(&hub.id, USB_HUB_INFORMATION_EX, &[], 80) {
            Ok(data) => data,
            Err(error) => {
                errors.push(format!("Hub #{}: {}", hub.index, error));
                io.close(&hub.id);
                continue;
            }
        };
        if hub_info.len() < 6 {
            errors.push(format!("Hub #{}: hub information truncated", hub.index));
            io.close(&hub.id);
            continue;
        }
        let count = u16::from_le_bytes([hub_info[4], hub_info[5]]).min(255);
        for port in 1..=u32::from(count) {
            match read_connector_properties(io, &hub.id, port) {
                Ok(rows) => {
                    port_candidates.extend(collect_port_candidates(&rows, &hub.hub_link, hub.index))
                }
                Err(error) => errors.push(format!(
                    "Hub #{} Port {} 端口属性: {}",
                    hub.index, port, error
                )),
            }
            let connection = match io.ioctl(
                &hub.id,
                IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX,
                &port.to_le_bytes(),
                4096,
            ) {
                Ok(data) => data,
                Err(error) => {
                    errors.push(format!("Hub #{} Port {}: {}", hub.index, port, error));
                    continue;
                }
            };
            let mut initial = Vec::with_capacity(16);
            initial.extend_from_slice(&port.to_le_bytes());
            initial.extend_from_slice(&16u32.to_le_bytes());
            initial.extend_from_slice(&4u32.to_le_bytes());
            initial.extend_from_slice(&0u32.to_le_bytes());
            let v2 = io
                .ioctl(
                    &hub.id,
                    IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2,
                    &initial,
                    16,
                )
                .ok();
            match decode_connection(&connection, v2.as_deref()) {
                Ok(Some(mut device)) => {
                    if let Value::Object(fields) = &mut device {
                        fields.insert(
                            "location".into(),
                            json!(format!("Hub #{} / Port {}", hub.index, port)),
                        );
                    }
                    devices.push(device);
                }
                Ok(None) => {}
                Err(error) => errors.push(format!("Hub #{} Port {}: {}", hub.index, port, error)),
            }
        }
        io.close(&hub.id);
    }
    (devices, errors, port_candidates)
}

/// 两阶段查询：先以最小长度读 `ActualLength`，再按精确长度遍历伴口索引。
pub fn read_connector_properties(
    ioctl: &mut dyn HubIo,
    handle: &str,
    port: u32,
) -> Result<Vec<ConnectorRow>, String> {
    let first_bytes = ioctl.ioctl(
        handle,
        IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES,
        &port.to_le_bytes(),
        USB_PORT_CONNECTOR_PROPERTIES_MIN_LENGTH,
    )?;
    let first = decode_connector_properties(&first_bytes)?;
    let mut rows = Vec::new();
    let mut companion: u16 = 0;
    loop {
        let mut initial = Vec::with_capacity(16);
        initial.extend_from_slice(&port.to_le_bytes());
        initial.extend_from_slice(&first.actual_length.to_le_bytes());
        initial.extend_from_slice(&0u32.to_le_bytes());
        initial.extend_from_slice(&companion.to_le_bytes());
        initial.extend_from_slice(&0u16.to_le_bytes());
        let reply = ioctl.ioctl(
            handle,
            IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES,
            &initial,
            first.actual_length as usize,
        )?;
        let row = decode_connector_properties(&reply)?;
        if row.connection_index != port {
            return Err("USB port connector properties ConnectionIndex 不匹配".to_string());
        }
        let stop = !row.multiple_companions || row.companion_port == 0 || companion >= 255;
        rows.push(row);
        if stop {
            break;
        }
        companion += 1;
    }
    Ok(rows)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateItem {
    hub_link: String,
    hub_index: u32,
    row: ConnectorRow,
}

#[cfg(test)]
pub(crate) fn companion_key_for_test(link: &str, port: u16) -> String {
    companion_group_key(link, port)
}

fn group_ambiguous(items: &[CandidateItem]) -> bool {
    let identities: BTreeSet<(&str, u16)> = items
        .iter()
        .map(|item| (item.row.companion_link.as_str(), item.row.companion_port))
        .collect();
    if identities.len() > 1 {
        return true;
    }
    let hubs: BTreeSet<(&str, u32)> = items
        .iter()
        .map(|item| (item.hub_link.as_str(), item.hub_index))
        .collect();
    identities == BTreeSet::from([("", 0)]) && hubs.len() > 1
}

/// 按伴口组键合并候选；歧义时保留独立行，`max` 并列取第一个（Python 语义）。
pub fn collect_port_candidates(
    rows: &[ConnectorRow],
    hub_link: &str,
    hub_index: u32,
) -> Vec<Value> {
    let mut groups: Vec<(String, Vec<CandidateItem>)> = Vec::new();
    for row in rows {
        if !row.user_connectable || row.debug_capable {
            continue;
        }
        let port = row.connection_index.to_string();
        let key = if row.companion_port == 0 {
            digest(&["no-companion", hub_link, &port])
        } else {
            companion_group_key(&row.companion_link, row.companion_port)
        };
        let item = CandidateItem {
            hub_link: hub_link.to_string(),
            hub_index,
            row: row.clone(),
        };
        push_group(&mut groups, key, item);
    }
    collect_port_candidates_from_groups(groups)
}

fn push_group(groups: &mut Vec<(String, Vec<CandidateItem>)>, key: String, item: CandidateItem) {
    if let Some((_, items)) = groups.iter_mut().find(|(k, _)| *k == key) {
        items.push(item);
    } else {
        groups.push((key, vec![item]));
    }
}

/// 允许测试模拟哈希冲突：把伴口分组键覆盖为固定值，验证歧义拆分文案。
pub fn collect_port_candidates_with_key(
    rows: &[ConnectorRow],
    hub_link: &str,
    hub_index: u32,
    group_key: impl Fn(&str, u16) -> String,
) -> Vec<Value> {
    let mut groups: Vec<(String, Vec<CandidateItem>)> = Vec::new();
    for row in rows {
        if !row.user_connectable || row.debug_capable {
            continue;
        }
        let port = row.connection_index.to_string();
        let key = if row.companion_port == 0 {
            digest(&["no-companion", hub_link, &port])
        } else {
            group_key(&row.companion_link, row.companion_port)
        };
        let item = CandidateItem {
            hub_link: hub_link.to_string(),
            hub_index,
            row: row.clone(),
        };
        push_group(&mut groups, key, item);
    }
    collect_port_candidates_from_groups(groups)
}

fn collect_port_candidates_from_groups(groups: Vec<(String, Vec<CandidateItem>)>) -> Vec<Value> {
    let mut result = Vec::new();
    for (_, items) in &groups {
        if group_ambiguous(&items) {
            for item in items {
                let row = &item.row;
                if !row.type_c {
                    continue;
                }
                let discriminator = format!(
                    "{}:{}:{}",
                    row.companion_index, row.companion_link, row.companion_port
                );
                result.push(port_candidate(
                    row,
                    &item.hub_link,
                    &format!("Hub #{} Port {}", item.hub_index, row.connection_index),
                    true,
                    &discriminator,
                ));
            }
            continue;
        }
        let mut chosen: Option<&CandidateItem> = None;
        for item in items {
            let better = match chosen {
                None => true,
                Some(current) => {
                    (item.row.type_c, -(item.row.companion_index as i32))
                        > (current.row.type_c, -(current.row.companion_index as i32))
                }
            };
            if better {
                chosen = Some(item);
            }
        }
        if let Some(item) = chosen {
            let row = &item.row;
            if row.type_c {
                result.push(port_candidate(
                    row,
                    &item.hub_link,
                    &format!("Hub #{} Port {}", item.hub_index, row.connection_index),
                    false,
                    "",
                ));
            }
        }
    }
    result.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
    });
    result
}

pub fn classify_ucsi_state(inventory: &Value) -> &'static str {
    let state = inventory.get("ucsi_state").and_then(Value::as_str);
    UCSI_STATES
        .iter()
        .copied()
        .find(|candidate| Some(*candidate) == state)
        .unwrap_or("query_failed")
}

pub fn ucsi_limitations(state: &str) -> &'static str {
    match state {
        "available" => "探测到 UCM / UCSI 设备节点；本工具仅做只读存在性探测，未读取或解析 UCSI 命令、PDO / RDO 与线缆身份。E-Marker、PD 档位、电源角色与单端口输入功率仍保持未知。Hub IOCTL 仅识别 Type-C 端口候选与设备枚举位置；未启用测试接口、未修改注册表、未重启设备。",
        "no_data_plane" => "本机固件未暴露可读取的 UCSI 设备节点或 WMI 数据面；只读 Hub IOCTL 仅识别 Type-C 端口候选与设备枚举位置。E-Marker、PD 档位、电源角色与单端口输入功率保持未知。未启用测试接口、未修改注册表、未重启设备。",
        _ => "UCSI 数据面探测查询未能完成，无法确认是否存在可读取的设备节点；本次不据此推断 PD 合同、电源角色、E-Marker 或单端口输入功率。只读 Hub IOCTL 仍可识别 Type-C 端口候选；未启用测试接口、未修改注册表、未重启设备。",
    }
}

pub fn power_status_from_fields(
    ac: u8,
    flags: u8,
    percent: u8,
    ok: bool,
    error: Option<String>,
    errors: &mut Vec<String>,
) -> Value {
    if !ok {
        if let Some(message) = error {
            errors.push(message);
        }
    }
    json!({
        "external_connected": if ok && ac != 255 { json!(ac == 1) } else { Value::Null },
        "charging": if ok && flags != 255 && flags & 128 == 0 { json!(flags & 8 != 0) } else { Value::Null },
        "battery_percent": if ok && percent != 255 && flags & 128 == 0 { json!(percent) } else { Value::Null },
        "input_w": Value::Null,
        "adapter_rated_w": Value::Null,
        "input_voltage_v": Value::Null,
        "input_current_a": Value::Null,
        "battery_net_w": Value::Null,
        "profiles": [],
        "source": POWER_SOURCE
    })
}

/// 复刻 `Backend.sample()` 中电池/Hub/显示器失败后的降级与快照组装。
pub fn assemble_snapshot(
    mut power: Value,
    mut devices: Vec<Value>,
    ports: Vec<Value>,
    displays: Vec<Value>,
    errors: Vec<String>,
    inventory: &Value,
) -> Value {
    let ucsi_state = classify_ucsi_state(inventory);
    let single_battery;
    let batteries: &[Value] = match inventory.get("batteries") {
        Some(Value::Array(rows)) => rows.as_slice(),
        Some(Value::Object(row)) => {
            single_battery = Value::Object(row.clone());
            std::slice::from_ref(&single_battery)
        }
        _ => &[],
    };
    if batteries.len() == 1 {
        let row = &batteries[0];
        let charge =
            cablescope_core::number(&row.get("ChargeRate").cloned().unwrap_or(Value::Null));
        let discharge =
            cablescope_core::number(&row.get("DischargeRate").cloned().unwrap_or(Value::Null));
        let charge_w = charge
            .as_ref()
            .and_then(|v| v.as_f64())
            .filter(|rate| (0.0..0xffff_ffffu32 as f64).contains(rate));
        let discharge_w = discharge
            .as_ref()
            .and_then(|v| v.as_f64())
            .filter(|rate| (0.0..0xffff_ffffu32 as f64).contains(rate));
        if let Some(fields) = power.as_object_mut() {
            if is_truthy(row.get("Charging")) && charge_w.is_some() {
                fields.insert(
                    "battery_net_w".into(),
                    cablescope_core::scaled(&row["ChargeRate"], 1000.0).unwrap_or(Value::Null),
                );
            } else if is_truthy(row.get("Discharging")) && discharge_w.is_some() {
                if let Some(value) = cablescope_core::scaled(&row["DischargeRate"], 1000.0) {
                    let negative = value.as_f64().map(|v| json!(-v)).unwrap_or(Value::Null);
                    fields.insert("battery_net_w".into(), negative);
                }
            }
            fields.insert("battery_source".into(), json!(BATTERY_SOURCE));
        }
    }
    if devices.is_empty() {
        let single_device;
        let fallback: &[Value] = match inventory.get("devices") {
            Some(Value::Array(rows)) => rows.as_slice(),
            Some(Value::Object(row)) => {
                single_device = Value::Object(row.clone());
                std::slice::from_ref(&single_device)
            }
            _ => &[],
        };
        devices = fallback
            .iter()
            .map(|device| {
                let name = device
                    .get("FriendlyName")
                    .filter(|name| is_truthy(Some(name)))
                    .and_then(Value::as_str)
                    .unwrap_or("USB 设备");
                json!({
                    "name": name,
                    "speed": "未知",
                    "vendor": "",
                    "source": "Get-PnpDevice（15 秒缓存）；设备存在不证明其使用 USB-C"
                })
            })
            .collect();
    }
    let all_errors = errors;
    json!({
        "platform": "Windows",
        "power": power,
        "ports": ports,
        "devices": devices,
        "fabric": [],
        "displays": displays,
        "errors": all_errors,
        "limitations": [
            ucsi_limitations(ucsi_state),
            "USB Hub 端口不等于机身 USB-C 接口；端口候选状态未知，设备存在不证明物理连接态，当前速率不等于线缆最大能力。",
            "电池净功率与适配器输入功率不同；未测线阻、柔软度或实际吞吐。"
        ]
    })
}

fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(v)) => *v,
        Some(Value::Number(n)) => n.as_f64().map(|v| v != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}
