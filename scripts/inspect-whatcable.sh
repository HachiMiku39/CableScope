#!/bin/sh
# Static inspection only: never execute the target app or its helpers.
set -u
app=${1:-/Applications/WhatCable.app}
exe="$app/Contents/MacOS/WhatCable"
if [ ! -f "$exe" ]; then
  printf 'Missing WhatCable executable: %s\n' "$exe" >&2
  exit 1
fi
printf '\nBundle metadata\n'
/usr/libexec/PlistBuddy -c Print "$app/Contents/Info.plist"
printf '\nArchitecture and hashes\n'
/usr/bin/file "$exe"
/usr/bin/shasum -a 256 "$exe"
if [ -f "$app/Contents/Helpers/whatcable" ]; then
  /usr/bin/file "$app/Contents/Helpers/whatcable"
  /usr/bin/shasum -a 256 "$app/Contents/Helpers/whatcable"
fi
printf '\nSignature metadata (not a verification result)\n'
/usr/bin/codesign -dv --verbose=4 "$app" 2>&1 || true
printf '\nStrict signature verification\n'
/usr/bin/codesign --verify --deep --strict "$app" 2>&1 || true
printf '\nEntitlements\n'
/usr/bin/codesign -d --entitlements :- "$app" 2>&1 || true
printf '\nDependencies\n'
/usr/bin/otool -arch arm64 -L "$exe"
printf '\nRelevant imports\n'
/usr/bin/nm -arch arm64 -u "$exe" | /usr/bin/grep -E 'IOService|IORegistry|IOConnect|IOPS|CGDisplay|sqlite3' || true
printf '\nSelected service strings\n'
/usr/bin/strings -a "$exe" | /usr/bin/grep -E '^(AppleHPMInterfaceType[0-9]+|AppleTCControllerType[0-9]+|IOPortTransportComponentCCUSBPDSOPp*|IOPortFeaturePowerSource|AppleSMC|PowerTelemetryData)$' | /usr/bin/sort -u
printf '\nSelected Swift symbol evidence\n'
/usr/bin/nm -arch arm64 -j "$exe" | /usr/bin/xcrun swift-demangle | /usr/bin/grep -E '^WhatCableDarwinBackend.SMCPowerReader\.(read|open|close)|identityWatcher.*WhatCableDarwinBackend.VDMIdentityWatcher' | /usr/bin/head -25
