# CableScope · AGENTS.md

本文件面向 AI 编码代理，假设读者对本项目一无所知。所有信息均来自仓库实际内容，修改代码导致以下描述失准时请同步更新本文件。

## 项目概述

CableScope（线缆实验室）是免费、MIT 开源的本地 USB-C 连接观察与诊断工具，支持 Windows 10/11（x64 / ARM64）和 macOS 15+（仅 Apple Silicon，Intel Mac 已停止支持），界面与文档语言为中文。

产品形态为**纯桌面 App**：网页版与外部网页配对（`/bridge/`、`pairing.py`、`site.json`、`scripts/prepare_web.py`、菜单「打开网页版」）已从代码中移除，Electron 壳（`desktop/`）也已退役删除。网页 UI 文件（`cablescope/web/`）保留为应用窗口内嵌界面：Tauri 壳经 `cablescope://` 自定义协议直接伺服这些静态文件并经内部 IPC 取数。

核心形态：Tauri 2.x 壳（`src-tauri/`）+ Rust 原生采集引擎为发布形态；纯 Python 3.9+ 采集程序（仅标准库，无第三方运行依赖）保留为 `run.py` 源码运行方式与 `CABLESCOPE_COLLECTOR=python` 回退路径。默认直接用 Rust `cablescope-engine` 原生采样，前端经 `window.__TAURI__.core.invoke("api_request")` 内部 IPC 取数，不经 HTTP；设 `CABLESCOPE_COLLECTOR=python` 可回退到 Python sidecar + 令牌回环 URL。分发为三平台便携 zip（无安装器；Windows 依赖系统 WebView2 运行时）。运行时无需账户、不向外部服务上传设备信息。

后端正在分波次重写为 Rust（根 `Cargo.toml` workspace + `crates/` + `src-tauri/`）：`core`（数值清洗/PD3 VDO 解码/诊断 + `py` 语义兼容层）、`analysis`（诊断/回归/enrich）、`engine`（rusqlite 存储 + 采样线程）、`backends-traits`、`backends-windows`、`backends-macos`+`smc`（FFI 手写 extern "C"，编译期 ABI 断言；CI macos-26 全绿）、`cli`（`--json` 单次采集，Windows 真机字段级差分零差异）已移植并接入 Tauri。`server` crate 为占位（纯 App 形态下不需要 loopback HTTP 服务，计划移除）。

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
| `cablescope/web/` | 无构建步骤的原生 JS 界面：`index.html`、`style.css`、`app.js`（渲染与图表）、`connection.js`（三种 transport：Tauri IPC `cablescope://` origin、本机 HTTP 令牌 URL（Python run.py）、离线导入）。任何用户数据进 DOM 前必须经过 `esc()` 转义 |
| `src-tauri/` | Tauri 2 壳（根 workspace 成员，发布形态）。默认路径：Rust 原生 `cablescope-engine` 采样 + 前端 IPC（`engine_cmd.rs` 的 `api_request` 通用端点按 path/method 分发 state/save/report/csv/quit，复刻 engine.py 校验与中文文案；`native.rs` 以 `cablescope://` 自定义协议伺服**编译期嵌入**的四个前端文件——便携包不得依赖构建机路径）。`CABLESCOPE_COLLECTOR=python` 回退到 Python sidecar 令牌 URL（`collector.rs` 为 collector.cjs 协议 Rust 移植）。单实例插件；capabilities 仅 core 默认 + `withGlobalTauri`；CSP `connect-src 'self' ipc: http://ipc.localhost`；`build.rs` 断言 package.json ↔ `__init__.py` ↔ 根 workspace 版本一致，并为 bundle 资源创建占位目录使干净 checkout 的 `cargo test` 可过；分发为便携 zip（targets 仅 `app`，无安装器） |
| `scripts/build.py` | PyInstaller 打包原生采集程序；`--collector-only` 供桌面壳内嵌；只允许在本机架构/系统运行（拒绝交叉编译），打包后自动跑 `smoke.py` |
| `scripts/smoke.py` | 真实后端 + 真实 HTTP 进程冒烟：快照、静态资源、保存/报告、CSV、跨源 403、退出码 |
| `scripts/smoke_tauri.cjs` | 对 Tauri exe（无参默认根 `target/release`，或显式传入解压产物路径）跑 `--smoke-test`：断言 `mode:"ipc"`、真实渲染窗口经 IPC 往返、保存/报告、非空 devices/ports（真机硬件覆盖）；CI 对解压后的 zip 产物跑 |
| `scripts/package_tauri.py` | 把 `target/release` 产物打成便携 zip `dist/desktop/CableScope-{版本}-{target}.zip`：Windows 收 exe+相邻 dll/pdb+collector/+licenses/；macOS 用 `ditto -c -k --keepParent` 保留 .app 权限与符号链接 |
| `scripts/diff_rust_python.py` | Python 版与 Rust CLI `--json` 快照的字段级差分（忽略 `at`；数组顺序无关比较），CI 与本地通用 |
| `scripts/test_web.cjs` | 无浏览器渲染测试：用 `vm` 加载前端 JS，校验 UI id、XSS 转义、图表与筛选；含 Tauri IPC 与 HTTP 回退 transport 模式 |
| `scripts/check_release.py` / `scripts/publish_release.py` | 发布前校验三份便携 zip（大小下限 4 MiB、Windows MZ 头、macOS .app 内可执行文件）并生成 `SHA256SUMS.txt`；仅以草稿 Release 上传，已发布版本不可覆盖 |
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
npm ci && npm test                        # 网页渲染测试（HTTP / Tauri IPC / 回退三分支）
python scripts/smoke.py                   # 真实后端 + 真实服务进程冒烟

