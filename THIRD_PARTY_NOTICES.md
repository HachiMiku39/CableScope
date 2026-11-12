# Third-party references and distribution

CableScope is an independent implementation (Rust engine plus a Tauri desktop shell). Its name, interface and runtime code are distinct from WhatCable. It does not ship WhatCable's executable, assets, database, activation code, or proprietary plugins.

The USB-PD field interpretation and macOS IORegistry service names were checked against public documentation and the publicly available MIT-default portion of [WhatCable](https://github.com/darrylmorley/whatcable). Version 0.3 adapts the AppleSMC ABI, power-key interpretation, controller UUID joining and USB-C pin routing rules from Darryl Morley's MIT-licensed `SMCPowerReader.swift`, `HPMPortUUIDMap.swift` and `USBCPinMap.swift` at commit `58e588ab45350cf9512ce40d4080fbbef3b94ad0`. Copyright (c) 2026 Darryl Morley. The full notice is included in `licenses/WhatCable-MIT.txt` and copied into binary packages. That repository explicitly reserves proprietary subtrees under separate licences; those subtrees were not used. See [WhatCable's licence](https://github.com/darrylmorley/whatcable/blob/main/LICENSE).

The Windows implementation uses the public structures and queries described in [Microsoft USBView](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/usbview) and [Win32 metadata headers](https://github.com/microsoft/win32metadata/tree/main/generation/WinSDK/RecompiledIdlHeaders/shared). No Microsoft sample implementation is bundled.

CableScope's own code is MIT licensed. System frameworks and APIs belong to their respective owners.

The desktop shell is built with [Tauri](https://tauri.app) (MIT) and Rust crates from crates.io; their licence texts are embedded in the shipped binaries. Binary packages carry the WhatCable MIT notice under the `licenses` directory. The Windows build uses the system WebView2 runtime, which is not redistributed.
