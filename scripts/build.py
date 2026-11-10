"""Run on each native GitHub-hosted runner, never cross-compile."""
import argparse
import hashlib
import importlib.metadata
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from cablescope import __version__


def build():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=["windows-x64", "windows-arm64", "macos-arm64", "macos-x64"])
    args = parser.parse_args()
    win = sys.platform == "win32"
    actual = platform.machine().lower()
    expected = "arm64" if args.target.endswith("arm64") else "x64"
    native = "arm64" if actual in ("arm64", "aarch64") else "x64" if actual in ("amd64", "x86_64") else actual
    if native != expected or (args.target.startswith("windows") != win):
        raise RuntimeError("Wrong build host: %s/%s for %s" % (sys.platform, actual, args.target))
    os.chdir(ROOT)
    import PyInstaller.__main__
    PyInstaller.__main__.run(["--noconfirm", "--clean", "--onedir", "--console", "--name", "CableScope",
                             "--collect-submodules", "cablescope.backends", "--add-data", str(ROOT / "cablescope/web") + ":cablescope/web",
                             "--distpath", str(ROOT / "build/runtime"), "--workpath", str(ROOT / "build/pyinstaller"),
                             "--specpath", str(ROOT / "build"), str(ROOT / "run.py")])
    package = ROOT / "build" / ("CableScope-" + args.target)
    if package.exists():
        shutil.rmtree(package)
    package.mkdir(parents=True)
    runtime = ROOT / "build/runtime/CableScope"
    if win:
        shutil.copytree(runtime, package, dirs_exist_ok=True)
        exe = package / "CableScope.exe"
    else:
        contents = package / "CableScope.app/Contents"
        macos, resources = contents / "MacOS", contents / "Resources"
        macos.mkdir(parents=True)
        resources.mkdir()
        shutil.copytree(runtime, resources / "runtime", symlinks=True)
        launcher = macos / "launch"
        launcher.write_text('#!/bin/sh\ncontents="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"\nexec "$contents/Resources/runtime/CableScope" "$@"\n', encoding="utf-8")
        launcher.chmod(0o755)
        (contents / "Info.plist").write_bytes(plistlib.dumps({"CFBundleExecutable": "launch", "CFBundleIdentifier": "org.cablescope.desktop",
            "CFBundleName": "CableScope", "CFBundleDisplayName": "CableScope", "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": __version__, "CFBundleVersion": "1", "LSMinimumSystemVersion": "15.0", "LSUIElement": True}))
        subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(contents.parent)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(contents.parent)], check=True)
        exe = resources / "runtime/CableScope"
    # Test the bundled interpreter, imported backends, assets, HTTP API and shutdown.
    subprocess.run([sys.executable, "scripts/smoke.py", "--executable", str(exe)], check=True, timeout=100)
    for name in ("README.md", "LICENSE", "THIRD_PARTY_NOTICES.md"):
        shutil.copy2(ROOT / name, package / name)
    licenses = package / "licenses"
    licenses.mkdir()
    for candidate in (Path(sys.base_prefix) / "LICENSE.txt", Path(sys.base_prefix) / "LICENSE"):
        if candidate.is_file():
            shutil.copy2(candidate, licenses / "Python-LICENSE.txt")
            break
    distribution = importlib.metadata.distribution("pyinstaller")
    for entry in distribution.files or []:
        if "license" in str(entry).lower() or entry.name == "COPYING.txt":
            src = Path(distribution.locate_file(entry))
            if src.is_file():
                shutil.copy2(src, licenses / ("PyInstaller-" + entry.name))
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    archive = dist / ("CableScope-%s-%s.zip" % (__version__, args.target))
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
        for item in sorted(package.rglob("*")):
            if item.is_file() or item.is_symlink():
                arcname = str(item.relative_to(package.parent)).replace(os.sep, "/")
                if item.is_symlink():
                    info = zipfile.ZipInfo(arcname)
                    info.create_system = 3
                    info.external_attr = (0o120777 << 16)
                    z.writestr(info, os.readlink(item))
                else:
                    z.write(item, arcname)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(".zip.sha256").write_text(checksum + "  " + archive.name + "\n", encoding="ascii")
    print("Built", archive.name, "sha256", checksum)


if __name__ == "__main__":
    build()
