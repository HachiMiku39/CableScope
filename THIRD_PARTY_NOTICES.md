# Third-party references and distribution

CableScope is an independent Python implementation. Its name, interface and runtime code are distinct from WhatCable. It does not ship WhatCable's executable, assets, database, activation code, or proprietary plugins.

The USB-PD field interpretation and macOS IORegistry service names were checked against public documentation and the publicly available MIT-default portion of [WhatCable](https://github.com/darrylmorley/whatcable). That repository explicitly reserves proprietary subtrees under separate licences. Those subtrees were not used. See [WhatCable's licence](https://github.com/darrylmorley/whatcable/blob/main/LICENSE).

The Windows implementation uses the public structures and queries described in [Microsoft USBView](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/usbview) and [Win32 metadata headers](https://github.com/microsoft/win32metadata/tree/main/generation/WinSDK/RecompiledIdlHeaders/shared). No Microsoft sample implementation is bundled.

Binary packages include the Python runtime under the [Python Software Foundation licence](https://docs.python.org/3/license.html). PyInstaller's bootloader is distributed under its [GPL licence with the bootloader exception](https://pyinstaller.org/en/stable/license.html), which permits bundling independently licensed applications. The packaging script copies available installed Python and PyInstaller licence files into the package's `licenses` directory.

CableScope's own code is MIT licensed. System frameworks and APIs belong to their respective owners.
