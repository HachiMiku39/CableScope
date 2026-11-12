"""Package the native Tauri build as a portable release archive."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / "target" / "release"
DIST = ROOT / "dist" / "desktop"
TARGETS = ("windows-x64", "windows-arm64", "macos-arm64")


def portable_files():
    exe = RELEASE / "CableScope.exe"
    if not exe.is_file():
        raise RuntimeError("Missing Windows executable: " + str(exe))
    files = [exe]
    # A raw Windows build can place runtime DLLs and debug symbols beside the exe.
    for suffix in (".dll", ".pdb"):
        files.extend(sorted(RELEASE.glob("*" + suffix)))
    # Tauri copies the bundled licenses beside the raw Windows exe; carry them
    # into the portable archive.
    licenses = RELEASE / "licenses"
    if not licenses.is_dir():
        raise RuntimeError("Missing bundled licenses: " + str(licenses))
    files.extend(sorted(item for item in licenses.rglob("*") if item.is_file() or item.is_symlink()))
    return files


def add_item(archive, item, base):
    arcname = item.relative_to(base).as_posix()
    if item.is_symlink():
        # Keep symlinks as symlinks; Python's zipfile does not do this automatically.
        info = zipfile.ZipInfo(arcname)
        info.create_system = 3
        info.external_attr = 0o120777 << 16
        archive.writestr(info, item.readlink().as_posix())
    else:
        archive.write(item, arcname)


def package(target):
    version = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))["version"]
    host = "windows" if sys.platform == "win32" else "macos" if sys.platform == "darwin" else sys.platform
    if host != target.split("-")[0]:
        raise RuntimeError("Wrong packaging host: %s for %s" % (sys.platform, target))
    archive = DIST / ("CableScope-%s-%s.zip" % (version, target))
    archive.unlink(missing_ok=True)
    DIST.mkdir(parents=True, exist_ok=True)
    if host == "windows":
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as output:
            for item in portable_files():
                add_item(output, item, RELEASE)
    else:
        app = RELEASE / "bundle" / "macos" / "CableScope.app"
        if not app.is_dir():
            raise RuntimeError("Missing macOS application: " + str(app))
        # ditto preserves executable permissions, symlinks and resource forks in .app bundles.
        # This branch runs on the native macOS runner; Windows cannot validate it locally.
        subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app), str(archive)], check=True)
    print("Packaged", archive, "(%d bytes)" % archive.stat().st_size)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("target", choices=TARGETS)
    args = parser.parse_args()
    package(args.target)


if __name__ == "__main__":
    main()
