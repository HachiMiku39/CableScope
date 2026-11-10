# CableScope · 线缆实验室

免费、MIT 开源的本地 USB-C 连接观察工具。Windows 与 macOS 共用中文界面，按系统实际提供的数据展示结果。运行时无需账户，也不会向外部服务上传设备信息。

当前为 **0.3：网页 + 本机采集程序**，已加入 Mac 只读 SMC 功率、协商诊断、引脚路由、显示模式比较和结构化线缆测试记录。Windows 能运行不代表能读取与 Apple Silicon 相同的底层数据。[功能与实测范围](docs/FEATURES.md)列出了已实现能力和缺口。

## 网页入口与配对

[打开 CableScope 网页](https://cablescope-lab.leisurelyhorse4.chatgpt.site/)。当前部署为创建者私有；需要网页所属账户访问。源码和采集程序仍可独立下载、离线使用。

1. 下载并启动 **0.3 或更新版本**的本机采集程序。Windows x64 / ARM64 和两种 Mac 均有对应包。
2. 程序打开本机控制页，显示八位配对码与端口，默认端口为 `47831`。
3. 在网页填写配对码，点击“连接本机”。如浏览器询问，允许该网页访问本机；实时连接建议使用 Chrome / Edge。浏览器版本与企业策略可能限制网页访问本机，此时仍可使用本机控制页。
4. 配对码五分钟有效且只能使用一次；连续五次错误后，须在本机页刷新。网页会话一小时有效，只保存在页面内存中，不写入网址或浏览器存储。
5. 在本机页点击“撤销所有网页连接”即可立即使全部网页会话失效。

设备数据直接在采集程序与当前浏览器之间传输，不上传网页服务器。网页也支持导入 CableScope JSON 报告离线查看。0.1 程序尚不支持网页配对。

默认采集程序只允许上述网页来源跨来源配对。自行部署时，通过 `--site-url https://your-site.example` 指定自己的网页；使用 `python scripts/prepare_web.py /path/to/dist` 导出静态网页资源。`--port` 可调整监听端口，`--port 0` 使用系统随机分配端口；网页填入本机页显示的对应端口。

## 下载与运行

在本仓库 **Actions → Build CableScope → 最近一次成功构建 → Artifacts** 中下载对应文件。解开 Artifact 后，再解开里面的产品 ZIP。

| 下载包 | 系统 | 启动方式 |
|---|---|---|
| `windows-x64` | Windows 10/11 x64 | 解压整个目录，双击 `CableScope.exe` |
| `windows-arm64` | Windows 11 ARM64 | 同上，原生 ARM64 解释器与程序 |
| `macos-arm64` | macOS 15+，Apple Silicon | 解压后双击 `CableScope.app` |
| `macos-x64` | macOS 15+，Intel | 同上，部分端口底层字段可能不公开 |

打包版自带 Python，不必另外安装。程序打开本机浏览器界面；点击右上角“退出”结束采样。关闭页面后，如果十分钟没有请求，服务自动结束。源码运行时也可以按 Ctrl+C 退出。

首版没有商业开发者签名或 Apple 公证。Mac 包只做临时签名；系统可能要求在“隐私与安全性”中允许打开已确认来源的应用。Windows 也可能显示未知发布者。构建附带 SHA-256 校验文件。

## 当前读取范围

| 功能 | macOS | Windows x64 / ARM64 |
|---|---|---|
| 本地界面、JSON / CSV 导出、手动保存测试快照 | 已实现 | 已实现 |
| 电量、外接电源、充电状态 | AppleSmartBattery | GetSystemPowerStatus |
| 系统输入功率趋势 | SMC VD0R / ID0R / PDTR，失败回退系统遥测 | 未提供；保持未知 |
| 多端口输出曲线、输入合同 | SMC 通道 UUID 与 HPM 控制器精确匹配后显示 | 尚未实现 |
| 电池净充放电功率 | 电压 × 电池电流 | 单电池 BatteryStatus 可读时显示，15 秒缓存 |
| 物理 USB-C / MagSafe 状态、方向、累计故障计数 | IOPort 可读时显示 | 首版未实现 |
| E-Marker 的 VID / PID、原始 VDO、已知字段 | SOP′ 已公开时显示；部分格式保留未知 | 首版未读取 UCSI |
| USB 设备当前链路 | IOUSBHostDevice | USB Hub IOCTL；失败则降级为 PnP 列表 |
| Thunderbolt / USB4 设备拓扑 | system_profiler，15 秒缓存 | 首版未实现 |
| 当前显示模式、系统最高可选模式比较 | CoreGraphics | EnumDisplaySettingsW |
| USB-C 24 引脚逻辑路由、MagSafe 五触点示意 | 注册表有配置时绘图；MagSafe 为结构示意 | 尚未读取物理端口路由 |
| DisplayPort lanes / 速率 / 隧道 | IOPort 状态存在时显示，未判定 DSC | 尚未实现 |
| 被动输入路径等效电阻估计 | 满足样本与拟合条件时显示；不是线材直流电阻实测 | 尚无所需电压、电流与合同数据 |
| 线长、价格、柔软度、端口归属、记录筛选 | 人工填写，连同快照保存 | 同左 |

Windows 普通 USB Hub 端口与机身 USB-C 口没有直接的一一对应关系。UCSI 的开发测试接口默认关闭；本工具不修改注册表、不重启控制器、不切换供电角色、不安装驱动。参考[微软 UCSI 文档](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/ucsi)。

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

每组先跑解码、SMC ABI、诊断、数据存储与配对测试、纯 JavaScript 界面渲染检查及本机服务检查，再用 PyInstaller 6.22.3 打包，随后再次检查打包程序。产物保存十四天，附带 SHA-256；不会自动发布 Release。Actions 的虚拟机通常没有真实 USB-C 配件，构建通过只证明程序可启动及基本路径可用，不能证明硬件覆盖。四种架构是否构建成功，请以实际 Actions 结果为准。

```sh
python -m unittest discover -s tests -v
python scripts/smoke.py
```

## 研究与证据

[WhatCable 的静态分析及独立实现说明](docs/WHATCABLE_ANALYSIS.md)。本项目没有修改 WhatCable，也不包含授权、支付或 DRM 绕过。

[Ghidra 调用路径复核与可复现脚本](docs/GHIDRA_ANALYSIS.md)：已定位 SMC 用户客户端、读键调用点与 IOKit 身份解析路径；记录了自动分析的时间上限和 Swift 伪代码的限制。
