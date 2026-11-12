//! `cablescope/backends/smc.py` 的 macOS IOKit 调用序列。
//!
//! 本模块只在 macOS 编译：手写 extern "C" 声明并链接 IOKit framework，
//! `mach_task_self()` 来自 libSystem，不需要额外 link 属性。所有协议判定
//! 都在 [`crate::logic`] 中，FFI 只负责打开连接、按 9→5 两阶段读键、释放对象。
//! 不构造写命令，不修改 SMC，不重启控制器。

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::logic::{
    call_succeeded, fourcc, key_is_allowed, normal_uuid, type_name, validate_key_info_size,
    validate_request_command, CALL_SELECTOR, PARAMETERS_SIZE,
};
use crate::logic::{Command, KeyInfo, Parameters, SampleBuilder};

type IoObject = u32;
type IoConnect = u32;
type MachPort = u32;
type IOReturn = i32;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const i8) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: MachPort, matching: *mut c_void) -> IoObject;
    fn IOServiceOpen(
        service: IoObject,
        owning_task: MachPort,
        connection_type: u32,
        connection: *mut IoConnect,
    ) -> IOReturn;
    fn IOServiceClose(connection: IoConnect) -> IOReturn;
    fn IOObjectRelease(object: IoObject) -> IOReturn;
    fn IOConnectCallStructMethod(
        connection: IoConnect,
        selector: u32,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
        output_size: *mut usize,
    ) -> IOReturn;
}

extern "C" {
    fn mach_task_self() -> MachPort;
}

const APPLE_SMC: &[u8] = b"AppleSMC\0";
const NO_SERVICE: &str = "此系统未提供 AppleSMC";
const OPEN_OK: &str = "AppleSMC 只读连接可用";
const INITIAL: &str = "尚未读取";

/// Python `Reader` 的 Rust 对应物：连接、键信息缓存与状态由同一把可重入锁保护
/// （Rust 侧用 `Arc<Mutex<()>>`，锁的本地 Arc 副本不借用 `self`）。
/// `close()` 与 `Drop` 都会关闭连接，避免用户客户端句柄泄漏。
pub struct Reader {
    connection: IoConnect,
    status: String,
    info_cache: HashMap<String, KeyInfo>,
    lock: Arc<Mutex<()>>,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader {
    pub fn new() -> Self {
        Self {
            connection: 0,
            status: INITIAL.to_string(),
            info_cache: HashMap::new(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// `IOServiceMatching("AppleSMC")` → `IOServiceGetMatchingService(0, dict)`
    /// → `IOServiceOpen(service, mach_task_self(), 0, &connection)`。
    /// matching 字典是 Create-rule 对象，但 IOKit 的 GetMatchingService 会**消费**
    /// 该引用，因此不额外 CFRelease；service 在 finally 语义下用 IOObjectRelease 释放。
    fn open(&mut self) -> bool {
        if self.connection != 0 {
            return true;
        }
        unsafe {
            let matching = IOServiceMatching(APPLE_SMC.as_ptr().cast());
            if matching.is_null() {
                self.status = NO_SERVICE.to_string();
                return false;
            }
            let service = IOServiceGetMatchingService(0, matching);
            if service == 0 {
                self.status = NO_SERVICE.to_string();
                return false;
            }
            let mut connection: IoConnect = 0;
            let error = IOServiceOpen(service, mach_task_self(), 0, &mut connection);
            IOObjectRelease(service);
            if error != 0 {
                self.status = format!("AppleSMC 只读连接不可用（0x{:08x}）", error as u32);
                self.connection = 0;
                return false;
            }
            self.connection = connection;
            self.status = OPEN_OK.to_string();
            true
        }
    }

    /// 对应 `Reader.call(request)`：selector 2，输入 80 字节，输出缓冲区 80 字节。
    /// 只允许命令 5 / 9；三重成功条件由 `logic::call_succeeded` 判定。
    fn call(&self, request: &Parameters) -> Option<Parameters> {
        validate_request_command(request)?;
        let input = request.encode_bytes();
        let mut output = [0_u8; PARAMETERS_SIZE];
        let mut out_size: usize = PARAMETERS_SIZE;
        let error = unsafe {
            IOConnectCallStructMethod(
                self.connection,
                CALL_SELECTOR,
                input.as_ptr().cast(),
                input.len(),
                output.as_mut_ptr().cast(),
                &mut out_size,
            )
        };
        let response = Parameters::decode_bytes(&output)?;
        call_succeeded(error, out_size, &response).then_some(response)
    }

    /// 9（键信息）→ 5（读值）两阶段；键信息缓存 `(size, type)`，读值只取
    /// `data[..size]`，并按大端 FourCC 还原类型文本。
    fn read(&mut self, key: &str) -> Option<(Vec<u8>, String)> {
        if !key_is_allowed(key) {
            return None;
        }
        let code = fourcc(key)?;
        let info = match self.info_cache.get(key).copied() {
            Some(info) => info,
            None => {
                let mut request = Parameters::default();
                request.key = code;
                request.set_command(Command::KeyInfo);
                let response = self.call(&request)?;
                if !validate_key_info_size(response.info.size) {
                    return None;
                }
                let info = response.info;
                self.info_cache.insert(key.to_string(), info);
                info
            }
        };
        let mut request = Parameters::read_request(key, info)?;
        request.key = code;
        let response = self.call(&request)?;
        let size = (info.size as usize).min(PARAMETERS_SIZE - 48);
        Some((response.data[..size].to_vec(), type_name(info.data_type)))
    }

    /// 与 Python `Reader.sample()` 一致：采集系统输入、1–4 号端口输出通道、
    /// 协商合同和 PPBR 放电功率，返回统一 JSON 视图。
    pub fn sample(&mut self) -> Value {
        let lock = Arc::clone(&self.lock);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !self.open() {
            return json!({
                "status": self.status,
                "system": Value::Null,
                "channels": [],
                "contracts": []
            });
        }
        let mut builder = SampleBuilder::new(self.status.clone());
        for key in ["VD0R", "ID0R", "PDTR", "PPBR"] {
            if let Some((data, kind)) = self.read(key) {
                builder.insert_reading(key, data, kind);
            }
        }
        for index in 1..=4 {
            for suffix in ["UI", "JV", "JI", "PR", "MP", "MV", "MI", "DE"] {
                let key = format!("D{index}{suffix}");
                if let Some((data, kind)) = self.read(&key) {
                    builder.insert_reading(&key, data, kind);
                }
            }
        }
        for (key, info) in &self.info_cache {
            builder.insert_info(key, *info);
        }
        let mut sample = builder.sample();
        if let Value::Object(fields) = &mut sample {
            fields.insert("battery_discharge_w".into(), builder.battery_discharge_w());
        }
        sample
    }

    pub fn close(&mut self) {
        let lock = Arc::clone(&self.lock);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.connection != 0 {
            unsafe { IOServiceClose(self.connection) };
            self.connection = 0;
        }
    }

    pub fn status(&self) -> &str {
        &self.status
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.close();
    }
}

/// 独立入口：与 `Reader::sample()` 返回同一 schema。
pub fn sample() -> Value {
    Reader::new().sample()
}

/// UI 字节 → HPM/端口控制器 UUID（与 `logic::normal_uuid` 同一规则）。
pub fn controller_uuid(bytes: &[u8]) -> Option<String> {
    normal_uuid(&bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}
