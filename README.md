# CableScope · 线缆实验室

免费、MIT 开源的本地 USB-C 连接观察工具。Windows 与 macOS 共用中文界面，按系统实际提供的数据展示结果。运行时无需账户，也不会向外部服务上传设备信息。

当前为 **0.4：纯桌面 App**，已加入 Mac 只读 SMC 功率、协商诊断、引脚路由、显示模式比较和结构化线缆测试记录。Windows 能运行不代表能读取与 Apple Silicon 相同的底层数据。[功能与实测范围](docs/FEATURES.md)列出了已实现能力和缺口。

## 下载与运行

从 [GitHub Releases](https://github.com/HachiMiku39/CableScope/releases/latest) 直接下载，无需登录 GitHub。每次发布只有四个平台全部构建、启动与界面检查通过后才公开。

| 文件 | 系统 | 启动方式 |
|---|---|---|
| `windows-x64.exe` | Windows 10/11 x64 | 双击直接运行 |
| `windows-arm64.exe` | Windows 11 ARM64 | 双击直接运行，原生 ARM64 |
| `macos-arm64.zip` | macOS 15+，Apple Silicon | 解压后把 `CableScope.app` 拖入应用程序，双击打开 |
| `macos-x64.zip` | macOS 15+，Intel | 同上 |

Windows EXE 是便携自解压包，第一次启动会先解包。Mac 用 ZIP 保留完整 APP 结构与执行权限。客户端自带 Electron 和原生 Python 采集程序，不必另外安装运行环境。打开后自动启动采集，关闭窗口或点击“退出”结束自己启动的采集程序。程序使用随机本机端口，允许和其它采集实例共存；现有数据库路径不变。

程序的 Electron 界面不开启 Node.js 权限，只访问自己启动的本机采集程序；下载页与外链由系统浏览器打开。导出时可选择文件位置。免费、开源、无需账户、无授权激活。

当前没有商业开发者签名或 Apple 公证。Mac 包只做临时签名；系统可能要求在“隐私与安全性”中允许打开已确认来源的应用。Windows 可能显示未知发布者。Release 附带 `SHA256SUMS.txt` 校验文件。

## 当前读取范围

| 功能 | macOS | Windows x64 / ARM64 |
|---|---|---|
| 本地界面、JSON / CSV 导出、手动保存测试快照 | 已实现 | 已实现 |
| 电量、外接电源、充电状态 | AppleSmartBattery | GetSystemPowerStatus |
| 系统输入功率趋势 | SMC VD0R / ID0R / PDTR，失败回退系统遥测 | 未提供；保持未知 |
| 多端口输出曲线、输入合同 | SMC 通道 UUID 与 HPM 控制器精确匹配后显示 | 本机未提供可读取的数据面；保持未知 |
| 电池净充放电功率 | 电压 × 电池电流 | 单电池 BatteryStatus 可读时显示，15 秒缓存 |
| 物理 USB-C / MagSafe 状态、方向、累计故障计数 | IOPort 可读时显示 | 仅用 Hub IOCTL 识别 Type-C 端口候选；连接态与故障计数未提供 |
| E-Marker 的 VID / PID、原始 VDO、已知字段 | SOP′ 已公开时显示；部分格式保留未知 | 本机未提供可读取的 UCSI 数据面；不解析 PDO / RDO，保持未知 |
| USB 设备当前链路 | IOUSBHostDevice | USB Hub IOCTL；失败则降级为 PnP 列表 |
| Thunderbolt / USB4 设备拓扑 | system_profiler，15 秒缓存 | 首版未实现 |
| 当前显示模式、系统最高可选模式比较 | CoreGraphics | EnumDisplaySettingsW |
| USB-C 24 引脚逻辑路由、MagSafe 五触点示意 | 注册表有配置时绘图；MagSafe 为结构示意 | 本机未提供可读取的物理端口路由；候选端口不绘图 |
| DisplayPort lanes / 速率 / 隧道 | IOPort 状态存在时显示，未判定 DSC | 尚未实现 |
| 被动输入路径等效电阻估计 | 满足样本与拟合条件时显示；不是线材直流电阻实测 | 尚无所需电压、电流与合同数据 |
| 线长、价格、柔软度、端口归属、记录筛选 | 人工填写，连同快照保存 | 同左 |

Windows 普通 USB Hub 端口与机身 USB-C 口没有直接的一一对应关系。工具用只读 `IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES` 识别可插拔、非调试、标记 Type-C 的端口候选，并在同一 CIM / PnP 查询中探测 UCSI 数据面节点是否存在；候选不代表机身物理口，也不代表已连接，设备存在不会被反推为连接态。即使探测到 UCM/UCSI 节点，本版仍不读取 UCSI 命令、PDO / RDO 或线缆身份，因此 PD 合同、电源角色和单端口输入功率保持未知。UCSI 的开发测试接口默认关闭；本工具不修改注册表、不重启控制器、不切换供电角色、不安装驱动。参考[微软 UCSI 文档](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/ucsi)。

## 如何比较两条线

固定同一台电脑、端口、充电器、外设和测试条件。测试一条线后保存快照，填写名称、所选端口、线长、价格、柔软度自评；备注记录电池电量和负载。再更换线缆重复测试。记录支持名称筛选和 JSON 导出；不自动给主观柔软度和性能混合打分。

功率页可以切换系统曲线与多端口输出曲线。端口卡片分别列出**输出遥测、系统输入轨遥测、PD 合同上限**。协商诊断分列电源公布值、线缆芯片声明和实际取电，未知的电脑端口上限保持空白。

路径电阻估计只被动观察已有负载：至少 24 个有效样本、电流跨度至少 0.75 A、拟合 R² 至少 0.8 等条件全部满足才显示。插拔、输入端口或合同电压变化会重置样本。读数包含充电器、触点与内部路径影响，不能当作线材的独立直流电阻，也不能用于精确比较线材线阻。

报告保存整机状态，不会自动认定数据属于某一条线。E-Marker 指纹可能在同型号线缆间重复，不用于唯一识别实体线。当前链路速率不等于实际拷贝吞吐；低输入功率也可能来自负载、电量或温控。没有完整证据时不生成“线缆是瓶颈”的结论。

## 数据保存位置

- 打包版 Windows：`%LOCALAPPDATA%\CableScope\history.sqlite3`
- 打包版 Mac：`~/Library/Application Support/CableScope/history.sqlite3`
- 源码运行：程序旁的 `data/history.sqlite3`

采样间隔约五秒，每次保留采样时间；硬件读数可能由系统自行缓存。数据库保留最新 5000 次采样、最多 500 份手动快照；界面展示最近 50 份快照。插拔事件仅保留本次运行的最近 100 次。备份数据库即可保留全部记录。默认不保存序列号、MAC 地址、主机名。导出的报告仍包含设备名称、VID/PID、模式和时间，请按需分享。

## 从源码运行

Python 3.9+，无第三方运行依赖：

```sh
python3 run.py
python3 run.py --json
python3 run.py --no-browser --data-dir /path/to/test-data
```

Windows 使用 `py -3 run.py`。Mac 也可双击 `启动 Mac.command`，Windows 可双击 `启动 Windows.bat`。这些源码启动器需要预先安装 Python；云编译下载包自带 Python。

## GitHub 云编译

推送到 `main`、推送版本标签、创建 PR 或手动运行 `Build CableScope` 都会触发构建。四组任务分别使用 GitHub 的 `windows-2025`、`windows-11-arm`、`macos-15`、`macos-15-intel` 原生运行器；不进行跨架构模拟打包。

每组先测试解码、SMC ABI、诊断与数据存储，再打包并检查原生采集程序。随后用锁定版本的 Electron / electron-builder 生成 EXE 或 APP，并启动真实的打包客户端检查页面渲染、硬件快照、保存和报告。最后汇集四个平台产物、核验格式与 SHA-256，以草稿上传完整资产后一次性公开 Release。

GitHub 虚拟机通常没有真实 USB-C 配件，构建与启动成功不能证明硬件覆盖；两平台的采集差异仍以上表为准。Pull Request 只构建，不发布。正式发布需提升 `package.json` 和 Python 版本；已发布版本不会自动覆盖。

```sh
python -m unittest discover -s tests -v
npm ci
npm test
python scripts/smoke.py
```

开发 Electron 界面：先安装 Python 3.9+，再 `npm ci && npm start`。客户端编译在 GitHub 进行，源码中的 `scripts/build.py --collector-only` 负责生成每个平台原生内嵌采集程序。

## 研究与证据

[WhatCable 的静态分析及独立实现说明](docs/WHATCABLE_ANALYSIS.md)。本项目没有修改 WhatCable，也不包含授权、支付或 DRM 绕过。

[Ghidra 调用路径复核与可复现脚本](docs/GHIDRA_ANALYSIS.md)：已定位 SMC 用户客户端、读键调用点与 IOKit 身份解析路径；记录了自动分析的时间上限和 Swift 伪代码的限制。
