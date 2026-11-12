# CableScope · AGENTS.md

本文件面向 AI 编码代理，假设读者对本项目一无所知。所有信息均来自仓库实际内容，修改代码导致以下描述失准时请同步更新本文件。

## 项目概述

CableScope（线缆实验室）是免费、MIT 开源的本地 USB-C 连接观察与诊断工具，支持 Windows 10/11（x64 / ARM64）和 macOS 15+（仅 Apple Silicon，Intel Mac 已停止支持），界面与文档语言为中文。

产品形态为**纯桌面 App**：网页版与外部网页配对、Electron 壳与 Python 采集程序（`run.py`、`cablescope/` 包、`CABLESCOPE_COLLECTOR=python` sidecar 回退）均已移除。当前唯一形态：Tauri 2.x 壳（`src-tauri/`）+ Rust 原生采集引擎；前端经 `window.__TAURI__.core.invoke("api_request")` 内部 IPC 取数，无任何 HTTP 服务。网页 UI 文件（`web/`）编译期嵌入二进制，由 Tauri 壳经 `cablescope://` 自定义协议伺服。分发为三平台便携 zip（无安装器；Windows 依赖系统 WebView2 运行时）。运行时无需账户、不向外部服务上传设备信息。

后端已全部为 Rust（根 `Cargo.toml` workspace + `crates/` + `src-tauri/`）：`core`（数值清洗/PD3 VDO 解码/诊断 + `py` 语义兼容层）、`analysis`（诊断/回归/enrich）、`engine`（rusqlite 存储 + 采样线程）、`backends-traits`、`backends-windows`、`backends-macos`+`smc`（FFI 手写 extern "C"，编译期 ABI 断言；CI macos-26 全绿）、`cli`（二进制名 `CableScope`，`--json` 单次采集）。昔日的 `server` crate（loopback HTTP 占位）与 Python 实现一并删除。

数据采集完全只读：不修改注册表、不写 SMC、不重启控制器、不切换供电角色、不安装驱动、不主动制造负载。所有诊断结论必须基于证据，证据不足时保持"未知"，不生成"线缆是瓶颈"之类的推断。

## 目录与模块划分

| 路径 | 作用 |
|---|---|
| `web/` | 无构建步骤的原生 JS 界面：`index.html`、`style.css`、`app.js`（渲染与图表）、`connection.js`（两种 transport：Tauri IPC `cablescope://` origin、离线报告导入）。任何用户数据进 DOM 前必须经过 `esc()` 转义 |
| `src-tauri/` | Tauri 2 壳（根 workspace 成员，发布形态）。Rust 原生 `cablescope-engine` 采样 + 前端 IPC（`engine_cmd.rs` 的 `api_request` 通用端点按 path/method 分发 state/save/report/csv/quit，复刻原 engine.py 校验与中文文案；`native.rs` 以 `cablescope://` 自定义协议伺服**编译期嵌入**的四个前端文件——便携包不得依赖构建机路径）。单实例插件；capabilities 仅 core 默认 + `withGlobalTauri`；CSP `connect-src 'self' ipc: http://ipc.localhost`；`build.rs` 断言 package.json ↔ tauri.conf.json ↔ 根 workspace 版本一致；分发为便携 zip（targets 仅 `app`，无安装器） |
| `scripts/smoke_tauri.cjs` | 对 Tauri exe（无参默认根 `target/release`，或显式传入解压产物路径）跑 `--smoke-test`：断言 `mode:"ipc"`、真实渲染窗口经 IPC 往返、保存/报告、非空 devices/ports（真机硬件覆盖）；CI 对解压后的 zip 产物跑 |
| `scripts/package_tauri.py` | 把 `target/release` 产物打成便携 zip `dist/desktop/CableScope-{版本}-{target}.zip`：Windows 收 exe+相邻 dll/pdb+licenses/；macOS 用 `ditto -c -k --keepParent` 保留 .app 权限与符号链接 |
| `scripts/test_web.cjs` | 无浏览器渲染测试：用 `vm` 加载前端 JS，校验 UI id、XSS 转义、图表与筛选；含 Tauri IPC transport 模式 |
| `scripts/check_release.py` / `scripts/publish_release.py` | 发布前校验三份便携 zip（大小下限 4 MiB、Windows MZ 头、macOS .app 内可执行文件）并生成 `SHA256SUMS.txt`；仅以草稿 Release 上传，已发布版本不可覆盖。Python 编写，仅作构建工具 |
| `licenses/` | WhatCable（SMC ABI 参考来源，见 `docs/WHATCABLE_ANALYSIS.md`）许可证，随包分发 |
| `docs/` | 功能与实测范围、发布说明、WhatCable 静态分析、Ghidra 调用路径复核 |
| `Cargo.toml` / `crates/` | Rust 后端：`core`（数值清洗/PD3 VDO 解码/诊断 + `py` 语义兼容层）、`analysis`（诊断/回归/enrich）、`engine`（rusqlite 存储 + 采样线程）、`backends-traits`（`Backend` trait）、`backends-windows`、`backends-macos`+`smc`、`cli`（二进制名 `CableScope`，`--json`）。FFI 策略：手写 `extern "C"` 镜像原 ctypes 布局，不引 windows-sys/core-foundation；macOS 代码经 `cargo check --target aarch64-apple-darwin` 类型检查 |

