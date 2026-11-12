//! 平台采集后端的公共契约。
//!
//! 各平台 crate 提供自己的 `Backend` 实现，由 `create()` 风格的函数按目标
//! 平台分发（见 `src-tauri` 与 `cli`）。所有实现都只暴露 `sample()`，部分
//! 平台另有可选的 `close()`。

use serde_json::Value;

/// 只读平台采集后端。
///
/// `sample()` 返回一份完整的 Snapshot，以 `serde_json::Value` 表示
/// （`preserve_order` 保留 dict 的键序）。
/// 采集必须只读：不修改系统设置、不切换供电角色、不制造负载。
///
/// 读取失败向上传播；`Engine` 用 `catch_unwind` 在每轮采样边界兜底 panic，
/// 失败时生成「读取失败」快照并在下一轮继续。
///
/// `close()` 是可选方法：平台没有需要释放的资源时使用默认空实现。
/// 要求 `Send` 是为了把后端移入 `Engine` 的后台 `std::thread`。
pub trait Backend: Send {
    /// 读取并返回一份未增强的 Snapshot。
    fn sample(&mut self) -> Value;

    /// 释放可选的平台资源；默认空实现。
    fn close(&mut self) {}
}
