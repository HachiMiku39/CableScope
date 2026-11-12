//! `cablescope/backends/macos.py` 的 macOS 专属 FFI 与子进程层。
//!
//! 只在 `target_os = "macos"` 编译。手写 extern "C" 声明并链接 IOKit /
//! CoreFoundation / CoreGraphics framework；CF Create-rule 对象全部用 RAII
//! 包装在 Drop 中 CFRelease，IOKit 对象用 IOObjectRelease。
//! `system_profiler` 用 `Command` + `try_wait()` 轮询 12 秒，超时 kill。

use std::ffi::{c_char, c_void, CString};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cablescope_backends_traits::Backend as BackendTrait;
use cablescope_smc::ffi::Reader as SmcReader;
use plist::Value as PlistValue;
use serde_json::{json, Value};

use crate::logic::{
    display_modes_from_queries, fabric_from_profiler, is_hpm_device_class, plist_bytes_to_json,
    plist_to_json, uuid_from_properties, BackendLogic, DisplayQuery, RegistryData,
    PLIST_BINARY_FORMAT_V1_0,
};

type IoObject = u32;
type IoIterator = u32;
type IOReturn = i32;
type CFTypeRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFPropertyListFormat = u64;
type CFIndex = isize;
type CGDisplayID = u32;
type CGError = i32;
type CGDisplayModeRef = *const c_void;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(
        main_port: u32,
        matching: *mut c_void,
        iterator: *mut IoIterator,
    ) -> IOReturn;
    fn IOIteratorNext(iterator: IoIterator) -> IoObject;
    fn IORegistryEntryCreateCFProperties(
        entry: IoObject,
        properties: *mut CFTypeRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> IOReturn;
    fn IORegistryEntryGetRegistryEntryID(entry: IoObject, id: *mut u64) -> IOReturn;
    fn IORegistryEntryGetParentEntry(
        entry: IoObject,
        plane: *const c_char,
        parent: *mut IoObject,
    ) -> IOReturn;
    fn IOObjectGetClass(entry: IoObject, class_name: *mut c_char) -> IOReturn;
    fn IOObjectRelease(object: IoObject) -> IOReturn;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFPropertyListCreateData(
        allocator: CFAllocatorRef,
        property_list: CFTypeRef,
        format: CFPropertyListFormat,
        options: u64,
        error: *mut c_void,
    ) -> CFTypeRef;
    fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
    fn CFRelease(object: CFTypeRef);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetActiveDisplayList(
        max_displays: u32,
        active_displays: *mut CGDisplayID,
        display_count: *mut u32,
    ) -> CGError;
    fn CGDisplayCopyDisplayMode(display: CGDisplayID) -> CGDisplayModeRef;
    fn CGDisplayModeRelease(mode: CGDisplayModeRef);
    fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetRefreshRate(mode: CGDisplayModeRef) -> f64;
    fn CGDisplayCopyAllDisplayModes(display: CGDisplayID, options: CFTypeRef) -> CFTypeRef;
    fn CGDisplayIsBuiltin(display: CGDisplayID) -> u32;
}

const IOSERVICE_PLANE: &[u8] = b"IOService\0";
const THUNDERBOLT_TIMEOUT: Duration = Duration::from_secs(12);
const FABRIC_CACHE: Duration = Duration::from_secs(15);
const CLASS_BUFFER: usize = 128;
const MAX_DISPLAYS: u32 = 64;
const MAX_MODES: isize = 4096;

/// CF Create-rule 对象：非空时在 Drop 中 `CFRelease`。
struct CfObject(CFTypeRef);

impl CfObject {
    fn new(raw: CFTypeRef) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }

    fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for CfObject {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

/// IOKit 对象：非空时在 Drop 中 `IOObjectRelease`。
struct IoOwned(IoObject);

impl IoOwned {
    fn new(raw: IoObject) -> Option<Self> {
        (raw != 0).then_some(Self(raw))
    }

    fn get(&self) -> IoObject {
        self.0
    }
}

impl Drop for IoOwned {
    fn drop(&mut self) {
        unsafe { IOObjectRelease(self.0) };
    }
}

struct CgMode(CGDisplayModeRef);

impl CgMode {
    fn new(raw: CGDisplayModeRef) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }

    fn get(&self) -> CGDisplayModeRef {
        self.0
    }
}

impl Drop for CgMode {
    fn drop(&mut self) {
        unsafe { CGDisplayModeRelease(self.0) };
    }
}

/// 把 IORegistry 属性字典序列化为 binary plist v1_0(200) 后转成 JSON Value。
/// `CFDataGetBytePtr` / `CFDataGetLength` 的字节只在 CFData 存活期间读取。
fn properties_to_json(entry: IoObject) -> Option<Value> {
    unsafe {
        let mut raw: CFTypeRef = std::ptr::null();
        if IORegistryEntryCreateCFProperties(entry, &mut raw, std::ptr::null(), 0) != 0 {
            return None;
        }
        let properties = CfObject::new(raw)?;
        let data = CfObject::new(CFPropertyListCreateData(
            std::ptr::null(),
            properties.as_ptr(),
            PLIST_BINARY_FORMAT_V1_0,
            0,
            std::ptr::null_mut(),
        ))?;
        let length = CFDataGetLength(data.as_ptr());
        if length <= 0 {
            return None;
        }
        let pointer = CFDataGetBytePtr(data.as_ptr());
        if pointer.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(pointer, length as usize);
        plist_bytes_to_json(bytes).ok()
    }
}