数据库位置：系统应用数据目录（`%LOCALAPPDATA%\CableScope` 或 `~/Library/Application Support/CableScope`）。

## 构建与测试命令

环境要求：Node 24 与 Rust stable（本机 1.97）。打包/发布脚本（`package_tauri.py` 等）用 Python 3 标准库编写，仅 CI 与本机打包时需要，不是运行依赖。

```sh
# 测试
cargo test --workspace --locked          # Rust 单元测试（合成数据；workspace 含 src-tauri）
npm ci && npm test                       # 网页渲染测试（渲染 + Tauri IPC 两分支）

# macOS 代码跨平台类型检查（无需 macOS SDK）
cargo check --target aarch64-apple-darwin -p cablescope-smc -p cablescope-backends-macos -p cablescope-cli

# Tauri 壳（Rust 原生 Engine + IPC）
npm ci && npm run desktop:dev             # 开发
npm run desktop:build                     # 出 target/release 原生 exe/app
python scripts/package_tauri.py windows-x64   # 打便携 zip 到 dist/desktop（或 windows-arm64 / macos-arm64）
node scripts/smoke_tauri.cjs              # 根 target/release 冒烟（mode 必须为 ipc）

# 单次 JSON 采集（不经界面）
cargo run -p cablescope-cli -- --json
```

## 测试策略

- 单元测试必须可在无硬件的 GitHub 运行器上通过：所有平台结构体布局、解码、拟合门槛都用合成数据验证，禁止依赖真实 USB 配件。
- `node scripts/smoke_tauri.cjs` 会启动真实构建产物（真实引擎采样 + 渲染窗口），是唯一接触硬件的自动化测试；CI 明确声明"构建与启动成功不能证明硬件覆盖"。
- Rust 引擎的语义以 `crates/core` 的 `py` 兼容层固定（银行家舍入、真值、max 并列取第一个、CPython 补偿求和等）；插外设才有的数据（E-Marker/PD 合同）runner 覆盖不了，文档已声明。
- 前端渲染测试（`scripts/test_web.cjs`）不经过浏览器布局，只验证 DOM id 存在、内容转义与逻辑分支。
- 集成断言风格：smoke 脚本与 Rust 测试中的中文文案校验并存，改动用户可见文案时注意同步测试。

## 发布流程

由 `.github/workflows/build.yml` 驱动（唯一的构建/发布 workflow）：推送 `main`、推送 `v*` 标签、开 PR 或手动触发都会构建；三个矩阵任务分别用 `windows-2025`、`windows-11-arm`、`macos-26` 原生运行器（macOS 仅 Apple Silicon，Intel 已停止支持），每组顺序为 `cargo test --workspace --locked` → npm test → `tauri build` → `package_tauri.py` 打便携 zip → 解压产物跑 `smoke_tauri.cjs`（mode 必须为 ipc）。PR 只构建不发布；`release` job 在 main 分支上收集三平台 zip，经 `check_release.py` 校验（大小下限、MZ 头、zip 完整性、.app 结构）后由 `publish_release.py` 以**草稿** Release 上传。正式发布必须同时提升 `package.json`、`src-tauri/tauri.conf.json` 与 `Cargo.toml`（`[workspace.package] version`）的版本号（build.rs 强制校验一致）；已发布版本不会被覆盖。

## 代码风格与约定

- Rust workspace 允许引入锁定、审计过的 crate：`Cargo.lock` 入库、`cargo build/test --locked`、禁止 tokio/axum 等异步重生态；数值语义必须与 `core` 的 `py` 兼容层保持一致（银行家舍入、真值语义、max 并列取第一个、CPython 补偿求和等）。
- 用户可见字符串（界面、错误、诊断）一律中文；模块 docstring 可英文。诊断文案必须陈述证据边界（如"等效估计，不是线材直流电阻实测"），模糊值保留为"未知/保留编码"，不猜测。
- 采集只读是硬性约束：新增后端代码不得引入写命令、注册表修改、角色切换或主动负载。
- 前端无构建链：保持 `web/` 为原生 HTML/CSS/JS，不引入打包器；所有进 `innerHTML` 的数据走 `esc()`。
- Tauri webview 保持最小 capabilities（仅 core 默认，不授予 shell/http/fs 等 JS 权限）；前端仅在 `cablescope://` origin 且全局 Tauri API 存在时启用 IPC transport。
- 不要在日志中输出硬件名称或路径。
- 保留量上限写在 `crates/engine`（5000 采样 / 500 快照 / 50 展示 / 100 事件），调整时同步 README 与 `docs/FEATURES.md`。
- `build/`、`dist/`、`node_modules/`、`*.sqlite3*` 均被 gitignore，不要把运行产物提交进仓库。

## 安全注意事项

- Tauri 路径无 HTTP：前端仅在 `cablescope://` 自定义协议 origin 且全局 Tauri API 存在时启用 IPC；`api_request` 是唯一 command 面，按 path/method 白名单分发，保存载荷校验长度与类型（布尔不可作为数值）。
- 静态响应一律带 CSP（`default-src 'self'` 等）、`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、`Cache-Control: no-store`；新增端点时保持这些头。
- SQLite 全部参数化查询；保存的名称/备注长度有上限并校验类型。
- 默认不保存序列号、MAC、主机名；导出的 JSON 报告仍含设备名、VID/PID 与时间，文档已提示用户按需分享——不要静默扩大收集范围。
