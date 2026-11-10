免费开源的 Electron 客户端，内置本机采集程序，不用安装 Python。

| 平台 | 文件 |
|---|---|
| Windows x64 | windows-x64.exe，直接运行 |
| Windows ARM64 | windows-arm64.exe，原生 ARM64，直接运行 |
| Mac Apple Silicon | macos-arm64.zip，解压后把 CableScope.app 拖入应用程序 |
| Mac Intel | macos-x64.zip，同上 |

Mac 要求 macOS 15+；Windows 要求 Windows 10/11 x64 或 Windows 11 ARM64。Windows EXE 为便携自解压包，第一次启动需要解包时间。四个版本均通过原生云编译、打包程序启动、网页渲染、采集、保存和报告检查。

[网页入口](https://cablescope-lab.leisurelyhorse4.chatgpt.site/)已公开。设备数据保存在本机；网页连接需要本机一次性配对码。客户端内直接显示采集结果，关闭客户端会停止其采集程序。没有账户、付费授权或订阅。

功能包括 Mac SMC 功率与端口曲线、协商诊断、引脚路由、MagSafe 结构示意、两平台显示模式比较、线缆记录和 JSON/CSV。Windows 深层 PD/E-Marker、完整 DSC、逐针健康检测尚未实现；路径电阻仅为条件满足后的等效估计。

当前没有商业代码签名和 Apple 公证。Mac 使用临时签名，系统可能要求在“隐私与安全性”中允许打开已确认来源的应用；Windows 可能显示未知发布者。可使用 SHA256SUMS.txt 核验下载文件。