# Rust 后端 + Tauri 壳（98 个单元测试，合成数据；workspace 含 src-tauri）
cargo test --workspace --locked
# Python ↔ Rust 真机差分（CI macOS job 同款）
python3 run.py --json > py.json && cargo run -q -p cablescope-cli -- --json > rust.json
python3 scripts/diff_rust_python.py py.json rust.json

# macOS 代码跨平台类型检查（无需 macOS SDK）
cargo check --target aarch64-apple-darwin -p cablescope-smc -p cablescope-backends-macos -p cablescope-cli

# Tauri 壳（默认 Rust 原生 Engine + IPC，无需 Python）
npm ci && npm run desktop:dev             # 开发
npm run desktop:build                     # 出 target/release 原生 exe/app（仍内嵌 collector 资源供 python 回退）
python scripts/package_tauri.py windows-x64   # 打便携 zip 到 dist/desktop（或 windows-arm64 / macos-arm64）
node scripts/smoke_tauri.cjs              # 根 target/release 冒烟（mode 必须为 ipc）
CABLESCOPE_COLLECTOR=python node scripts/smoke_tauri.cjs   # 显式验证 Python sidecar 回退（mode 为 python-collector）

# 打包（只能在对应平台的原生机器上运行，CI 中执行）
python -m pip install -r requirements-build.txt
python scripts/build.py --target windows-x64 --collector-only   # 或 windows-arm64 / macos-arm64
```

## 测试策略

- 单元测试必须可在无硬件的 GitHub 运行器上通过：所有平台结构体布局、解码、拟合门槛都用合成数据验证，禁止依赖真实 USB 配件。
- `python scripts/smoke.py` 会启动真实采集进程并监听端口，是唯一接触硬件的自动化测试；CI 明确声明"构建与启动成功不能证明硬件覆盖"。
- Rust 移植以 `py -3` 差分向量固定 Python 语义（银行家舍入、真值、max 并列取第一个、CPython 补偿求和等）；Windows 后端已做真机字段级差分（`scripts/diff_rust_python.py`），macOS 后端经 build.yml 的 macos-26 job 真机构建、单测与差分全绿；插外设才有的数据（E-Marker/PD 合同）runner 覆盖不了，文档已声明。
- 前端渲染测试（`scripts/test_web.cjs`）不经过浏览器布局，只验证 DOM id 存在、内容转义与逻辑分支。
- 集成断言风格：大量 `assert`（smoke 脚本）与 `assertEqual/assertRaises`（unittest）中文文案校验并存，改动用户可见文案时注意同步测试。

## 发布流程

由 `.github/workflows/build.yml` 驱动（唯一的构建/发布 workflow）：推送 `main`、推送 `v*` 标签、开 PR 或手动触发都会构建；三个矩阵任务分别用 `windows-2025`、`windows-11-arm`、`macos-26` 原生运行器（macOS 仅 Apple Silicon，Intel 已停止支持），每组顺序为 单测 → npm test → smoke.py →（macOS 额外：`cargo test --workspace --locked` + Python/Rust 快照字段级差分）→ collector 打包（Python 回退资源）→ `tauri build` → `package_tauri.py` 打便携 zip → 解压产物跑 `smoke_tauri.cjs`（mode 必须为 ipc）。PR 只构建不发布；`release` job 在 main 分支上收集三平台 zip，经 `check_release.py` 校验（大小下限、MZ 头、zip 完整性、.app 结构）后由 `publish_release.py` 以**草稿** Release 上传。正式发布必须同时提升 `package.json`、`cablescope/__init__.py` 与 `Cargo.toml`（`[workspace.package] version`）的版本号；已发布版本不会被覆盖。

## 代码风格与约定

- Python 运行时代码只用标准库、不新增第三方依赖（需要类型/结构声明时用 `ctypes`）。Rust workspace 允许引入锁定、审计过的 crate：`Cargo.lock` 入库、`cargo build/test --locked`、禁止 tokio/axum 等异步重生态；移植代码必须复刻 Python 语义（银行家舍入、真值语义、max 并列取第一个、CPython 补偿求和等），用 `py -3` 差分向量固定行为，新增 crate 前证明语义等价。
- 用户可见字符串（界面、错误、诊断）一律中文；模块 docstring 可英文。诊断文案必须陈述证据边界（如"等效估计，不是线材直流电阻实测"），模糊值保留为"未知/保留编码"，不猜测。
- 采集只读是硬性约束：新增后端代码不得引入写命令、注册表修改、角色切换或主动负载。
- 前端无构建链：保持 `cablescope/web/` 为原生 HTML/CSS/JS，不引入打包器；所有进 `innerHTML` 的数据走 `esc()`。
- Tauri webview 保持最小 capabilities（仅 core 默认，不授予 shell/http/fs 等 JS 权限），子进程只能由 Rust 侧拉起；前端仅在 `cablescope://` origin 启用 IPC transport。
- 不要在日志中输出访问令牌、硬件名称或路径（`server.py` 与 `collector.cjs` 均有相关注释，保持这一行为）。
- 保留量上限写在 `engine.py`（5000 采样 / 500 快照 / 50 展示 / 100 事件），调整时同步 README 与 `docs/FEATURES.md`。
- `data/`、`build/`、`dist/`、`node_modules/`、`*.sqlite3*` 均被 gitignore，不要把运行产物提交进仓库。

## 安全注意事项

- HTTP 服务只绑 `127.0.0.1`；`Host` 不匹配、来源不匹配一律 403；本地页面 URL 含随机令牌前缀。
- Tauri 路径无 HTTP：前端仅在 `cablescope://` 自定义协议 origin 且全局 Tauri API 存在时启用 IPC；`api_request` 是唯一 command 面，按 path/method 白名单分发，保存载荷复刻 Python 侧长度/类型校验。
- 响应一律带 CSP（`default-src 'self'` 等）、`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、`Cache-Control: no-store`；新增端点时保持这些头。
- SQLite 全部参数化查询；保存的名称/备注长度有上限并校验类型（布尔不可作为数值）。
- 默认不保存序列号、MAC、主机名；导出的 JSON 报告仍含设备名、VID/PID 与时间，文档已提示用户按需分享——不要静默扩大收集范围。
