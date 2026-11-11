//! `cablescope/backends/smc.py` 的平台无关部分：ABI 布局、只读命令、解码与采样组装。
//!
//! 这里的函数不接触 IOKit：FFI 层把 80 字节输入/输出缓冲区交给本模块编解码，
//! 从而让所有协议规则都能在 Windows 上用合成字节验证。中文文案与 Python 版逐字一致。

use std::mem::{offset_of, size_of};

use cablescope_core::py::py_round;
use serde_json::{json, Map, Number, Value};

/// AppleSMC 用户客户端结构方法的 selector（`IOConnectCallStructMethod` 第二参数）。
pub const CALL_SELECTOR: u32 = 2;
/// `ctypes.sizeof(Parameters)`；输入长度和初始输出长度均为 80。
pub const PARAMETERS_SIZE: usize = 80;

/// 只读命令。写命令（例如风扇控制）没有对应变体，无法从公开 API 构造。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    ReadKey = 5,
    KeyInfo = 9,
}

impl Command {
    /// 对应 Python `if request.command not in (5, 9): raise ValueError`。
    /// 未知命令一律拒绝，不向驱动发送任何数据。
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            5 => Some(Self::ReadKey),
            9 => Some(Self::KeyInfo),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
    pub build: u8,
    pub reserved: u8,
    pub release: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct PLimit {
    pub version: u16,
    pub length: u16,
    pub cpu: u32,
    pub gpu: u32,
    pub memory: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct KeyInfo {
    pub size: u32,
    pub data_type: u32,
    pub attributes: u8,
}

/// 与 Python ctypes `Parameters` 逐字段对应的 80 字节结构。
///
/// `Version` 6 字节、`PLimit` 16 字节、`KeyInfo` 12 字节（尾部自然对齐补齐），
/// Rust `#[repr(C)]` 与 ctypes 的隐式填充一致；下方全部用编译期断言固定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Parameters {
    pub key: u32,
    pub version: Version,
    pub limits: PLimit,
    pub info: KeyInfo,
    pub result: u8,
    pub status: u8,
    pub command: u8,
    pub data32: u32,
    pub data: [u8; 32],
}

const _: () = assert!(size_of::<Parameters>() == PARAMETERS_SIZE);
const _: () = assert!(offset_of!(Parameters, key) == 0);
const _: () = assert!(offset_of!(Parameters, version) == 4);
const _: () = assert!(offset_of!(Parameters, limits) == 12);
const _: () = assert!(offset_of!(Parameters, info) == 28);
const _: () = assert!(offset_of!(Parameters, result) == 40);
const _: () = assert!(offset_of!(Parameters, status) == 41);
const _: () = assert!(offset_of!(Parameters, command) == 42);
const _: () = assert!(offset_of!(Parameters, data32) == 44);
const _: () = assert!(offset_of!(Parameters, data) == 48);
const _: () = assert!(size_of::<Version>() == 6);
const _: () = assert!(size_of::<PLimit>() == 16);
const _: () = assert!(size_of::<KeyInfo>() == 12);
const _: () = assert!(offset_of!(KeyInfo, size) == 0);
const _: () = assert!(offset_of!(KeyInfo, data_type) == 4);
const _: () = assert!(offset_of!(KeyInfo, attributes) == 8);

impl Default for Parameters {
    fn default() -> Self {
        Self {
            key: 0,
            version: Version {
                major: 0,
                minor: 0,
                build: 0,
                reserved: 0,
                release: 0,
            },
            limits: PLimit {
                version: 0,
                length: 0,
                cpu: 0,
                gpu: 0,
                memory: 0,
            },
            info: KeyInfo {
                size: 0,
                data_type: 0,
                attributes: 0,
            },
            result: 0,
            status: 0,
            command: 0,
            data32: 0,
            data: [0; 32],
        }
    }
}

impl Parameters {
    /// 构造键信息请求：key=FourCC，command=9，其余字段清零。
    pub fn key_info_request(key: &str, command: Command) -> Option<Self> {
        let mut request = Self::default();
        request.key = fourcc(key)?;
        request.command = command.as_u8();
        Some(request)
    }

