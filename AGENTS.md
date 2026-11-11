# CableScope · AGENTS.md

本文件面向 AI 编码代理，假设读者对本项目一无所知。所有信息均来自仓库实际内容，修改代码导致以下描述失准时请同步更新本文件。

## 项目概述

CableScope（线缆实验室）是免费、MIT 开源的本地 USB-C 连接观察与诊断工具，支持 Windows 10/11（x64 / ARM64）和 macOS 15+（Apple Silicon / Intel），界面与文档语言为中文。

产品形态为**纯桌面 App**：网页版与外部网页配对（`/bridge/`、`pairing.py`、`site.json`、`scripts/prepare_web.py`、菜单「打开网页版」）已从代码中移除。网页 UI 文件（`cablescope/web/`）保留为应用窗口内嵌界面，由桌面壳加载本机令牌化回环 URL 渲染，不是独立产品。

核心形态：纯 Python 3.9+ 本机采集程序（仅标准库，无第三方运行依赖），通过只回环的 HTTP 服务提供界面；桌面壳正在从 Electron（`desktop/`，仍在服役）迁移到 Tauri 2.x（`src-tauri/`，阶段一已落地：壳 + Python sidecar 过渡期，窗口加载 collector 令牌 URL，NSIS + WebView2 在线 bootstrapper）。运行时无需账户、不向外部服务上传设备信息。

后端正在分波次重写为 Rust（根 `Cargo.toml` workspace + `crates/`）：`core`（数值清洗/PD3 VDO 解码/诊断 + `py` 语义兼容层）、`analysis`（诊断/回归/enrich）、`engine`（rusqlite 存储 + 采样线程）、`backends-traits`、`backends-windows`、`backends-macos`+`smc`（FFI 经 `aarch64-apple-darwin` check 验证，真机读取待 Mac 实测）、`cli`（`--json` 单次采集，Windows 真机字段级差分零差异）已移植。`server` crate 为占位（纯 App 形态下终态走 Tauri IPC，不需要 loopback HTTP 服务）。在 Rust 版达到对等并通过全部黑盒验收前，Python 版仍是发布与打包形态。

数据采集完全只读：不修改注册表、不写 SMC、不重启控制器、不切换供电角色、不安装驱动、不主动制造负载。所有诊断结论必须基于证据，证据不足时保持"未知"，不生成"线缆是瓶颈"之类的推断。

## 目录与模块划分

