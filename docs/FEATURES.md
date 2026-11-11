# CableScope 0.3：已落地功能与证据

实现方式是公开源码复核、Ghidra 调用点验证、独立 Python 采集和桌面应用内嵌的本地界面。没有修改用户提供的 WhatCable，也没有包含其授权模块、插件或二进制。

## 与截图中的功能对应

| 项目 | 当前实现 | 边界 |
|---|---|---|
| Power Monitor | 系统输入 / 电池净功率曲线、Mac 多端口输出曲线、插拔事件、CSV / JSON | Windows 暂无实时系统输入与物理 USB-C 端口功率；Mac 依赖 SMC 键存在及系统允许 |
| Negotiation Diagnostics | 电源公布值、E-Marker 声明、输入合同电压 / 电流 / 功率、实际输入分列，并提示有证据的容量差异 | 没有完整电脑端口上限与电源 PDO 时，不断言具体瓶颈；Windows 仅探测 UCSI 数据面是否存在，不读取或解析 UCSI 命令与 PD 合同 |
| Type-C 端口候选 | Windows 用 Hub IOCTL 端口属性识别可插拔、非调试的 Type-C 端口候选，按伴口关系去重 | 候选不等于机身物理口；连接态、线缆身份、方向与功率全部未知，不能由 USB 设备存在反推 |
| Display Diagnostics | 两平台当前模式、系统最高可选分辨率及其刷新率、同分辨率刷新率比较；Mac 可显示 DP lanes、原始速率 / 描述和隧道状态 | 不推断面板原生上限；无显示器到 USB-C 端口的完整映射，DSC、完整 DP 带宽及 HDMI 协商判定未实现 |
| Cable Diagnostics | Mac 物理端口、方向、活动协议、E-Marker 已知字段、原始 VDO、24 引脚逻辑路由与累计故障字段 | 图中固定引脚只是用途；未逐针测导通或健康。未知路由不强行译成 Thunderbolt |
| MagSafe pin diagrams | MagSafe 五触点标准结构示意，旁列实时端口状态、输入与合同 | 未测每个触点的电压、温度或损伤，不是实时逐针诊断 |
| 线缆测试记录 | 名称、端口、长度、价格、柔软度自评、备注、整机快照、名称筛选与报告导出 | 人工标注，不能自动唯一识别实体线缆；不编造综合性能分 |
| 电阻研究 | 满足条件时对输入路径电压与电流作被动线性拟合 | 是整个供电路径的等效估计，不是线材独立直流电阻，不能替代线阻仪 |

## 采集与归属

- `VD0R` / `ID0R` 是系统 DC 输入电压和电流；优先用独立 `PDTR` 功率，缺失时才用 V×I。不是墙端交流功率。
- `D1..D4` 的 `JV` / `JI` 是端口输出遥测。`MP` / `MV` / `MI` 是输入合同的 mW / mV / mA。二者不混用。
- `DxUI` 的 16 字节控制器标识与 IOPort 的 HPM 父控制器 UUID 精确匹配，只在唯一匹配时归属。内核 UUID 不写入报告。通道序号不等于机身端口号。
- 系统只有一个有效输入合同时，系统输入轨可关联至该端口，但仍明确标成系统轨；不声称存在独立逐口输入电流传感器。全局充电器额定值不分配给输出端口。
- `IOPortTransportStateDisplayPort` 独立采集；仅用唯一的父端口类型 / 编号关联，不用显示器名称猜端口。
- 系统最高模式按真实的分辨率与刷新率组合选取。不会把 4K 60 Hz 与 1080p 240 Hz 拼成不存在的 4K 240 Hz。

### Windows 采集边界与归属

- `IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES`（0x220458）按 pack(1) 的 18 字节最小布局读取：ConnectionIndex、ActualLength、UsbPortProperties、CompanionIndex、CompanionPortNumber 与变长 WCHAR 伴口链接。先用 18 字节缓冲区取 ActualLength（只接受 18–65536），再按该长度二次调用；多伴口时递增 CompanionIndex 直到 CompanionPortNumber 为 0。
- 只采用用户可插拔（0x1）、非调试口（0x2 未置位）且 PortConnectorIsTypeC（0x8）置位的行；0x4 仅表示存在多个伴口，0x10 是保留位，不当作 UCSI 标志，也不存在 64 位 ConnectorId。
- 伴口组键是规范化伴口 hub 符号链接名与 CompanionPortNumber 的 SHA-256 前 16 位；同一组多行合并为一条候选，优先保留含 Type-C 位的行。无伴口行按 hub 哈希加端口单独分组，哈希键冲突且伴口身份不一致时保留独立行并在 source 注明歧义。原始 hub 路径只用于规范化与哈希，不写入快照、错误或日志。
- 候选写入 `ports`，id 为规范化 hub 链接与 ConnectionIndex 的 SHA-256 前 16 位；`connected` 恒为 `null`，合同、输出 / 输入功率、线缆、引脚配置、方向、过流 / 插拔 / 液体检测均为空值。设备枚举或 PnP 设备存在不证明物理连接态，也不会生成插拔事件。
- 同一 PowerShell CIM / PnP 进程（15 秒缓存、12 秒超时）额外查询 `Get-PnpDevice -Class UCM`、UCSI WMI 类与名称匹配 UCSI/UCM 的设备节点，只返回存在性三态：`available`、`no_data_plane`、`query_failed`。查询失败一律降级，不推断 PD 合同或电源角色；程序不解析 UCSI RDO/PDO、不启用测试接口、不修改注册表、不重启设备、不安装驱动。