/// `IORegistryEntryGetParentEntry(... "IOService" ...)` 最多向上 8 层，
/// 遇到 `AppleHPMDevice*` 类时读取其 `UUID` 属性并规范化。
fn controller_uuid(entry: IoObject) -> Option<String> {
    let mut owned: Vec<IoOwned> = Vec::new();
    let mut current = entry;
    for _ in 0..8 {
        unsafe {
            let mut parent: IoObject = 0;
            if IORegistryEntryGetParentEntry(current, IOSERVICE_PLANE.as_ptr().cast(), &mut parent)
                != 0
            {
                break;
            }
            let Some(handle) = IoOwned::new(parent) else {
                break;
            };
            current = handle.get();
            let mut name = [0_i8; CLASS_BUFFER];
            IOObjectGetClass(current, name.as_mut_ptr());
            let class = name
                .iter()
                .take_while(|b| **b != 0)
                .map(|b| *b as u8)
                .collect::<Vec<_>>();
            if !is_hpm_device_class(&class) {
                owned.push(handle);
                continue;
            }
            if let Some(properties) = properties_to_json(current) {
                if let Some(uuid) = uuid_from_properties(&properties) {
                    return Some(uuid);
                }
            }
            owned.push(handle);
        }
    }
    drop(owned);
    None
}

/// `IOServiceGetMatchingServices` → 迭代 → 属性字典 → `_registry_id` 与
/// `_controller_uuid` 注入；matching 字典按 Create-rule 由 IOKit 消费。
fn services(name: &str) -> Result<Vec<Value>, String> {
    let c_name = CString::new(name).map_err(|e| e.to_string())?;
    unsafe {
        let matching = IOServiceMatching(c_name.as_ptr());
        if matching.is_null() {
            return Err(format!("IOKit matching {name}: 匹配字典创建失败"));
        }
        let mut iterator: IoIterator = 0;
        let error = IOServiceGetMatchingServices(0, matching, &mut iterator);
        if error != 0 {
            return Err(format!("IOKit matching {name}: 0x{:x}", error as u32));
        }
        let iterator = IoOwned::new(iterator);
        let mut result = Vec::new();
        loop {
            let object = IOIteratorNext(iterator.as_ref().map(IoOwned::get).unwrap_or(0));
            if object == 0 {
                break;
            }
            let entry = IoOwned::new(object);
            let Some(entry) = entry else { continue };
            if let Some(mut properties) = properties_to_json(entry.get()) {
                let mut id: u64 = 0;
                IORegistryEntryGetRegistryEntryID(entry.get(), &mut id);
                if let Value::Object(fields) = &mut properties {
                    fields.insert("_registry_id".into(), json!(id.to_string()));
                    if name == "IOPort"
                        || name.starts_with("AppleHPMInterface")
                        || name.starts_with("AppleTCController")
                    {
                        fields.insert(
                            "_controller_uuid".into(),
                            controller_uuid(entry.get())
                                .map(Value::String)
                                .unwrap_or(Value::Null),
                        );
                    }
                }
                result.push(properties);
            }
        }
        Ok(result)
    }
}

fn mode_fields(mode: CGDisplayModeRef) -> Value {
    unsafe {
        json!({
            "width": CGDisplayModeGetPixelWidth(mode),
            "height": CGDisplayModeGetPixelHeight(mode),
            "refresh_hz": CGDisplayModeGetRefreshRate(mode)
        })
    }
}

/// CoreGraphics 当前/全部显示模式查询；失败返回错误文案（Python 的
/// `display_modes()` 异常由 `Backend.sample()` 转成 `errors[]`）。
fn display_queries() -> Result<Vec<DisplayQuery>, String> {
    unsafe {
        let mut ids = [0_u32; MAX_DISPLAYS as usize];
        let mut count: u32 = 0;
        let error = CGGetActiveDisplayList(MAX_DISPLAYS, ids.as_mut_ptr(), &mut count);
        if error != 0 {
            return Err(format!("CoreGraphics display query failed: {error}"));
        }
        let mut queries = Vec::new();
        for identifier in ids.iter().take(count as usize).copied() {
            let Some(mode) = CgMode::new(CGDisplayCopyDisplayMode(identifier)) else {
                continue;
            };
            let mut modes = Vec::new();
            if let Some(array) =
                CfObject::new(CGDisplayCopyAllDisplayModes(identifier, std::ptr::null()))
            {
                let total = CFArrayGetCount(array.as_ptr()).min(MAX_MODES);
                for index in 0..total {
                    let item = CFArrayGetValueAtIndex(array.as_ptr(), index);
                    if !item.is_null() {
                        modes.push(mode_fields(item));
                    }
                }
            }
            queries.push(DisplayQuery {
                identifier,
                builtin: CGDisplayIsBuiltin(identifier) != 0,
                current: mode_fields(mode.get()),
                modes,
            });
        }
        Ok(queries)
    }
}