    /// 设置命令字段；只有 [`Command`] 的两个只读变体可传入。
    pub fn set_command(&mut self, command: Command) {
        self.command = command.as_u8();
    }

    /// 构造读键请求：command=5，并把缓存的键信息 `(size, type)` 写回请求。
    pub fn read_request(key: &str, info: KeyInfo) -> Option<Self> {
        let mut request = Self::default();
        request.key = fourcc(key)?;
        request.command = Command::ReadKey.as_u8();
        request.info = info;
        Some(request)
    }

    /// 按 ctypes 结构体的本机字节序（macOS arm64/x86_64 均为小端）序列化。
    /// 直接用 `repr(C)` 的字节视图，保证与 `c.byref(request)` 完全一致。
    pub fn encode_bytes(&self) -> [u8; PARAMETERS_SIZE] {
        let mut bytes = [0_u8; PARAMETERS_SIZE];
        self.write_to(&mut bytes);
        bytes
    }

    fn write_to(&self, bytes: &mut [u8; PARAMETERS_SIZE]) {
        put(bytes, 0, self.key.to_ne_bytes());
        bytes[4] = self.version.major;
        bytes[5] = self.version.minor;
        bytes[6] = self.version.build;
        bytes[7] = self.version.reserved;
        put(bytes, 8, self.version.release.to_ne_bytes());
        put(bytes, 12, self.limits.version.to_ne_bytes());
        put(bytes, 14, self.limits.length.to_ne_bytes());
        put(bytes, 16, self.limits.cpu.to_ne_bytes());
        put(bytes, 20, self.limits.gpu.to_ne_bytes());
        put(bytes, 24, self.limits.memory.to_ne_bytes());
        put(bytes, 28, self.info.size.to_ne_bytes());
        put(bytes, 32, self.info.data_type.to_ne_bytes());
        bytes[36] = self.info.attributes;
        bytes[40] = self.result;
        bytes[41] = self.status;
        bytes[42] = self.command;
        put(bytes, 44, self.data32.to_ne_bytes());
        bytes[48..80].copy_from_slice(&self.data);
    }

    /// 解析驱动写回的 80 字节；长度不足或命令字段不是只读命令时返回 `None`。
    pub fn decode_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != PARAMETERS_SIZE {
            return None;
        }
        Command::from_u8(bytes[42])?;
        Some(Self {
            key: u32::from_ne_bytes(get_u32(bytes, 0)),
            version: Version {
                major: bytes[4],
                minor: bytes[5],
                build: bytes[6],
                reserved: bytes[7],
                release: u16::from_ne_bytes(get_u16(bytes, 8)),
            },
            limits: PLimit {
                version: u16::from_ne_bytes(get_u16(bytes, 12)),
                length: u16::from_ne_bytes(get_u16(bytes, 14)),
                cpu: u32::from_ne_bytes(get_u32(bytes, 16)),
                gpu: u32::from_ne_bytes(get_u32(bytes, 20)),
                memory: u32::from_ne_bytes(get_u32(bytes, 24)),
            },
            info: KeyInfo {
                size: u32::from_ne_bytes(get_u32(bytes, 28)),
                data_type: u32::from_ne_bytes(get_u32(bytes, 32)),
                attributes: bytes[36],
            },
            result: bytes[40],
            status: bytes[41],
            command: bytes[42],
            data32: u32::from_ne_bytes(get_u32(bytes, 44)),
            data: bytes[48..80].try_into().ok()?,
        })
    }

    /// 调用是否成功：IOKit 返回码 0、输出长度 80、响应 `result` 也为 0。
    /// `0x84`（键不存在等 SMC 侧错误）即使 IOKit 成功也必须失败。
    pub fn is_success(&self, io_error: i32, out_size: usize) -> bool {
        io_error == 0 && out_size == PARAMETERS_SIZE && self.result == 0
    }
}

fn put<const N: usize>(bytes: &mut [u8; PARAMETERS_SIZE], offset: usize, value: [u8; N]) {
    bytes[offset..offset + N].copy_from_slice(&value);
}

