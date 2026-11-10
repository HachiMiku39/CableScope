"""Export the shared web interface as a buildless static website."""
import argparse
from pathlib import Path
import shutil

parser = argparse.ArgumentParser()
parser.add_argument("destination", type=Path)
args = parser.parse_args()
source = Path(__file__).resolve().parents[1] / "cablescope/web"
args.destination.mkdir(parents=True, exist_ok=True)
for path in source.iterdir():
    if path.is_file():
        shutil.copy2(path, args.destination / path.name)
print("Static CableScope web interface exported")