| 路径 | 作用 |
|---|---|
| `run.py` | 命令行入口。参数：`--json`（读一次输出 JSON）、`--no-browser`、`--port`（默认 47831，`0` 为随机端口）、`--data-dir` |
| `cablescope/__init__.py` | `__version__`，发布时需与 `package.json` 同步提升 |
| `cablescope/core.py` | 数值清洗（`number`/`scaled`/`signed64`）、E-Marker VDO 按 PD3 公开字段解码（`decode_identity`）、诊断提示（`diagnostics`） |
| `cablescope/backends/` | 平台采集。`__init__.py:create()` 按 `sys.platform` 分发；`macos.py` 用 ctypes 调 IOKit 读 IORegistry（IOPort、HPM 控制器 UUID 匹配）；`smc.py` 只读 AppleSMC 电源键（仅读键命令 5 与键信息命令 9，ABI 布局有运行时校验）；`windows.py` 用 Win32 `GetSystemPowerStatus`、USB Hub IOCTL（含 `IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES`：Type-C 端口候选识别与伴口分组，pack(1) 字节偏移解析）、UCSI 数据面只读存在性探测（三态 available/no_data_plane/query_failed，只探测不解析命令）、只读 CIM |
| `cablescope/analysis.py` | 纯函数诊断：USB-C 24 引脚逻辑路由图（`pin_map`）、显示模式比较（`mode_comparison`，不得跨模式拼分辨率/刷新率）、协商诊断（`negotiation`，电源公布值/线缆声明/合同/实测分列）、输入路径等效电阻被动回归（`ResistanceTracker`，见下）、`enrich` 快照增强 |
| `cablescope/engine.py` | `Store`：sqlite3 两张表（`samples` 保留最近 5000 条、`records` 上限 500 份、界面展示最近 50 份）；`Engine`：后台线程每 5 秒采样，维护最近 100 条插拔事件，校验快照名称（1–80 字）与备注（≤1000 字）、线缆信息（线长/价格/柔软度范围） |
| `cablescope/server.py` | `ThreadingHTTPServer` 只监听 `127.0.0.1`；随机令牌 URL 前缀；严格 Host 校验、本地 Origin 校验、CSP、`no-store`、POST 限 8 KiB 且仅 application/json；空闲 600 秒自动退出 |
| `cablescope/web/` | 无构建步骤的原生 JS 界面：`index.html`、`style.css`、`app.js`（渲染与图表）、`connection.js`（本机/离线导入两种模式）。任何用户数据进 DOM 前必须经过 `esc()` 转义 |
| `desktop/` | Electron 壳（现役）。`main.cjs` 单实例窗口（禁 Node 集成、禁导航外跳、下载走系统对话框）；`collector.cjs` 启动/停止采集子进程，只接受子进程打印的令牌化回环 URL；`test/collector.test.cjs` |
| `src-tauri/` | Tauri 2 壳（阶段一，过渡期）。collector 监督器为 `collector.cjs` 协议的 Rust 移植（令牌 URL 白名单/8 KiB 缓冲/30s/3.5s）；单实例插件；导航仅放行 collector base；`capabilities/desktop-minimal.json` 仅 core 默认权限；`build.rs` 编译期断言 package.json ↔ tauri.conf.json ↔ `__init__.py` 版本一致；打包 NSIS currentUser + WebView2 downloadBootstrapper |
| `scripts/build.py` | PyInstaller 打包原生采集程序；`--collector-only` 供桌面壳内嵌；只允许在本机架构/系统运行（拒绝交叉编译），打包后自动跑 `smoke.py` |
| `scripts/smoke.py` | 真实后端 + 真实 HTTP 进程冒烟：快照、静态资源、保存/报告、CSV、跨源 403、退出码 |
| `scripts/smoke_tauri.cjs` | 对打包后的 Tauri raw exe 跑 `--smoke-test`（真实渲染、保存、报告、导航） |
| `scripts/diff_rust_python.py` | Python 版与 Rust CLI `--json` 快照的字段级差分（忽略 `at`；数组顺序无关比较），CI 与本地通用 |
| `scripts/test_web.cjs` | 无浏览器渲染测试：用 `vm` 加载前端 JS，校验 UI id、XSS 转义、图表与筛选 |
| `scripts/smoke_desktop.cjs` | 对打包后的 Electron 客户端跑 `--smoke-test`（真实渲染、保存、报告） |
| `scripts/check_release.py` / `scripts/publish_release.py` | 发布前校验四个平台产物并生成 `SHA256SUMS.txt`；仅以草稿 Release 上传，已发布版本不可覆盖（当前按 Electron 产物校验，Tauri 产物接入待阶段二） |
| `tests/` | 标准库 unittest：`test_core.py`（PD 解码、Windows IOCTL/Hub 连接器属性结构、存储）、`test_features.py`（SMC ABI、引脚图、模式比较、电阻门槛） |
| `启动 Windows.bat` / `启动 Mac.command` | 源码运行的双击启动器（要求预装 Python） |
| `licenses/` | Python 与 WhatCable（SMC ABI 参考来源，见 `docs/WHATCABLE_ANALYSIS.md`）许可证 |
| `docs/` | 功能与实测范围、发布说明、WhatCable 静态分析、Ghidra 调用路径复核 |
| `Cargo.toml` / `crates/` | 后端 Rust 重写（分波次进行）：`core`（数值清洗/PD3 VDO 解码/诊断 + `py` 语义兼容层）、`analysis`（诊断/回归/enrich）、`engine`（rusqlite 存储 + 采样线程）、`backends-traits`（`Backend` trait）、`backends-windows`、`backends-macos`+`smc`、`cli`（二进制名 `CableScope`，`--json`）已移植；`server` 为占位（纯 App 终态走 Tauri IPC，计划移除）。FFI 策略：手写 `extern "C"` 镜像 ctypes，不引 windows-sys/core-foundation；macOS 代码经 `cargo check --target aarch64-apple-darwin` 类型检查 |

