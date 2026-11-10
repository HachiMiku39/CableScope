import sys


def create():
    if sys.platform == "darwin":
        from .macos import Backend
    elif sys.platform == "win32":
        from .windows import Backend
    else:
        raise RuntimeError("CableScope 首版支持 macOS 和 Windows。")
    return Backend()