/// `subprocess.run(["/usr/sbin/system_profiler", "-json", "SPThunderboltDataType"],
/// timeout=12)` 的无 async 等价实现：`try_wait()` 每 50ms 轮询，超时 kill。
fn thunderbolt_json() -> Result<Value, String> {
    let mut child = Command::new("/usr/sbin/system_profiler")
        .args(["-json", "SPThunderboltDataType"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None => {
                if started.elapsed() >= THUNDERBOLT_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("system_profiler SPThunderboltDataType 超时".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    if !status.success() {
        return Err(format!(
            "system_profiler 退出码 {}",
            status.code().unwrap_or(-1)
        ));
    }
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout)
            .map_err(|e| e.to_string())?;
    }
    let value: Value = serde_json::from_str(&stdout).map_err(|e| e.to_string())?;
    Ok(value)
}

fn thunderbolt_fabric() -> (Vec<Value>, Option<String>) {
    match thunderbolt_json() {
        Ok(value) => {
            let nodes = value
                .get("SPThunderboltDataType")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            (fabric_from_profiler(&nodes), None)
        }
        Err(error) => (Vec::new(), Some(format!("Thunderbolt: {error}"))),
    }
}

/// Python `Backend` 的 Rust 实现：FFI 采集 → [`crate::logic::BackendLogic`] 组装。
pub struct Backend {
    smc: SmcReader,
    logic: BackendLogic,
    fabric_time: Option<Instant>,
}

impl Default for Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend {
    pub fn new() -> Self {
        Self {
            smc: SmcReader::new(),
            logic: BackendLogic::new(),
            fabric_time: None,
        }
    }

    fn read(&self, name: &str) -> (Vec<Value>, Option<String>) {
        match services(name) {
            Ok(rows) => (rows, None),
            Err(error) => (Vec::new(), Some(format!("{name}: {error}"))),
        }
    }

    fn collect(&self, telemetry: &Value) -> (RegistryData, Vec<Value>) {
        let mut data = RegistryData::default();
        let mut display_error = None;
        let (batteries, error) = self.read("AppleSmartBattery");
        data.batteries_present = !batteries.is_empty();
        data.batteries = batteries;
        if let Some(error) = error {
            data.errors.push(error);
        }
        let (identities, error) = self.read("IOPortTransportComponentCCUSBPDSOPp");
        data.identities = identities;
        if let Some(error) = error {
            data.errors.push(error);
        }
        let (ports, error) = self.read("IOPort");
        data.ports = ports;
        if let Some(error) = error {
            data.errors.push(error);
        }
        if data.ports.is_empty() {
            for class in [
                "AppleHPMInterfaceType10",
                "AppleHPMInterfaceType11",
                "AppleHPMInterfaceType12",
                "AppleHPMInterfaceType18",
                "AppleTCControllerType10",
                "AppleTCControllerType11",
            ] {
                let (rows, error) = self.read(class);
                if let Some(error) = error {
                    data.errors.push(error);
                }
                data.port_class_results.push((class.to_string(), rows));
            }
        }
        let (links, error) = self.read("IOPortTransportStateDisplayPort");
        data.displayport = links;
        if let Some(error) = error {
            data.errors.push(error);
        }
        let (devices, error) = self.read("IOUSBHostDevice");
        data.usb_devices = devices;
        if let Some(error) = error {
            data.errors.push(error);
        }
        let displays = match display_queries() {
            Ok(queries) => display_modes_from_queries(&queries),
            Err(error) => {
                display_error = Some(format!("显示器: {error}"));
                Vec::new()
            }
        };
        if let Some(error) = display_error {
            data.errors.push(error);
        }
        let _ = telemetry;
        (data, displays)
    }
}

impl BackendTrait for Backend {
    fn sample(&mut self) -> Value {
        let telemetry = self.smc.sample();
        let (data, displays) = self.collect(&telemetry);
        let now = Instant::now();
        let refresh = self
            .fabric_time
            .map(|last| now.duration_since(last) >= FABRIC_CACHE)
            .unwrap_or(true);
        if refresh {
            self.fabric_time = Some(now);
            let (fabric, error) = thunderbolt_fabric();
            self.logic.set_fabric(fabric, error);
        }
        self.logic.sample(&data, &telemetry, &displays)
    }

    fn close(&mut self) {
        self.smc.close();
    }
}

/// 把 FFI 读到的 plist `Value` 转成 JSON 的公开入口（与纯逻辑共用转换器）。
pub fn plist_value_to_json(value: PlistValue) -> Value {
    plist_to_json(value)
}