数据库位置：打包版在系统应用数据目录（`%LOCALAPPDATA%\CableScope` 或 `~/Library/Application Support/CableScope`），源码运行时在仓库旁 `data/history.sqlite3`（已被 gitignore）。

## 构建与测试命令

环境要求：Python 3.9+（CI 用 3.13）、Node 24 与 Rust stable（本机 1.97）。运行与测试均不需要安装任何 Python 第三方包；`requirements-build.txt` 只有 PyInstaller（仅打包用）。

```sh
# 从源码运行
python3 run.py                 # Windows 用 py -3 run.py
python3 run.py --json          # 单次采集输出 JSON
python3 run.py --no-browser --data-dir /path/to/test-data

# 测试（三层，缺一不可）
python -m unittest discover -s tests -v   # Python 单元测试
npm ci && npm test                        # Node：collector 测试 + 网页渲染测试
python scripts/smoke.py                   # 真实后端 + 真实服务进程冒烟

# Rust 后端（87 个单元测试，合成数据）
cargo test --workspace
# Python ↔ Rust 真机差分（CI 的 rust-macos job 同款）
python3 run.py --json > py.json && cargo run -q -p cablescope-cli -- --json > rust.json
python3 scripts/diff_rust_python.py py.json rust.json

# macOS 代码跨平台类型检查（无需 macOS SDK）
cargo check --target aarch64-apple-darwin -p cablescope-smc -p cablescope-backends-macos -p cablescope-cli

# Electron 界面开发（本机需有 Python，Windows 可用 CABLESCOPE_PYTHON 指定）
npm ci && npm start

# Tauri 壳（阶段一）
npm run desktop:dev                       # 开发（源码 collector）
npm run desktop:build                     # 打包 NSIS（先 build.py --collector-only）
node scripts/smoke_tauri.cjs              # raw exe 冒烟
cd src-tauri && cargo test                # 壳单元测试

# 打包（只能在对应平台的原生机器上运行，CI 中执行）
python -m pip install -r requirements-build.txt
python scripts/build.py --target windows-x64 --collector-only   # 或 windows-arm64 / macos-arm64 / macos-x64
npm run desktop:package -- --mac --arm64                        # electron-builder
node scripts/smoke_desktop.cjs                                  # 需先完成打包
node scripts/smoke_desktop.cjs --portable                       # Windows 便携 EXE
```

## 测试策略

- 单元测试必须可在无硬件的 GitHub 运行器上通过：所有平台结构体布局、解码、拟合门槛都用合成数据验证，禁止依赖真实 USB 配件。
- `python scripts/smoke.py` 会启动真实采集进程并监听端口，是唯一接触硬件的自动化测试；CI 明确声明"构建与启动成功不能证明硬件覆盖"。
- Rust 移植以 `py -3` 差分向量固定 Python 语义（银行家舍入、真值、max 并列取第一个、CPython 补偿求和等）；Windows 后端已做真机字段级差分（`scripts/diff_rust_python.py`），macOS 后端经 `rust-macos` CI job（macos-15 / macos-15-intel 双 runner）做构建、测试与差分，真实硬件读取覆盖仍待有外设的机器补验。
- 前端渲染测试（`scripts/test_web.cjs`）不经过浏览器布局，只验证 DOM id 存在、内容转义与逻辑分支。
- 集成断言风格：大量 `assert`（smoke 脚本）与 `assertEqual/assertRaises`（unittest）中文文案校验并存，改动用户可见文案时注意同步测试。