fn get_u32(bytes: &[u8], offset: usize) -> [u8; 4] {
    bytes[offset..offset + 4].try_into().unwrap()
}

fn get_u16(bytes: &[u8], offset: usize) -> [u8; 2] {
    bytes[offset..offset + 2].try_into().unwrap()
}

/// 与 Python `Reader.call()` 完全相同的三重成功条件，供 FFI 层与测试共用。
pub fn call_succeeded(io_error: i32, out_size: usize, response: &Parameters) -> bool {
    response.is_success(io_error, out_size)
}

/// 与 Python `Reader.call()` 开头 `if request.command not in (5, 9)` 相同的门禁。
/// FFI 层用它拒绝任何无法由 [`Command`] 表示的命令字节。
pub fn validate_request_command(request: &Parameters) -> Option<()> {
    Command::from_u8(request.command).map(|_| ())
}

/// 两个对外别名：FFI 层直接传 80 字节缓冲区，不必接触 `Parameters` 字段。
pub fn encode_bytes(parameters: &Parameters) -> [u8; PARAMETERS_SIZE] {
    parameters.encode_bytes()
}

pub fn decode_bytes(bytes: &[u8]) -> Option<Parameters> {
    Parameters::decode_bytes(bytes)
}

/// Python `int.from_bytes(key.encode("ascii"), "big")`：只接受 4 个 ASCII 字符。
pub fn fourcc(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if bytes.len() != 4 || !bytes.iter().all(u8::is_ascii) {
        return None;
    }
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// `re.fullmatch(r"VD0R|ID0R|PDTR|PPBR|D[1-4](UI|JV|JI|PR|MP|MV|MI|DE)", key)`：
/// 只允许系统输入、电池放电和 1–4 号端口的遥测/合同键。
pub fn key_is_allowed(key: &str) -> bool {
    if matches!(key, "VD0R" | "ID0R" | "PDTR" | "PPBR") {
        return true;
    }
    let bytes = key.as_bytes();
    bytes.len() == 4
        && bytes[0] == b'D'
        && (b'1'..=b'4').contains(&bytes[1])
        && matches!(
            &key[2..],
            "UI" | "JV" | "JI" | "PR" | "MP" | "MV" | "MI" | "DE"
        )
}

/// Python `not 1 <= response.info.size <= 32` 的反向：键信息读取的长度上限。
pub fn validate_key_info_size(size: u32) -> bool {
    (1..=32).contains(&size)
}

/// Python `struct.unpack("<f", data)`：SMC `flt` 是小端 IEEE-754 单精度；
/// 长度不是 4 字节或非有限值（NaN/Inf）一律返回 `None`。
pub fn finite_float(data: &[u8]) -> Option<f32> {
    if data.len() != 4 {
        return None;
    }
    let value = f32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    value.is_finite().then_some(value)
}

/// Python `int.from_bytes(result[0], "big")`：SMC 整型是大端、无符号，
/// 与 `flt` 的小端浮点明确区分。允许 ui8/ui16/ui32/ui64 四种类型。
pub fn unsigned_integer(data: &[u8], data_type: &str) -> Option<u64> {
    if !matches!(data_type, "ui8" | "ui16" | "ui32" | "ui64") || data.len() > 8 {
        return None;
    }
    let mut value = 0_u64;
    for byte in data {
        value = (value << 8) | u64::from(*byte);
    }
    Some(value)
}

/// Python `info[1].to_bytes(4, "big").decode("ascii","replace").strip()`：
/// FourCC 类型码按大端还原为文本，非法字节用 U+FFFD 替换并去掉首尾空白。
pub fn type_name(code: u32) -> String {
    let text: String = code
        .to_be_bytes()
        .iter()
        .map(|b| if b.is_ascii() { *b as char } else { '\u{fffd}' })
        .collect();
    text.trim_matches(char::from(0)).trim().to_string()
}

/// Python `normal_uuid`：去连字符、转小写、必须恰好 32 个 ASCII 十六进制字符，
/// 且不能全为 0（HPM 父控制器 UUID 与通道 UUID 的匹配键）。
pub fn normal_uuid(value: &str) -> Option<String> {
    let result = value.replace('-', "").to_lowercase();
    if result.len() != 32 || !result.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    if result.bytes().all(|b| b == b'0') {
        return None;
    }
    Some(result)
}

fn json_number(value: f64) -> Value {
    Value::Number(Number::from_f64(value).unwrap_or_else(|| Number::from(0)))
}

/// Python `round(value, 4)`：沿用 core 的银行家舍入兼容层。
pub fn round4(value: f64) -> Value {
    json_number(py_round(value, 4))
}

/// Python `round(value, 3)`，用于 mV/mA → V/A 的合同换算。
pub fn round3(value: f64) -> Value {
    json_number(py_round(value, 3))
}

/// `Reader.floating()` 在 FFI 层的数值入口：取有限 f32 并做 Python 舍入。
pub fn py_float_value(reading: &Option<(Vec<u8>, String)>, low: f64, high: f64) -> Option<Value> {
    floating(reading, low, high)
}

/// `Reader.unsigned()` 在 FFI 层的数值入口。
pub fn py_unsigned_value(reading: &Option<(Vec<u8>, String)>) -> Option<u64> {
    unsigned(reading)
}

/// `Reader.floating(key, low, high)` 的纯逻辑部分。
pub fn floating(reading: &Option<(Vec<u8>, String)>, low: f64, high: f64) -> Option<Value> {
    let (data, kind) = reading.as_ref()?;
    if kind != "flt" {
        return None;
    }
    let value = f32::from(finite_float(data)?) as f64;
    ((low..=high).contains(&value)).then(|| round4(value))
}

/// `Reader.unsigned(key)` 的纯逻辑部分。
pub fn unsigned(reading: &Option<(Vec<u8>, String)>) -> Option<u64> {
    let (data, kind) = reading.as_ref()?;
    unsigned_integer(data, kind)
}

/// SMC 键读取器的纯逻辑视图：FFI 提供 `(data, type)`，本结构负责缓存与 JSON 组装。
pub struct SampleBuilder {
    status: String,
    info_cache: Map<String, Value>,
    readings: Map<String, Value>,
}

impl SampleBuilder {
    pub fn new(status: impl Into<String>) -> Self {
        Self {
            status: status.into(),
            info_cache: Map::new(),
            readings: Map::new(),
        }
    }

    /// FFI 层每次成功读键后写入 `[Vec<u8>, type]`；键信息查得的 size/type 写缓存。
    pub fn insert_reading(&mut self, key: &str, data: Vec<u8>, data_type: String) {
        self.readings.insert(
            key.to_string(),
            json!([
                Value::Array(data.into_iter().map(|b| json!(b)).collect()),
                data_type
            ]),
        );
    }

    pub fn insert_info(&mut self, key: &str, info: KeyInfo) {
        self.info_cache
            .insert(key.to_string(), json!([info.size, info.data_type]));
    }

    /// FFI 成功分支需要把 PPBR 放电功率单独写回；与 `Reader.sample()` 的键保持一致。
    pub fn battery_discharge_w(&self) -> Value {
        self.floating("PPBR", 0.0, 200.0).unwrap_or(Value::Null)
    }

    pub fn info(&self, key: &str) -> Option<KeyInfo> {
        let row = self.info_cache.get(key)?.as_array()?;
        Some(KeyInfo {
            size: row.first()?.as_u64()? as u32,
            data_type: row.get(1)?.as_u64()? as u32,
            attributes: 0,
        })
    }

    fn reading(&self, key: &str) -> Option<(Vec<u8>, String)> {
        let row = self.readings.get(key)?.as_array()?;
        let data: Vec<u8> = row
            .first()?
            .as_array()?
            .iter()
            .filter_map(|b| b.as_u64().and_then(|n| u8::try_from(n).ok()))
            .collect();
        let kind = row.get(1)?.as_str()?.to_string();
        Some((data, kind))
    }

    fn floating(&self, key: &str, low: f64, high: f64) -> Option<Value> {
        floating(&self.reading(key), low, high)
    }

    fn unsigned(&self, key: &str) -> Option<u64> {
        unsigned(&self.reading(key))
    }

    /// `Reader.sample()` 的平台无关组装（成功分支）：系统输入 → 1–4 号端口
    /// 输出通道 → 协商合同。`battery_discharge_w` 由 FFI 成功分支单独写回，
    /// 因为 Python 的失败分支没有该键，成功分支还包含 PPBR 读数。
    pub fn sample(&self) -> Value {
        let voltage = self.floating("VD0R", 0.0, 60.0);
        let current = self.floating("ID0R", 0.0, 30.0);
        let dedicated = self.floating("PDTR", 0.0, 1000.0);
        let watts = match (&dedicated, &voltage, &current) {
            (Some(_), _, _) => dedicated.clone(),
            (None, Some(v), Some(a)) => Some(round4(
                v.as_f64().unwrap_or(0.0) * a.as_f64().unwrap_or(0.0),
            )),
            _ => None,
        };
        let system = json!({
            "voltage_v": voltage,
            "current_a": current,
            "watts": watts,
            "independent_power": dedicated.is_some(),
            "source": format!("AppleSMC VD0R / ID0R / {}", if dedicated.is_some() { "PDTR" } else { "V×I（计算值）" })
        });
        let mut channels = Vec::new();
        let mut contracts = Vec::new();
        for index in 1..=4 {
            let prefix = format!("D{index}");
            let identity = self.reading(&format!("{prefix}UI"));
            let uuid = identity
                .as_ref()
                .and_then(|(data, _)| normal_uuid(&hex(data)));
            let Some(uuid) = uuid else { continue };
            let v = self.floating(&format!("{prefix}JV"), 0.0, 60.0);
            let a = self.floating(&format!("{prefix}JI"), 0.0, 10.0);
            let present = self.unsigned(&format!("{prefix}PR"));
            let channel_watts = match (&v, &a) {
                (Some(v), Some(a)) => round4(v.as_f64().unwrap_or(0.0) * a.as_f64().unwrap_or(0.0)),
                _ => Value::Null,
            };
            channels.push(json!({
                "_uuid": uuid,
                "voltage_v": v,
                "current_a": a,
                "watts": channel_watts,
                "present": present.map(|n| n >= 1),
                "direction": "out",
                "source": format!("AppleSMC {prefix}JV / {prefix}JI（端口输出遥测）")
            }));
            let mw = self.unsigned(&format!("{prefix}MP"));
            let mv = self.unsigned(&format!("{prefix}MV"));
            let ma = self.unsigned(&format!("{prefix}MI"));
            let label = self.reading(&format!("{prefix}DE"));
            if let Some(mw) = mw.filter(|n| *n > 0 && *n <= 1_000_000) {
                contracts.push(json!({
                    "_uuid": uuid,
                    "watts": round3(mw as f64 / 1000.0),
                    "voltage_v": mv.filter(|n| *n > 0 && *n <= 60000)
                        .map(|n| round3(n as f64 / 1000.0)).unwrap_or(Value::Null),
                    "current_a": ma.filter(|n| *n > 0 && *n <= 10000)
                        .map(|n| round3(n as f64 / 1000.0)).unwrap_or(Value::Null),
                    "label": label.map(|(data, _)| nul_terminated_utf8(&data)).unwrap_or_default(),
                    "source": format!("AppleSMC {prefix}MP / MV / MI（协商上限，非实测功耗）")
                }));
            }
        }
        json!({
            "status": self.status,
            "system": system,
            "channels": Value::Array(channels),
            "contracts": Value::Array(contracts)
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Python `label[0].split(b"\0",1)[0].decode("utf-8","replace")`。
pub fn nul_terminated_utf8(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flt(value: f32) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    fn ui(value: u64, width: usize) -> Vec<u8> {
        value.to_be_bytes()[8 - width..].to_vec()
    }

    #[test]
    fn compile_time_abi_matches_python_ctypes_offsets() {
        // tests/test_features.py::SMCProtocolTests::test_driver_layout_matches_observed_eighty_byte_abi
        // 与 `py -3` 实测的完整偏移表。
        assert_eq!(size_of::<Parameters>(), 80);
        assert_eq!(offset_of!(Parameters, key), 0);
        assert_eq!(offset_of!(Parameters, version), 4);
        assert_eq!(offset_of!(Parameters, limits), 12);
        assert_eq!(offset_of!(Parameters, info), 28);
        assert_eq!(offset_of!(Parameters, result), 40);
        assert_eq!(offset_of!(Parameters, status), 41);
        assert_eq!(offset_of!(Parameters, command), 42);
        assert_eq!(offset_of!(Parameters, data32), 44);
        assert_eq!(offset_of!(Parameters, data), 48);
        assert_eq!(size_of::<Version>(), 6);
        assert_eq!(size_of::<PLimit>(), 16);
        assert_eq!(size_of::<KeyInfo>(), 12);
        assert_eq!(offset_of!(KeyInfo, size), 0);
        assert_eq!(offset_of!(KeyInfo, data_type), 4);
        assert_eq!(offset_of!(KeyInfo, attributes), 8);
    }

    #[test]
    fn parameters_round_trip_over_synthetic_eighty_bytes() {
        let mut original = Parameters::default();
        original.key = fourcc("VD0R").unwrap();
        original.version = Version {
            major: 2,
            minor: 3,
            build: 1,
            reserved: 7,
            release: 0x1234,
        };
        original.limits = PLimit {
            version: 9,
            length: 16,
            cpu: 0xdead_beef,
            gpu: 0x0102_0304,
            memory: 42,
        };
        original.info = KeyInfo {
            size: 4,
            data_type: 0x00_66_6c_74,
            attributes: 0x80,
        };
        original.result = 0;
        original.status = 3;
        original.command = Command::ReadKey.as_u8();
        original.data32 = 0xa5a5_a5a5;
        for (i, byte) in original.data.iter_mut().enumerate() {
            *byte = i as u8 + 1;
        }
        let bytes = encode_bytes(&original);
        assert_eq!(bytes.len(), 80);
        assert_eq!(&bytes[0..4], &fourcc("VD0R").unwrap().to_ne_bytes());
        assert_eq!(bytes[42], 5);
        assert_eq!(&bytes[48..52], &[1, 2, 3, 4]);
        assert_eq!(decode_bytes(&bytes), Some(original));
        assert_eq!(decode_bytes(&bytes[..79]), None);
    }

    #[test]
    fn commands_are_read_only_and_encode_as_five_and_nine() {
        assert_eq!(Command::from_u8(5), Some(Command::ReadKey));
        assert_eq!(Command::from_u8(9), Some(Command::KeyInfo));
        assert_eq!(Command::from_u8(6), None);
        assert_eq!(Command::from_u8(0), None);
        assert_eq!(Command::ReadKey.as_u8(), 5);
        assert_eq!(Command::KeyInfo.as_u8(), 9);
        let request = Parameters::key_info_request("VD0R", Command::KeyInfo).unwrap();
        assert_eq!(request.encode_bytes()[42], 9);
        assert_eq!(
            &request.encode_bytes()[0..4],
            &fourcc("VD0R").unwrap().to_ne_bytes()
        );
        let read = Parameters::read_request(
            "VD0R",
            KeyInfo {
                size: 4,
                data_type: 0x00_66_6c_74,
                attributes: 0,
            },
        )
        .unwrap();
        let bytes = read.encode_bytes();
        assert_eq!(bytes[42], 5);
        assert_eq!(&bytes[28..32], &4_u32.to_ne_bytes());
        assert_eq!(&bytes[32..36], &0x00_66_6c_74_u32.to_ne_bytes());
        // 写命令的 80 字节响应无法被解析为合法请求/响应。
        let mut illegal = bytes;
        illegal[42] = 6;
        assert_eq!(decode_bytes(&illegal), None);
        assert_eq!(CALL_SELECTOR, 2);
    }

    #[test]
    fn key_allowlist_and_info_size_bounds() {
        for key in [
            "VD0R", "ID0R", "PDTR", "PPBR", "D1UI", "D4DE", "D2MP", "D3MV", "D1MI", "D4PR",
        ] {
            assert!(key_is_allowed(key), "{key}");
        }
        for key in [
            "F0Ac", "D0UI", "D5UI", "D1XX", "VDOR", "vd0r", "VD0R ", "D1U", "D1UII", "",
        ] {
            assert!(!key_is_allowed(key), "{key}");
        }
        assert_eq!(fourcc("VD0R"), Some(0x5644_3052));
        assert_eq!(fourcc("VD0"), None);
        assert_eq!(fourcc("ＶＤ０Ｒ"), None);
        assert!(validate_key_info_size(1));
        assert!(validate_key_info_size(32));
        assert!(!validate_key_info_size(0));
        assert!(!validate_key_info_size(33));
        assert!(!validate_key_info_size(u32::MAX));
    }

    #[test]
    fn smc_result_0x84_fails_even_when_iokit_succeeds() {
        let mut response = Parameters::default();
        response.command = Command::KeyInfo.as_u8();
        response.result = 0x84;
        assert!(!call_succeeded(0, 80, &response));
        response.result = 0;
        assert!(call_succeeded(0, 80, &response));
        assert!(!call_succeeded(0, 79, &response));
        assert!(!call_succeeded(1, 80, &response));
    }

    #[test]
    fn float_is_little_endian_integers_are_big_endian() {
        assert_eq!(finite_float(&28.2_f32.to_le_bytes()), Some(28.2_f32));
        assert_eq!(finite_float(&f32::NAN.to_le_bytes()), None);
        assert_eq!(finite_float(&f32::INFINITY.to_le_bytes()), None);
        assert_eq!(finite_float(&[1, 2, 3]), None);
        // 0x00004e20 大端 = 20000，小端解释会得到 0x204e0000。
        let bytes = 20_000_u32.to_be_bytes();
        assert_eq!(unsigned_integer(&bytes, "ui32"), Some(20_000));
        assert_ne!(u32::from_le_bytes(bytes), 20_000);
        assert_eq!(unsigned_integer(&bytes, "flt"), None);
        assert_eq!(unsigned_integer(&[0xff], "ui8"), Some(255));
        assert_eq!(unsigned_integer(&[0; 9], "ui64"), None);
        assert_eq!(type_name(0x00_66_6c_74), "flt");
        assert_eq!(type_name(0x75_69_33_32), "ui32");
        assert_eq!(type_name(0xff_66_6c_74), "\u{fffd}flt");
        let flt_reading = Some((flt(28.2), "flt".to_string()));
        assert_eq!(floating(&flt_reading, 0.0, 60.0), Some(json!(28.2)));
        assert_eq!(floating(&flt_reading, 0.0, 20.0), None);
        assert_eq!(
            floating(&Some((ui(20_000, 4), "ui32".into())), 0.0, 60.0),
            None
        );
        assert_eq!(
            unsigned(&Some((ui(20_000, 4), "ui32".into()))),
            Some(20_000)
        );
    }

    #[test]
    fn uuid_normalization_rejects_non_hex_fullwidth_and_all_zero() {
        assert_eq!(normal_uuid(&"AA".repeat(16)), Some("aa".repeat(16)));
        assert_eq!(
            normal_uuid("00112233-4455-6677-8899-AABBCCDDEEFF"),
            Some("00112233445566778899aabbccddeeff".to_string())
        );
        assert_eq!(normal_uuid(&"0".repeat(32)), None);
        assert_eq!(normal_uuid(&"Ａ".repeat(32)), None);
        assert_eq!(normal_uuid(&"g".repeat(32)), None);
        assert_eq!(normal_uuid(""), None);
        assert_eq!(normal_uuid(&"a".repeat(31)), None);
    }

    #[test]
    fn sample_builder_matches_python_system_channels_and_contracts() {
        let mut builder = SampleBuilder::new("AppleSMC 只读连接可用");
        builder.insert_info(
            "VD0R",
            KeyInfo {
                size: 4,
                data_type: 0x00_66_6c_74,
                attributes: 0,
            },
        );
        assert_eq!(builder.info("VD0R").unwrap().size, 4);
        builder.insert_reading("VD0R", flt(20.0), "flt".into());
        builder.insert_reading("ID0R", flt(3.0), "flt".into());
        builder.insert_reading("PPBR", flt(12.5), "flt".into());
        let uuid = "00112233445566778899aabbccddeeff";
        builder.insert_reading("D1UI", hex_to_bytes(uuid), "ui8".into());
        builder.insert_reading("D1JV", flt(5.0), "flt".into());
        builder.insert_reading("D1JI", flt(1.5), "flt".into());
        builder.insert_reading("D1PR", ui(1, 1), "ui8".into());
        builder.insert_reading("D1MP", ui(100_000, 4), "ui32".into());
        builder.insert_reading("D1MV", ui(20_000, 4), "ui32".into());
        builder.insert_reading("D1MI", ui(5_000, 4), "ui32".into());
        let label = b"USB-C\0tail".to_vec();
        builder.insert_reading("D1DE", label, "ch8*".into());
        let sample = builder.sample();
        assert_eq!(sample["status"], json!("AppleSMC 只读连接可用"));
        assert_eq!(sample["system"]["watts"], json!(60.0));
        assert_eq!(sample["system"]["voltage_v"], json!(20.0));
        assert_eq!(sample["system"]["independent_power"], json!(false));
        assert_eq!(
            sample["system"]["source"],
            json!("AppleSMC VD0R / ID0R / V×I（计算值）")
        );
        assert_eq!(sample["channels"].as_array().unwrap().len(), 1);
        assert_eq!(sample["channels"][0]["_uuid"], json!(uuid));
        assert_eq!(sample["channels"][0]["watts"], json!(7.5));
        assert_eq!(sample["channels"][0]["present"], json!(true));
        assert_eq!(sample["channels"][0]["direction"], json!("out"));
        assert_eq!(sample["contracts"][0]["watts"], json!(100.0));
        assert_eq!(sample["contracts"][0]["voltage_v"], json!(20.0));
        assert_eq!(sample["contracts"][0]["current_a"], json!(5.0));
        assert_eq!(sample["contracts"][0]["label"], json!("USB-C"));
        assert_eq!(builder.battery_discharge_w(), json!(12.5));
    }

    #[test]
    fn dedicated_pdtr_overrides_computed_system_power_and_invalid_contract_is_dropped() {
        let mut builder = SampleBuilder::new("可用");
        builder.insert_reading("VD0R", flt(20.0), "flt".into());
        builder.insert_reading("ID0R", flt(1.0), "flt".into());
        builder.insert_reading("PDTR", flt(94.5), "flt".into());
        let sample = builder.sample();
        assert_eq!(sample["system"]["watts"], json!(94.5));
        assert_eq!(sample["system"]["independent_power"], json!(true));
        assert_eq!(
            sample["system"]["source"],
            json!("AppleSMC VD0R / ID0R / PDTR")
        );
        let mut builder = SampleBuilder::new("可用");
        builder.insert_reading("D2UI", hex_to_bytes(&"ab".repeat(16)), "ui8".into());
        // mW 为 0 或 > 1000000 时不生成合同；MV/MI 越界保持 null。
        builder.insert_reading("D2MP", ui(0, 4), "ui32".into());
        assert!(builder.sample()["contracts"].as_array().unwrap().is_empty());
        let mut builder = SampleBuilder::new("可用");
        builder.insert_reading("D2UI", hex_to_bytes(&"ab".repeat(16)), "ui8".into());
        builder.insert_reading("D2MP", ui(60_000, 4), "ui32".into());
        builder.insert_reading("D2MV", ui(60_001, 4), "ui32".into());
        builder.insert_reading("D2MI", ui(10_001, 4), "ui32".into());
        let sample = builder.sample();
        assert_eq!(sample["contracts"][0]["watts"], json!(60.0));
        assert_eq!(sample["contracts"][0]["voltage_v"], Value::Null);
        assert_eq!(sample["contracts"][0]["current_a"], Value::Null);
        assert_eq!(sample["contracts"][0]["label"], json!(""));
    }

    fn hex_to_bytes(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }
}
