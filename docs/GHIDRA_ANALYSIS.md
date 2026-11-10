# WhatCable 1.5.0：Ghidra 调用路径复核

日期：2026-11-10。工具：本机现有的 **Ghidra 12.1.4 / JDK 21.0.13 / Apple Silicon**。这是实际执行 Ghidra 后的记录，补充[前一份静态报告](WHATCABLE_ANALYSIS.md)。原应用及其 CLI、探针均未运行，安装目录未改动。

## 输入与分析范围

- 输入：用户提供的 `/Applications/WhatCable.app/Contents/MacOS/WhatCable`。
- Universal 文件 SHA-256：`fde60f05b1baf4e25fa9c9c923e3ce27f8c2d1b8db0e188bfb3cf094291179ce`。
- 用 `lipo -thin arm64` 生成工作副本，SHA-256：`2405623b45882efe498622d3f9b696a2fab07d3d6c76371521713d4650cb2ef1`。
- Ghidra 导入 Mach-O，分析 AArch64 指令和 Swift 符号，随后对 11 个采集相关函数进行定向反编译、枚举被调用函数和 IOKit 导入引用。
- 自动分析设置 2 个 CPU、2 GiB 堆与每文件 300 秒上限。**全程序自动分析触及时间上限，未全部完成**；之后的目标函数反编译、引用导出和工程保存成功。第二轮打开已保存工程、跳过全局分析，补查了内部读键函数及身份解析函数。

分析不涉及修改授权、支付或 DRM。没有把 WhatCable 二进制、完整 Ghidra 工程或原程序伪代码提交到公开仓库；仅发布我们编写的复查脚本及下述事实摘要。

## 已确认的调用路径

| 功能 / 地址 | 观察到的直接调用或常量 | 结论 |
|---|---|---|
| `SMCPowerReader.open`，`0x100234ee4` | `IOServiceMatching("AppleSMC")` → `IOServiceGetMatchingService` → `IOServiceOpen`；`IOServiceOpen` 引用位置 `0x100234f70` | 实际读取依赖 Apple 的本机服务，浏览器 WebUSB 不能直接代替该路径 |
| 内部 `readKey`，`0x100235df8` | 先转 FourCC，再两次调用内部 `callDriver`；可见命令字段常量 `9` 和 `5`，读取长度有 32 字节上限 | 存在查询键元数据、再读取键值的两阶段流程；字段语义仍需结合公开源代码和实机复核 |
| 内部 `callDriver`，`0x100235fd0` | `IOConnectCallStructMethod`；调用位置 `0x100236040`；selector 为 `2`；输入长度和初始输出长度均为 `0x50`（80 字节） | 从“存在导入”升级为已定位的具体调用点与参数常量 |
| `readSystemPowerInput`，`0x100235524` | 调用 `open`、`readKey`、浮点解码；Swift 小字符串常量对应 `VD0R`、`ID0R`、`PDTR` | 系统输入电压、电流和功率读数有独立 SMC 路径；不应把它们与适配器额定值混为一谈 |
| `readPortPowerChannels`，`0x100234fd0` | 调用 `open`、`readUUID`、`readKey`、浮点解码；可见对 1–4 通道的循环 | 单端口遥测与系统总输入采用不同结构；不能把通道序号直接当成机身印刷端口号 |
| `readPortContracts`，`0x1002356bc` | 调用 `readUUID`、`readKey` 并构建 `SMCPortContract` 数组 | 协商合同与实时电压、电流是分开的信息 |
| `AppleHPMInterfaceWatcher.refresh`，`0x10024f458` | 调用 `readPorts` 的优化版本 `0x100255b90`，随后调用 `apply` | 已观察到端口读取与界面状态更新之间的实际调用边 |
| `readPorts`，`0x10024f494` / 优化版本 `0x100255b90` | 遍历候选控制器类 → `IOServiceMatching` → `IOServiceGetMatchingServices` → 端口模型解析 | macOS 后端确实通过注册表控制器枚举建立端口列表 |
| `PowerSourceWatcher.refresh`，`0x10027ac5c` | 调用读取逻辑，应用 `Reading` 结果 | 电源信息有独立的读取及状态更新过程 |
| `VDMIdentityWatcher.parseUpdate` 的优化版本，`0x10029af44` | 通过 `IORegistryEntryCreateCFProperty` 读取 `Metadata`，处理 `VDOs`、VID / PID 和其它身份字段；存在单字段读取回退 | E-Marker/设备身份解析来自 IOKit 属性，而不是通过浏览器枚举普通 USB 描述符得到 |

