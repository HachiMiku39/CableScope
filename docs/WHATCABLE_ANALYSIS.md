# WhatCable 1.5.0 静态检查与 CableScope 实现记录

后续已使用 Ghidra 进行定向反编译与调用引用复核，详见 [Ghidra 分析补充](GHIDRA_ANALYSIS.md)。本文保留最初静态检查记录。

检查日期：2026-11-10（Asia/Shanghai）。范围：用户提供的本地应用 `/Applications/WhatCable.app`，以及作者公开源码。未执行该应用、其 CLI 或探针；未修改安装目录。本文只保存必要的技术证据，不包含整段反编译代码、授权数据或设备序列号。

## 1. 发行与授权

作者发行仓库为 [darrylmorley/whatcable](https://github.com/darrylmorley/whatcable)，[当前核实的最新 Release 是 v1.5.0](https://github.com/darrylmorley/whatcable/releases/tag/v1.5.0)，发布时间为 2026-10-13 12:07:41 UTC，提供 `WhatCable.zip` 和 `whatcable-cli-1.5.0.zip`。

[官网 Pro 页](https://www.whatcable.uk/pro)显示 Pro 为 £9.99 一次购买，可用于两台 Mac；免费基础功能仍存在。截图显示的是 Pro 功能，不表示整个程序都收费。

[仓库根 LICENSE](https://github.com/darrylmorley/whatcable/blob/main/LICENSE)以 MIT 为默认，但明确规定带独立 LICENSE 的子目录遵循自己的条款，特别提及私有开发库中的 `Sources/WhatCablePlugins/` 为专有代码。不能把“基础源码 MIT”推导为“付费插件可以任意复制”。

源码参考点：`v1.5.0` 指向提交 `58e588ab45350cf9512ce40d4080fbbef3b94ad0`。本次检查的根 LICENSE blob 为 `272cd697ed272f9d1d5642f0699c0c410133445a`（main 当时状态）。未下载官方 Release 二进制进行逐字节比对，所以不宣称本地文件与官方发行完全一致。

## 2. 本地输入与指纹

最初仅提供了收费界面截图；随后明确提供了 `/Applications/WhatCable.app`，因此完成了本地只读静态检查。

| 项目 | 观察结果 |
|---|---|
| 版本 | `CFBundleShortVersionString=1.5.0`；`CFBundleVersion=140` |
| Bundle ID | `uk.whatcable.whatcable` |
| 主程序 | `Contents/MacOS/WhatCable` |
| 架构 | Mach-O universal：`x86_64`、`arm64` |
| 最低版本 | Info.plist 与 arm64 `LC_BUILD_VERSION` 均为 macOS 14.0 |
| SDK | arm64 `LC_BUILD_VERSION`: macOS 27.0 |
| 其他可执行文件 | `Contents/Helpers/whatcable`；Widget extension；若干诊断探针 |
| 主程序 SHA-256 | `fde60f05b1baf4e25fa9c9c923e3ce27f8c2d1b8db0e188bfb3cf094291179ce` |
| CLI SHA-256 | `d1b2456c286fa093ab8d2934574bb734a3b00d3397f0b785ec35b0fc416921d5` |

主程序与 CLI 均含两种架构；这不证明 Intel Mac 可以公开与 Apple Silicon 相同的端口控制器数据。[作者说明](https://github.com/darrylmorley/whatcable#install)对 Intel 的底层读取能力作了限制。

## 3. 签名观察

`codesign -dv --verbose=4` 可读到 TeamIdentifier `M4RUJ7W6MP`、hardened runtime 标志与 stapled notarization ticket 的元数据；Authority 显示 unavailable，且输出 `Info.plist=not bound`。

`codesign --verify --deep --strict` 未通过，arm64 与 x86_64 单独检查均报告 invalid signature。提取 entitlements 时提示 invalid entitlements blob，未获得可靠的有效授权列表。

**结论仅限这份本地副本在当前环境中的验证结果。** 存在签名元数据或公证票据，不等于签名验证通过；本结果也不足以确定文件被谁修改、何时修改或是否恶意。没有把它推广成对官方发行包的判断。

## 4. 依赖、语言与可见符号

arm64 依赖包括 Foundation、AppKit、SwiftUI、Combine、Charts、CoreFoundation、CoreGraphics、IOKit、ServiceManagement、UserNotifications、WidgetKit、CryptoKit、libsqlite3、libobjc 及 Swift runtime。

`nm -arch arm64 -j` 读取到 59,270 行符号记录。可见大量 Swift mangled 符号；`swift-demangle` 能恢复模块与类型名。Objective-C 元数据中可见 `_TtC9WhatCable11AppDelegate`、`AppSettings`、`ConnectionFaultTracker` 等 Swift 类的运行时名称。该结果表明符号信息足够辅助分析，不表示恢复了完整原始工程。

有意义的导入包括：

- `IOServiceMatching`、`IOServiceGetMatchingServices`、`IOServiceAddMatchingNotification`、`IOServiceAddInterestNotification`
- `IORegistryEntryCreateCFProperties`、`IORegistryEntryCreateCFProperty`、`IORegistryEntryGetParentEntry`、`IORegistryEntryGetChildIterator`
- `IOServiceOpen`、`IOConnectCallStructMethod`、`IOServiceClose`
- `IOPSCopyExternalPowerAdapterDetails`
- `CGDisplayCopyDisplayMode`、`CGDisplayCopyAllDisplayModes`、`CGDisplayModeGetPixelWidth/Height`、`CGDisplayModeGetRefreshRate`
- `sqlite3_open_v2`、`sqlite3_prepare_v2`、绑定和读取函数

## 5. 数据路径：证据与推断分开

下面是由公开源码、导入和符号共同支持的模块关系，**不是通过动态跟踪或完整反汇编获得的逐指令调用图**。

| 信息 | 可复查的数据源 / 公开模块 | 二进制中的佐证 | 确信边界 |
|---|---|---|---|
| 端口状态、方向、活动协议 | `AppleHPMInterfaceType10/11/12/18`、`AppleTCControllerType10/11`、`IOPort` → `AppleHPMInterfaceWatcher` | 服务名字符串、Watcher 类型与 IOKit 注册表导入存在 | 源码可检查；具体硬件覆盖需实测 |
| 线缆 E-Marker | `IOPortTransportComponentCCUSBPDSOPp` → `Metadata.VDOs` → `VDMIdentityWatcher` → PD 解码 | SOP / SOPp / SOPpp 字符串、`CableDiagnosticView` 中的 `identityWatcher` 类型信息 | SOP 是设备，SOP′ 是线缆，不能混用；并非所有线都可读 |
| 电源 PDO | `IOPortFeaturePowerSource`、`IOPortFeaturePowerSourceOptionFixed` → `PowerSourceWatcher` | 相应字符串及 `powerWatcher` 字段类型 | 具体档位与当前 RDO 的对应尚未逐指令验证 |
| SMC 供电 | `AppleSMC` → `SMCPowerReader.open/readKey/callDriver` → `readSystemPowerInput/readPortPowerChannels/readPortContracts` | 相关方法符号、`IOServiceOpen` 与 `IOConnectCallStructMethod` | 符号支持此路径，未执行私有用户客户端调用，未测具体 selector |
| 显示状态 | CoreGraphics 当前显示模式与全部模式 → `DisplayDiagnostic` | CGDisplay 导入、DisplayDiagnostic 类型、`WhatCablePlugins.DisplayWindow` 字段 | 当前模式不直接证明 DP 物理带宽或 DSC 状态 |
| 历史保存 | SQLite | sqlite3 导入 | 未打开用户现有 WhatCable 数据库，不分析其私有记录 |

公开源码优先入口：

- [VDMIdentityWatcher.swift](https://github.com/darrylmorley/whatcable/blob/58e588ab45350cf9512ce40d4080fbbef3b94ad0/Sources/WhatCableDarwinBackend/Watchers/VDMIdentityWatcher.swift)
- [AppleHPMInterfaceWatcher.swift](https://github.com/darrylmorley/whatcable/blob/58e588ab45350cf9512ce40d4080fbbef3b94ad0/Sources/WhatCableDarwinBackend/Watchers/AppleHPMInterfaceWatcher.swift)
- [USBPDVDO.swift](https://github.com/darrylmorley/whatcable/blob/58e588ab45350cf9512ce40d4080fbbef3b94ad0/Sources/WhatCableCore/PD/USBPDVDO.swift)

本地可见 `WhatCablePlugins` 的类型信息，结合根 LICENSE，说明“能看到符号”不等于拥有重新分发插件实现的授权。CableScope 独立实现，不包含该插件代码或原程序资源。

## 6. 可复现检查

在安装了 Xcode Command Line Tools 的 Mac 上：

```sh
sh scripts/inspect-whatcable.sh /Applications/WhatCable.app
```

该脚本只读取 Info.plist、Mach-O、代码签名、依赖、符号和少量相关字符串，直接输出报告。不运行目标程序，不保存或修改目标文件。

如果要复核原始 Swift/Objective-C 信息，可用：

```sh
nm -arch arm64 -j /Applications/WhatCable.app/Contents/MacOS/WhatCable | xcrun swift-demangle
otool -ov -arch arm64 /Applications/WhatCable.app/Contents/MacOS/WhatCable
```

## 7. 独立免费实现与验证

CableScope 为 Rust 原生实现（Tauri 壳加系统 API 手写 FFI），中文界面内嵌于桌面窗口。Mac 通过 IOKit 公共入口读取注册表属性，没有向 USB 总线发送深度探测请求。Windows 使用公开 Hub 查询 IOCTL、GetSystemPowerStatus、CIM / PnP 和 EnumDisplaySettingsW。依据[微软文档](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/ucsi)，UCSI 测试接口默认未开启，因此首版 Windows 不宣称能读取所有线缆 E-Marker 或 PD 档位。

已在本机 Apple Silicon 环境完成：14 项解码、结构布局和持久化测试；真实快照读取；本机 HTTP 服务的资源加载、跨来源请求拒绝、手动快照保存、JSON / CSV 导出和退出检查。一次快照读取到 4 个端口、3 份 E-Marker 与系统输入功率。显示器枚举在本环境返回空列表，因此当前未验证真实显示器模式读取的硬件结果。

Windows x64、Windows ARM64、Mac Intel 与 Mac ARM64 均配置 GitHub 原生运行器构建。云端结果应以仓库 Actions 的实际状态为准；虚拟机启动检查不能代替 Windows ARM 实机和各类 USB-C 配件的测试。未把本机真实硬件快照、原程序、完整字符串库或用户数据库提交到仓库。

## 8. 未实现范围

本次未破解付费授权、支付或 DRM，未补丁原程序。未逆向付费插件的完整算法。独立首版没有单端口 SMC 电流、完整 PD 协商过程、DP/DSC 带宽判定、线阻实测、真实吞吐测试或自动综合评分。后续应通过公开接口、明确授权的代码和实机证据继续补齐。
