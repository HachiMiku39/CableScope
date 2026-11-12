//! 手写 Win32 `extern "C"` 签名，1:1 镜像 `windows.py` 的 ctypes 声明。
//!
//! 不依赖 `windows` / `windows-sys` crate。`#[repr(C)]` 只用于自然对齐的公开
//! Win32 结构体；`usbioctl.h` 的 pack(1) 回复全部在 `decode.rs` 按字节偏移切片。

#![cfg(windows)]

use serde_json::{json, Value};
use std::ffi::{c_void, OsStr};
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use crate::decode::{digest, normal_hub_link, Hub, HubIo};

const DIGCF_PRESENT: u32 = 0x2;
const DIGCF_DEVICEINTERFACE: u32 = 0x10;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const OPEN_EXISTING: u32 = 3;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const INVALID_HANDLE_VALUE: isize = -1;

const GUID_USB_HUB: [u8; 16] = [
    0x88, 0x0e, 0x8a, 0xf1, 0x0c, 0xc3, 0xd0, 0x11, 0x88, 0x15, 0x00, 0xa0, 0xc9, 0x06, 0xbe, 0xd8,
];

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SystemPowerStatus {
    pub ac: u8,
    pub flags: u8,
    pub percent: u8,
    pub saver: u8,
    pub life: u32,
    pub full_life: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InterfaceData {
    pub size: u32,
    pub guid: [u8; 16],
    pub flags: u32,
    pub reserved: usize,
}

#[repr(C)]
pub struct DisplayDevice {
    pub cb: u32,
    pub name: [u16; 32],
    pub label: [u16; 128],
    pub flags: u32,
    pub device_id: [u16; 128],
    pub key: [u16; 128],
}

type Handle = *mut c_void;
type Bool = i32;

#[link(name = "kernel32")]
extern "C" {
    #[link_name = "GetSystemPowerStatus"]
    fn get_system_power_status(status: *mut SystemPowerStatus) -> Bool;
    #[link_name = "CreateFileW"]
    fn create_file_w(
        path: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> Handle;
    #[link_name = "CloseHandle"]
    fn close_handle(handle: Handle) -> Bool;
    #[link_name = "DeviceIoControl"]
    fn device_io_control(
        handle: Handle,
        code: u32,
        in_buffer: *const c_void,
        in_size: u32,
        out_buffer: *mut c_void,
        out_size: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> Bool;
    #[link_name = "GetLastError"]
    fn get_last_error() -> u32;
}

#[link(name = "setupapi")]
extern "C" {
    #[link_name = "SetupDiGetClassDevsW"]
    fn setup_di_get_class_devs_w(
        class_guid: *const c_void,
        enumerator: *const u16,
        parent: Handle,
        flags: u32,
    ) -> Handle;
    #[link_name = "SetupDiEnumDeviceInterfaces"]
    fn setup_di_enum_device_interfaces(
        devs: Handle,
        device_info: *mut c_void,
        guid: *const c_void,
        index: u32,
        interface: *mut InterfaceData,
    ) -> Bool;
    #[link_name = "SetupDiGetDeviceInterfaceDetailW"]
    fn setup_di_get_device_interface_detail_w(
        devs: Handle,
        interface: *mut InterfaceData,
        detail: *mut c_void,
        size: u32,
        required: *mut u32,
        device_info: *mut c_void,
    ) -> Bool;
    #[link_name = "SetupDiDestroyDeviceInfoList"]
    fn setup_di_destroy_device_info_list(devs: Handle) -> Bool;
}

#[link(name = "user32")]
extern "C" {
    #[link_name = "EnumDisplayDevicesW"]
    fn enum_display_devices_w(
        device: *const u16,
        index: u32,
        output: *mut DisplayDevice,
        flags: u32,
    ) -> Bool;
    #[link_name = "EnumDisplaySettingsW"]
    fn enum_display_settings_w(device: *const u16, mode: u32, output: *mut c_void) -> Bool;
}

fn win_error_text(code: u32) -> String {
    let error = io::Error::from_raw_os_error(code as i32);
    // Rust 的 Display 是 "<message> (os error N)"；ctypes.WinError 是
    // "[WinError N] <message>"。保留逐字一致的错误文案。
    let full = error.to_string();
    let suffix = format!(" (os error {code})");
    let message = full.strip_suffix(&suffix).unwrap_or(&full);
    format!("[WinError {code}] {message}")
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

fn from_wide(chars: &[u16]) -> String {
    let end = chars.iter().position(|c| *c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..end])
}

fn const_ptr<T>(value: &T) -> *const c_void {
    value as *const T as *const c_void
}

/// Python `GetSystemPowerStatus` + `ctypes.WinError` 文案。
pub fn system_power(errors: &mut Vec<String>) -> Value {
    let mut state: SystemPowerStatus = unsafe { zeroed() };
    // SAFETY: 结构体布局与 Win32 SYSTEM_POWER_STATUS 相同，status 为有效可写指针。
    let ok = unsafe { get_system_power_status(&mut state) } != 0;
    let error = if ok {
        None
    } else {
        let code = unsafe { get_last_error() };
        Some(format!("GetSystemPowerStatus: {}", win_error_text(code)))
    };
    crate::decode::power_status_from_fields(state.ac, state.flags, state.percent, ok, error, errors)
}

/// SetupDi 枚举、CreateFileW 打开 Hub 句柄并按哈希路径分组。
pub fn enumerate_hubs() -> Result<(Vec<Hub>, Vec<String>, Vec<OpenHub>), String> {
    let mut errors = Vec::new();
    // SAFETY: GUID 是 16 字节 GUID 结构，devs 句柄由 setupapi 返回并在销毁前有效。
    let devs = unsafe {
        setup_di_get_class_devs_w(
            const_ptr(&GUID_USB_HUB),
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if devs as isize == INVALID_HANDLE_VALUE {
        return Err(win_error_text(unsafe { get_last_error() }));
    }
    let mut hubs = Vec::new();
    let mut open: Vec<OpenHub> = Vec::new();
    let mut index: u32 = 0;
    loop {
        let mut info = InterfaceData {
            size: size_of::<InterfaceData>() as u32,
            guid: [0; 16],
            flags: 0,
            reserved: 0,
        };
        // SAFETY: info 是自然对齐的 InterfaceData；devs/GUID 有效。
        let found = unsafe {
            setup_di_enum_device_interfaces(
                devs,
                ptr::null_mut(),
                const_ptr(&GUID_USB_HUB),
                index,
                &mut info,
            )
        };
        if found == 0 {
            let code = unsafe { get_last_error() };
            if code != ERROR_NO_MORE_ITEMS {
                errors.push(format!("USB hub enumeration WinError {code}"));
            }
            break;
        }
        index += 1;
        let mut required: u32 = 0;
        // SAFETY: detail=null + size=0 只查询所需字节数；devs/info 有效。
        unsafe {
            setup_di_get_device_interface_detail_w(
                devs,
                &mut info,
                ptr::null_mut(),
                0,
                &mut required,
                ptr::null_mut(),
            );
        }
        if !(8..=65536).contains(&required) {
            errors.push("USB hub 接口长度无效".to_string());
            continue;
        }
        let mut detail = vec![0u8; required as usize];
        let cb_size: u32 = if size_of::<usize>() == 8 { 8 } else { 6 };
        detail[..4].copy_from_slice(&cb_size.to_le_bytes());
        // SAFETY: detail 容量等于 required，前 4 字节按 32/64 位写入 cbSize。
        let ok = unsafe {
            setup_di_get_device_interface_detail_w(
                devs,
                &mut info,
                detail.as_mut_ptr() as *mut c_void,
                required,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            errors.push(format!("USB hub detail WinError {}", unsafe {
                get_last_error()
            }));
            continue;
        }
        let units: Vec<u16> = detail[4..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let path = from_wide(&units);
        let wide_path = wide(&path);
        // FILE_ANY_ACCESS query IOCTLs; no port resets or descriptor requests.
        // SAFETY: wide_path 以 NUL 结尾；CreateFileW 只读打开，不写设备。
        let handle = unsafe {
            create_file_w(
                wide_path.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null_mut(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle as isize == INVALID_HANDLE_VALUE {
            errors.push(format!("Hub #{} 无法打开（WinError {}）", index, unsafe {
                get_last_error()
            }));
            continue;
        }
        let id = format!("hub-{}", open.len() + 1);
        hubs.push(Hub {
            id: id.clone(),
            hub_link: digest(&[&normal_hub_link(&path)]),
            index,
        });
        open.push(OpenHub { id, handle });
    }
    // SAFETY: 枚举句柄由本函数创建且只在此销毁一次。
    unsafe {
        setup_di_destroy_device_info_list(devs);
    }
    Ok((hubs, errors, open))
}

#[derive(Debug)]
pub struct OpenHub {
    pub id: String,
    pub handle: Handle,
}

/// `WinHubIo` 在 `hubs()` 中完成一次 SetupDi 枚举，`ioctl()` 按 id 找到句柄。
pub struct WinHubIo {
    handles: Vec<OpenHub>,
}

// SAFETY: HANDLE 是进程级内核对象；句柄只归 WinHubIo 所有，CloseHandle 也只
// 在该实例（或其 Drop）里发生，不会在线程间同时操作同一个 handle 值。
unsafe impl Send for WinHubIo {}

impl Default for WinHubIo {
    fn default() -> Self {
        Self::new()
    }
}

impl WinHubIo {
    pub fn new() -> Self {
        Self {
            handles: Vec::new(),
        }
    }

    fn handle(&self, id: &str) -> Result<Handle, String> {
        self.handles
            .iter()
            .find(|hub| hub.id == id)
            .map(|hub| hub.handle)
            .ok_or_else(|| "Hub 句柄未找到".to_string())
    }
}

impl HubIo for WinHubIo {
    fn hubs(&mut self) -> Result<(Vec<Hub>, Vec<String>), String> {
        self.close_all();
        let (hubs, errors, handles) = enumerate_hubs()?;
        self.handles = handles;
        Ok((hubs, errors))
    }

    fn ioctl(
        &mut self,
        handle: &str,
        code: u32,
        initial: &[u8],
        size: usize,
    ) -> Result<Vec<u8>, String> {
        let raw = self.handle(handle)?;
        let mut buffer = vec![0u8; size];
        let copied = initial.len().min(size);
        buffer[..copied].copy_from_slice(&initial[..copied]);
        let mut returned: u32 = 0;
        // SAFETY: buffer 同时作为输入和输出，长度均为 size；DeviceIoControl
        // 返回的字节数由 returned 给出，调用方只读 buffer[..returned]。
        let ok = unsafe {
            device_io_control(
                raw,
                code,
                buffer.as_ptr() as *const c_void,
                size as u32,
                buffer.as_mut_ptr() as *mut c_void,
                size as u32,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(win_error_text(unsafe { get_last_error() }));
        }
        let count = (returned as usize).min(size);
        Ok(buffer[..count].to_vec())
    }

    fn close(&mut self, handle: &str) {
        if let Some(position) = self.handles.iter().position(|hub| hub.id == handle) {
            let hub = self.handles.remove(position);
            // SAFETY: 句柄来自本实例的 CreateFileW，只在此关闭一次。
            unsafe {
                close_handle(hub.handle);
            }
        }
    }
}

impl WinHubIo {
    fn close_all(&mut self) {
        for hub in self.handles.drain(..) {
            // SAFETY: 同上，每个句柄只关闭一次。
            unsafe {
                close_handle(hub.handle);
            }
        }
    }
}

impl Drop for WinHubIo {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// `EnumDisplayDevicesW` / `EnumDisplaySettingsW`，DEVMODEW 固定 220 字节，
/// dmSize 写 68、PelsWidth/Height 在 172、DisplayFrequency 在 184。
///
/// 返回 `Err` 对应 Python `mode_comparison()` 抛出的异常；`sample()` 会像
/// `windows.py` 一样降级为「显示器: ...」错误条目并清空显示器列表。
pub fn displays() -> Result<Vec<Value>, String> {
    let mut result = Vec::new();
    for index in 0..64u32 {
        let mut device: DisplayDevice = unsafe { zeroed() };
        device.cb = size_of::<DisplayDevice>() as u32;
        // SAFETY: device 是自然对齐的 DISPLAY_DEVICEW，cb 已初始化。
        if unsafe { enum_display_devices_w(ptr::null(), index, &mut device, 0) } == 0 {
            break;
        }
        if device.flags & 1 == 0 {
            continue;
        }
        let name = from_wide(&device.name);
        let label = from_wide(&device.label);
        let wide_name = wide(&name);
        let current = match display_mode(&wide_name, 0xffff_ffff) {
            Some(mode) => mode,
            None => continue,
        };
        let mut display = json!({
            "name": format!("{name} · {label}"),
            "width": current.0,
            "height": current.1,
            "refresh_hz": current.2,
            "source": "EnumDisplaySettingsW 当前及可选模式（刷新率为整数）"
        });
        let mut modes = Vec::new();
        for mode_index in 0..4096u32 {
            let Some((w, h, rate)) = display_mode(&wide_name, mode_index) else {
                break;
            };
            let item = json!({"width": w, "height": h, "refresh_hz": rate});
            if !modes.contains(&item) {
                modes.push(item);
            }
        }
        let comparison = cablescope_analysis::mode_comparison(&display, &modes)
            .map_err(|error| error.to_string())?;
        if let Value::Object(fields) = &mut display {
            if let Value::Object(extra) = comparison {
                for (key, value) in extra {
                    fields.insert(key, value);
                }
            }
        }
        result.push(display);
    }
    Ok(result)
}

fn display_mode(name: &[u16], index: u32) -> Option<(u32, u32, Option<u32>)> {
    let mut mode = vec![0u8; 220];
    mode[68..70].copy_from_slice(&220u16.to_le_bytes());
    // SAFETY: 220 字节是 DEVMODEW 的固定长度，偏移与 Python struct.pack_into 相同。
    let ok =
        unsafe { enum_display_settings_w(name.as_ptr(), index, mode.as_mut_ptr() as *mut c_void) };
    if ok == 0 {
        return None;
    }
    let width = u32::from_le_bytes(mode[172..176].try_into().unwrap());
    let height = u32::from_le_bytes(mode[176..180].try_into().unwrap());
    let hz = u32::from_le_bytes(mode[184..188].try_into().unwrap());
    Some((width, height, (hz > 1).then_some(hz)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naturally_aligned_struct_layouts_match_ctypes() {
        assert_eq!(size_of::<SystemPowerStatus>(), 12);
        assert_eq!(
            size_of::<InterfaceData>(),
            if size_of::<usize>() == 8 { 32 } else { 28 }
        );
        assert_eq!(size_of::<DisplayDevice>(), 840);
        assert_eq!(GUID_USB_HUB.len(), 16);
        assert_eq!(GUID_USB_HUB[0..4], [0x88, 0x0e, 0x8a, 0xf1]);
    }

    #[test]
    fn win_error_text_wraps_system_message() {
        // 消息文本来自 FormatMessageW，随系统语言变化（中文机为
        // "不支持该请求。"，英文机为 "The request is not supported."），
        // 只固定 "[WinError N] " 包装与非空消息，不断言具体文案。
        let text = win_error_text(50);
        assert!(text.starts_with("[WinError 50] "), "{text}");
        assert!(text.len() > "[WinError 50] ".len(), "{text}");
        let access = win_error_text(5);
        assert!(access.starts_with("[WinError 5] "), "{access}");
        assert!(access.len() > "[WinError 5] ".len(), "{access}");
    }
}
