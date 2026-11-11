"""Verify and hash the three portable desktop archives before publication."""
import hashlib
import json
from pathlib import Path
import sys
import zipfile

MIN_SIZE = 4 * 1024 * 1024


def check(folder):
    version = json.loads(Path("package.json").read_text())["version"]
    expected = ["CableScope-%s-windows-%s.zip" % (version, arch) for arch in ("x64", "arm64")]
    expected += ["CableScope-%s-macos-arm64.zip" % version]
    hashes = []
    for name in expected:
        path = folder / name
        assert path.is_file(), "Missing desktop package: " + name
        assert path.stat().st_size > MIN_SIZE, "Incomplete desktop package: " + name
        with zipfile.ZipFile(path) as archive:
            assert archive.testzip() is None, "Corrupt desktop package: " + name
            names = archive.namelist()
            if name.endswith("windows-x64.zip") or name.endswith("windows-arm64.zip"):
                assert archive.read("CableScope.exe")[:2] == b"MZ", "Not a Windows executable: " + name
            else:
                assert "CableScope.app/Contents/MacOS/CableScope" in names, "Missing macOS executable: " + name
        hashes.append(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + name)
    (folder / "SHA256SUMS.txt").write_text("\n".join(hashes) + "\n", encoding="ascii")
    print("Three portable desktop assets verified and hashed.")


if __name__ == "__main__":
    check(Path(sys.argv[1]))