## 电阻估计的门槛

只有外接电源已连接、输入端口与合同唯一、独立输入功率和电压 / 电流均可用时积累样本。电流须为 0.3–6 A，输入电压在合同电压 −1.5 至 +0.5 V 内，独立功率须与 V×I 在 2 W 或 15% 内相符。至少 24 个样本、电流跨度至少 0.75 A、负斜率、R² ≥ 0.8、合理截距与 1–800 mΩ 范围同时满足才显示估计。更换端口、插拔、线缆声明指纹或合同电压改变时重置。

充电器调压、不同测量点及系统内部损耗都会影响斜率，因此界面始终标注“输入路径等效估计”。工具不会主动制造负载或调整电压。

## 实测和自动检查

2026-11-10 在用户提供应用的同一台 Apple Silicon Mac 上，独立运行 CableScope 读取到 4 个控制器通道和 4 个物理端口。充电状态下输入轨约 27.8 V；三路 USB-C 输出存在约 1–5 W 读数；唯一 MagSafe 输入合同为 28 V × 4.99 A = 139.72 W。数值随当前负载变化，这只证明本机采集路径可用，不代表所有 Mac 都支持相同键。

本次环境没有枚举到可用的显示模式，未实机验证外接显示器或 DSC；显示模式比较使用合成模式对测试。被动电阻实机样本电流变化不足，程序正确保持不输出估计；已用已知斜率及正斜率等合成数据验证门槛。

2026-11-11 在 Windows 11 26200 x64、连接 RTL9210B Type-C 固态盘的本机复核：`Get-PnpDevice -Class UCM` 返回「类不存在」，`root/wmi` 与 `root/cimv2` 均无 UCSI WMI 类（仅有 USB4 路由相关类），名称匹配 UCSI/UCM 的 PnP 节点为 0，因此 `ucsi_state=no_data_plane`。同机只读 Hub IOCTL 读出 2 条 Type-C 端口候选，连接态保持 `null`。这是本机负结果：证明该机固件未提供可读取的数据面，不证明其它机型也没有；未启用 Microsoft UCSI 测试接口，因为那需要写注册表并重启设备。

```sh
python -m unittest discover -s tests -v
node scripts/test_web.cjs
python scripts/smoke.py
```

39 项单元测试覆盖 ABI 的 80 字节布局和偏移、只读命令限制、SMC 内部错误、大小端、UUID 校验、引脚规则、合同归属、模式配对、电阻门槛与存储，另以合成数据覆盖 Windows 端口属性 pack(1) 布局、位掩码、两阶段长度、伴口循环与分组、UCSI 三态及连接态不反推。服务检查覆盖真实后端启动、静态资源、人工记录校验、报告导出、跨来源拒绝及退出。纯 JavaScript 测试覆盖实际模板渲染、候选端口全缺字段与转义，不等同于浏览器布局验证。当前浏览器预览工具超时，新版布局未完成实际浏览器截图复核。

四平台由 GitHub Actions 原生编译并检查打包程序。云运行器通常没有真实 USB-C 外设，因此构建成功不代表 Windows ARM 或其它硬件已验证所有传感器。

## 来源与许可

公开上游 [WhatCable v1.5.0 的固定提交](https://github.com/darrylmorley/whatcable/tree/58e588ab45350cf9512ce40d4080fbbef3b94ad0)中的 SMC ABI、功率键、HPM UUID 映射和引脚逻辑用于复核及改写；请以仓库内 [THIRD_PARTY_NOTICES](../THIRD_PARTY_NOTICES.md) 和 [MIT 许可副本](../licenses/WhatCable-MIT.txt)为准。未使用 `Sources/WhatCablePlugins` 下的商业插件。

Ghidra 的具体地址、输入哈希、分析超时和复现脚本见 [GHIDRA_ANALYSIS.md](GHIDRA_ANALYSIS.md)。
