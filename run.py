import argparse
import json
import os
import sys
from pathlib import Path
from cablescope.backends import create
from cablescope.core import now, diagnostics
from cablescope.analysis import enrich


def main():
    for stream in (sys.stdout, sys.stderr):
        if stream is not None and hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description="CableScope · 免费本地线缆诊断")
    parser.add_argument("--json", action="store_true", help="读取一次并输出 JSON，不保存历史")
    parser.add_argument("--no-browser", action="store_true")
    parser.add_argument("--port", type=int, default=47831)
    parser.add_argument("--data-dir", type=Path, help="自选历史数据目录；打包版使用本机应用数据目录")
    args = parser.parse_args()
    backend = create()
    if args.json:
        try:
            data = enrich(backend.sample())
            data["at"] = now()
            data["diagnostics"] = diagnostics(data)
            print(json.dumps(data, ensure_ascii=False, indent=2, allow_nan=False))
        finally:
            if hasattr(backend, "close"):
                backend.close()
        return
    from cablescope.engine import Engine
    from cablescope.server import serve
    if getattr(sys, "frozen", False):
        data_dir = (Path(os.environ.get("LOCALAPPDATA", str(Path.home() / "AppData/Local"))) / "CableScope"
                    if sys.platform == "win32" else Path.home() / "Library/Application Support/CableScope")
    else:
        data_dir = Path(__file__).resolve().parent / "data"
    engine = Engine(backend, (args.data_dir or data_dir) / "history.sqlite3")
    serve(engine, not args.no_browser, args.port)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print("CableScope 无法启动：%s" % exc, file=sys.stderr)
        sys.exit(1)
