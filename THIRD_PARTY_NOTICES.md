# Third-party references and distribution

CableScope is an independent Python implementation. Its name, interface and runtime code are distinct from WhatCable. It does not ship WhatCable's executable, assets, database, activation code, or proprietary plugins.

The USB-PD field interpretation and macOS IORegistry service names were checked against public documentation and the publicly available MIT-default portion of [WhatCable](https://github.com/darrylmorley/whatcable). Version 0.3 adapts the AppleSMC ABI, power-key interpretation, controller UUID joining and USB-C pin routing rules from Darryl Morley's MIT-licensed `SMCPowerReader.swift`, `HPMPortUUIDMap.swift` and `USBCPinMap.swift` at commit `58e588ab45350cf9512ce40d4080fbbef3b94ad0`. Copyright (c) 2026 Darryl Morley. The full notice is included in `licenses/WhatCable-MIT.txt` and copied into binary packages. That repository explicitly reserves proprietary subtrees under separate licences; those subtrees were not used. See [WhatCable's licence](https://github.com/darrylmorley/whatcable/blob/main/LICENSE).

The Windows implementation uses the public structures and queries described in [Microsoft USBView](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/usbview) and [Win32 metadata headers](https://github.com/microsoft/win32metadata/tree/main/generation/WinSDK/RecompiledIdlHeaders/shared). No Microsoft sample implementation is bundled.

Binary packages include the Python runtime under the [Python Software Foundation licence](https://docs.python.org/3/license.html). PyInstaller's bootloader is distributed under its [GPL licence with the bootloader exception](https://pyinstaller.org/en/stable/license.html), which permits bundling independently licensed applications. The packaging script copies available installed Python and PyInstaller licence files into the package's `licenses` directory.

CableScope's own code is MIT licensed. System frameworks and APIs belong to their respective owners.

Version 0.4 embeds Electron, distributed under its MIT licence and Chromium/Node.js third-party notices. electron-builder preserves Electron's `LICENSE` and `LICENSES.chromium.html` in the application distribution. The embedded collector includes the Python, PyInstaller and WhatCable MIT notices under its `licenses` directory.
