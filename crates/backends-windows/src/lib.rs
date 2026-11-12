//! Windows 10/11 只读采集后端的完整 Rust 移植。
//!
//! `decode.rs` 是不触硬件的 pack(1) 字节解码与快照组装，`ffi.rs` 手写 Win32
//! 签名，`cim.rs` 以单个 PowerShell 进程完成只读 CIM/PnP 清点。用户可见中文
//! 文案与 `cablescope/backends/windows.py` 逐字一致。

pub mod decode;

#[cfg(windows)]
pub mod cim;
#[cfg(windows)]
pub mod ffi;

#[cfg(test)]
mod tests;

#[cfg(windows)]
mod backend {
    use super::{cim, decode, ffi};
    use cablescope_backends_traits::Backend;
    use serde_json::Value;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    type PowerFn = fn(&mut Vec<String>) -> Value;
    type DisplaysFn = fn() -> Result<Vec<Value>, String>;

    pub struct WindowsBackend {
        inventory: cim::InventoryCache,
        hubs: Box<dyn decode::HubIo + Send>,
        power: PowerFn,
        displays: DisplaysFn,
    }

    impl Default for WindowsBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl WindowsBackend {
        pub fn new() -> Self {
            Self {
                inventory: cim::InventoryCache::new(),
                hubs: Box::new(ffi::WinHubIo::new()),
                power: ffi::system_power,
                displays: ffi::displays,
            }
        }

        /// 注入清点缓存与 HubIo；用于合成字节驱动 `sample()`。
        pub fn with_parts(
            inventory: cim::InventoryCache,
            hubs: Box<dyn decode::HubIo + Send>,
        ) -> Self {
            Self {
                inventory,
                hubs,
                power: ffi::system_power,
                displays: ffi::displays,
            }
        }

        /// Python 测试替换模块级 `system_power` / `displays` / `usb_devices`；
        /// Rust 版把前两者做成函数指针，Hub 由 `HubIo` 注入。
        pub fn with_overrides(
            inventory: cim::InventoryCache,
            hubs: Box<dyn decode::HubIo + Send>,
            power: PowerFn,
            displays: DisplaysFn,
        ) -> Self {
            Self {
                inventory,
                hubs,
                power,
                displays,
            }
        }
    }

    impl Backend for WindowsBackend {
        fn sample(&mut self) -> Value {
            let mut errors = Vec::new();
            let power = (self.power)(&mut errors);
            let inventory = self.inventory.get();
            if let Some(Value::Array(notes)) = inventory.get("errors") {
                for note in notes {
                    errors.push(match note {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    });
                }
            }
            let (devices, hub_errors, ports) = decode::usb_devices(self.hubs.as_mut());
            errors.extend(hub_errors);
            let displays = match catch_unwind(AssertUnwindSafe(|| (self.displays)())) {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => {
                    errors.push(format!("显示器: {error}"));
                    Vec::new()
                }
                Err(panic) => {
                    errors.push(format!("显示器: {}", panic_text(panic)));
                    Vec::new()
                }
            };
            decode::assemble_snapshot(power, devices, ports, displays, errors, &inventory)
        }
    }

    fn panic_text(panic: Box<dyn std::any::Any + Send>) -> String {
        if let Some(text) = panic.downcast_ref::<&'static str>() {
            (*text).to_string()
        } else if let Some(text) = panic.downcast_ref::<String>() {
            text.clone()
        } else {
            "Box<dyn Any>".to_string()
        }
    }
}

#[cfg(windows)]
pub use backend::WindowsBackend;

/// 按目标平台创建后端；非 Windows 由 `cli` 负责中文错误与退出码 1。
#[cfg(windows)]
pub fn create() -> Box<dyn cablescope_backends_traits::Backend> {
    Box::new(WindowsBackend::new())
}