## 发布流程

由 `.github/workflows/build.yml` 驱动：推送 `main`、推送 `v*` 标签、开 PR 或手动触发都会构建；四个矩阵任务分别用 `windows-2025`、`windows-11-arm`、`macos-15`、`macos-15-intel` 原生运行器，每组顺序为 单测 → npm test → smoke.py → collector 打包 → electron-builder → 打包客户端冒烟（Windows 额外测便携 EXE）。PR 只构建不发布；`release` job 在 main 分支上收集四平台产物，经 `check_release.py` 校验（大小、MZ 头、zip 完整性、app 结构）后由 `publish_release.py` 以**草稿** Release 上传。正式发布必须同时提升 `package.json`、`cablescope/__init__.py` 与 `Cargo.toml`（`[workspace.package] version`）的版本号；已发布版本不会被覆盖。

另设 `.github/workflows/rust-macos.yml`：macos-15 / macos-15-intel 双 runner 跑 `cargo test --workspace --locked` + Python/Rust 快照字段级差分 + Python unittest 回归（不发布产物）。Electron → Tauri 迁移完成后，`build.yml` 的 electron-builder 阶段与 `check_release.py` 的 Electron 产物校验将替换为 Tauri NSIS/dmg 校验。

## 代码风格与约定

- Python 运行时代码只用标准库、不新增第三方依赖（需要类型/结构声明时用 `ctypes`）。Rust workspace 允许引入锁定、审计过的 crate：`Cargo.lock` 入库、`cargo build/test --locked`、禁止 tokio/axum 等异步重生态；移植代码必须复刻 Python 语义（银行家舍入、真值语义、max 并列取第一个、CPython 补偿求和等），用 `py -3` 差分向量固定行为，新增 crate 前证明语义等价。
- 用户可见字符串（界面、错误、诊断）一律中文；模块 docstring 可英文。诊断文案必须陈述证据边界（如"等效估计，不是线材直流电阻实测"），模糊值保留为"未知/保留编码"，不猜测。
- 采集只读是硬性约束：新增后端代码不得引入写命令、注册表修改、角色切换或主动负载。
- 前端无构建链：保持 `cablescope/web/` 为原生 HTML/CSS/JS，不引入打包器；所有进 `innerHTML` 的数据走 `esc()`。
- Electron 渲染进程保持零 Node 权限（`nodeIntegration:false`、`contextIsolation:true`、`sandbox:true`）；Tauri webview 保持最小 capabilities（仅 core 默认，不授予 shell/http/fs 等 JS 权限），子进程只能由 Rust 侧拉起。
- 不要在日志中输出访问令牌、硬件名称或路径（`server.py` 与 `collector.cjs` 均有相关注释，保持这一行为）。
- 保留量上限写在 `engine.py`（5000 采样 / 500 快照 / 50 展示 / 100 事件），调整时同步 README 与 `docs/FEATURES.md`。
- `data/`、`build/`、`dist/`、`node_modules/`、`*.sqlite3*` 均被 gitignore，不要把运行产物提交进仓库。

## 安全注意事项

- HTTP 服务只绑 `127.0.0.1`；`Host` 不匹配、来源不匹配一律 403；本地页面 URL 含随机令牌前缀。
- 响应一律带 CSP（`default-src 'self'` 等）、`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、`Cache-Control: no-store`；新增端点时保持这些头。
- SQLite 全部参数化查询；保存的名称/备注长度有上限并校验类型（布尔不可作为数值）。
- 默认不保存序列号、MAC、主机名；导出的 JSON 报告仍含设备名、VID/PID 与时间，文档已提示用户按需分享——不要静默扩大收集范围。
