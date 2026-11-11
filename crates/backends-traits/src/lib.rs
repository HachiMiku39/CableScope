//! 平台采集后端的公共契约。
//!
//! Python 版 `cablescope/backends/__init__.py` 的 `create()` 按 `sys.platform`
//! 分发到 macOS 或 Windows 的 `Backend`；所有实现都只暴露 `sample()`，部分
//! 平台另有可选的 `close()`。Rust 版用本 trait 固定这一接口，平台 crate 后续
//! 各自提供实现，并由 `create()` 风格的函数按目标平台分发。

use serde_json::Value;

/// 只读平台采集后端。
///
/// `sample()` 对应 Python 的 `Backend.sample()`：返回一份完整的 Snapshot，
/// 以 `serde_json::Value` 表示（`preserve_order` 保留 Python dict 的键序）。
/// 采集必须只读：不修改系统设置、不切换供电角色、不制造负载。
///
/// 读取失败由异常向上传播；Python 的 `Engine.run()` 用 `except Exception`
/// 在每轮采样边界兜底，异常不会终止后台线程。Rust `Engine` 同样用
/// `catch_unwind` 兜底 panic，失败时生成「读取失败」快照并在下一轮继续。
///
/// `close()` 对应 Python 后端的可选方法：`server.py` 用 `hasattr` 判断后才
/// 调用，因此这里提供默认空实现。要求 `Send` 是为了把后端移入 `Engine` 的
/// 后台 `std::thread`。
pub trait Backend: Send {
    /// 读取并返回一份未增强的 Snapshot。
    fn sample(&mut self) -> Value;

    /// 释放可选的平台资源；默认实现对应 Python 中没有 `close()` 的后端。
    fn close(&mut self) {}
}
