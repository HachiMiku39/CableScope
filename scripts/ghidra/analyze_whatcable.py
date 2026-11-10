"""Reproduce targeted, static Ghidra analysis of the supplied WhatCable 1.5.0."""
import argparse
import hashlib
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--ghidra-dir", required=True, type=Path)
parser.add_argument("--app", type=Path, default=Path("/Applications/WhatCable.app"))
parser.add_argument("--java", default="java", help="JDK executable compatible with the selected Ghidra")
parser.add_argument("--output", type=Path, default=Path("work/ghidra"))
args = parser.parse_args()
base = args.output.resolve()
for directory in ("project", "settings", "cache", "tmp", "exports"):
    (base / directory).mkdir(parents=True, exist_ok=True)
exe = args.app / "Contents/MacOS/WhatCable"
expected = "fde60f05b1baf4e25fa9c9c923e3ce27f8c2d1b8db0e188bfb3cf094291179ce"
if hashlib.sha256(exe.read_bytes()).hexdigest() != expected:
    raise SystemExit("Input differs from the analyzed local 1.5.0; re-check the input before reusing this report.")
thin = base / "WhatCable.arm64"
subprocess.run(["/usr/bin/lipo", "-thin", "arm64", str(exe), "-output", str(thin)], check=True)
nm = subprocess.run(["/usr/bin/nm", "-arch", "arm64", "-n", str(exe)], capture_output=True, check=True)
demangled = subprocess.run(["/usr/bin/xcrun", "swift-demangle"], input=nm.stdout, capture_output=True, check=True).stdout.decode()
names = ["SMCPowerReader.open()", "SMCPowerReader.readSystemPowerInput()", "SMCPowerReader.readPortPowerChannels()",
         "SMCPowerReader.readPortContracts()", "SMCPowerReader.(readKey in ", "SMCPowerReader.(callDriver in ",
         "AppleHPMInterfaceWatcher.refresh()", "PowerSourceWatcher.refresh()", "AppleHPMInterfaceWatcher.readPorts",
         "VDMIdentityWatcher.parseUpdate"]
targets = []
for line in demangled.splitlines():
    fields = line.split(" ", 2)
    if len(fields) == 3 and fields[1] in ("T", "t") and any(n in fields[2] for n in names):
        targets.append(fields[0] + "\t" + fields[2])
target_file = base / "targets.tsv"
target_file.write_text("\n".join(targets) + "\n")
command = [args.java, "-Djava.system.class.loader=ghidra.GhidraClassLoader", "-Dfile.encoding=UTF8", "-Djava.awt.headless=true",
           "-Dapplication.settingsdir="+str(base/"settings"), "-Dapplication.cachedir="+str(base/"cache"),
           "-Dapplication.tempdir="+str(base/"tmp"), "-Xmx2G", "-Xshare:off", "--enable-native-access=ALL-UNNAMED",
           "-cp", str(args.ghidra_dir.resolve()/"Ghidra/Framework/Utility/lib/Utility.jar"), "ghidra.Ghidra",
           "ghidra.app.util.headless.AnalyzeHeadless", str(base/"project"), "WhatCable150", "-import", str(thin),
           "-max-cpu", "2", "-analysisTimeoutPerFile", "300", "-scriptPath", str(Path(__file__).resolve().parent),
           "-postScript", "CablePaths.java", str(target_file), str(base/"exports"), "-log", str(base/"analysis.log")]
print("Static analysis only; original app is never executed. Target functions:", len(targets), flush=True)
raise SystemExit(subprocess.call(command))
