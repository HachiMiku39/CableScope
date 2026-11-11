免费开源的 Tauri 桌面客户端，内置 Rust 采集引擎，不用安装任何运行环境。

| 平台 | 文件 |
|---|---|
| Windows x64 | windows-x64.zip，解压后双击 CableScope.exe |
| Windows ARM64 | windows-arm64.zip，原生 ARM64，解压即用 |
| Mac Apple Silicon | macos-arm64.zip，解压后把 CableScope.app 拖入应用程序 |

Mac 要求 macOS 15+；Windows 要求 Windows 10/11 x64 或 Windows 11 ARM64，并依赖系统自带的 WebView2 运行时（Windows 11 预装，个别 Windows 10 需另行安装）。三个版本均为便携压缩包，通过原生云编译、打包产物解压启动、网页渲染、采集、保存和报告检查后发布，不写注册表、无需安装。

设备数据只保存在本机应用数据目录；关闭客户端即停止采集。没有账户、付费授权或订阅，也不存在网页版入口。

功能包括 Mac SMC 功率与端口曲线、协商诊断、引脚路由、MagSafe 结构示意、两平台显示模式比较、线缆记录和 JSON/CSV 导出。Windows 深层 PD/E-Marker、完整 DSC、逐针健康检测尚未实现；路径电阻仅为条件满足后的等效估计。Windows 端口级功率与本机固件是否暴露 UCSI 数据面有关，未暴露时保持未知。

当前没有商业代码签名和 Apple 公证。Mac 使用临时签名，系统可能要求在“隐私与安全性”中允许打开已确认来源的应用；Windows 可能显示未知发布者。可使用 SHA256SUMS.txt 核验下载文件。
