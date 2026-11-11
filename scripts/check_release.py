"""Verify and hash the three desktop assets before publication."""
import hashlib
import json
from pathlib import Path
import sys
import zipfile


def check(folder):
    version = json.loads(Path("package.json").read_text())["version"]
    expected = ["CableScope-%s-windows-%s.exe" % (version, arch) for arch in ("x64", "arm64")]
    expected += ["CableScope-%s-macos-arm64.zip" % version]
    hashes = []
    for name in expected:
        path = folder / name
        assert path.stat().st_size > 10000000, "Incomplete desktop package: " + name
        if name.endswith(".exe"):
            with path.open("rb") as stream:
                assert stream.read(2) == b"MZ", "Not a Windows executable"
        else:
            with zipfile.ZipFile(path) as archive:
                assert archive.testzip() is None
                assert "CableScope.app/Contents/MacOS/CableScope" in archive.namelist()
                assert "CableScope.app/Contents/Resources/collector/CableScope" in archive.namelist()
        hashes.append(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + name)
    (folder / "SHA256SUMS.txt").write_text("\n".join(hashes) + "\n", encoding="ascii")
    print("Three desktop assets verified and hashed.")


if __name__ == "__main__":
    check(Path(sys.argv[1]))