补充引用点：`AppleSmartBatteryReader.properties` 中 `0x10021bd70` 引用 `IORegistryEntryCreateCFProperties`；USB 设备读取中 `0x1002902ac` 引用同一接口；HPM 批量属性读取闭包中该引用位于 `0x10024f650`。这些支持按“电池、USB 外设、物理端口”分别采集的设计。

## 证据强度与限制

Ghidra 能可靠辅助定位调用指令、符号、地址和立即数，但 Swift 的隐藏参数、可选值及寄存器返回值在伪 C 中并不总是准确。例如反编译器会复用表示 IOKit 返回值和传入句柄的变量。因此这里没有把伪 C 当成可以直接复制运行的实现，也没有把每个分支和单位全部判定为已经验证。

`VD0R` 等字符串由小字符串字节序还原；其物理含义还应与公开 MIT 后端和实机读数相互校验。通道 UUID 与物理端口的对应也应另行验证。本次没有调用 SMC 用户客户端，没有证明某一具体 Mac 一定提供这些键，更没有据此声称 Windows 具备相同接口。

## 对 CableScope 0.2 的落地结果（历史记录）

0.2 使用“网页 + 本机采集程序”：网页负责展示与报告操作，平台采集程序负责读取系统信息。当前仍沿用已实测的注册表与 Windows 公共读取路径，**没有把未经实测的 SMC 调用直接加入用户程序**。Windows 的 E-Marker、完整 PD 仍是未实现范围。

网页登录连接采用八位一次性配对码（五分钟有效、最多五次错误），随后签发仅保存在当前页面内存中的一小时会话；会话绑定网页来源。本机窗口可以撤销全部连接。采集程序仅监听 `127.0.0.1`，没有把硬件读取开放给任意网站。

## CableScope 0.3 后续实现

随后以公开 MIT 源码复核 ABI、字段单位与 UUID 对应方式，在本机实际验证只读 SMC 采集后，已加入系统输入、端口输出和输入合同。实现只开放命令 5 / 9，使用允许列表限制读取键；不写 SMC，也不更改 PD 协商。三类功率分别展示，使用 HPM 父控制器 UUID 连接到物理端口，匹配不唯一时不分配。输入路径被动拟合、引脚路由和显示模式比较也已接入界面。详见[0.3 功能范围与复现](FEATURES.md)。上面的“本次没有调用 SMC”仅描述 Ghidra 分析阶段，不代表 0.3 仍未实现。

## 复现

在 macOS 上安装兼容 JDK 与 Ghidra 后，运行：

```sh
python3 scripts/ghidra/analyze_whatcable.py \
  --ghidra-dir /path/to/ghidra_12.1.4_PUBLIC \
  --java /path/to/jdk-21/bin/java \
  --app /Applications/WhatCable.app \
  --output work/ghidra
```

脚本核对输入哈希、提取 arm64 副本，根据 `nm` / `swift-demangle` 动态挑选采集函数，然后调用 Ghidra headless。Java 脚本导出目标函数、直接调用边、IOKit 引用和伪 C。完整伪代码仅写入指定工作目录，不用于默认发布。重复运行时请使用新的输出目录或显式管理已有 Ghidra 工程。
