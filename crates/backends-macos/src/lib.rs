//! macOS 后端的平台无关映射层。
//!
//! 对应 `cablescope/backends/macos.py`：plist 字节 → `serde_json::Value`、
//! IORegistry 合成属性字典 → 快照（power/ports/devices/displays/display_links/fabric），
//! 以及 SMC 通道/合同匹配、DP 唯一父端口、输入功率分配、显示模式比较、
//! Thunderbolt 树展开等规则。所有函数都可在 Windows 上编译并用合成数据测试；
//! IOKit/CoreGraphics 调用与子进程放在 `ffi` 模块，仅在 macOS 编译。

pub mod logic;

#[cfg(target_os = "macos")]
pub mod ffi;

pub use logic::{
    display_modes_from_queries, fabric_from_profiler, usb_devices_from_properties,
    uuid_from_properties, BackendLogic, DisplayQuery, RegistryData,
};

#[cfg(target_os = "macos")]
pub use ffi::Backend;
