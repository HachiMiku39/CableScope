//! AppleSMC 只读电源键的完整 Rust 移植。
//!
//! 对应 `cablescope/backends/smc.py`：平台无关的 80 字节 `Parameters` 编解码、
//! 命令与键白名单、flt/整型解码、UUID 规范化、通道/合同/系统功率合成都放在
//! [`logic`]（任何平台都能编译与单元测试）；IOKit 调用序列放在 `ffi` 模块，
//! 仅在 `target_os = "macos"` 编译。只开放读键命令 5 与键信息命令 9。

pub mod logic;

#[cfg(target_os = "macos")]
pub mod ffi;

pub use logic::{
    call_succeeded, decode_bytes, encode_bytes, finite_float, fourcc, key_is_allowed, normal_uuid,
    nul_terminated_utf8, validate_key_info_size, validate_request_command, Command, KeyInfo,
    PLimit, Parameters, SampleBuilder, Version, CALL_SELECTOR, PARAMETERS_SIZE,
};

#[cfg(target_os = "macos")]
pub use ffi::Reader;
