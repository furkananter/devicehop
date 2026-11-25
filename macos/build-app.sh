#!/usr/bin/env bash
# Builds build/DeviceHop.app (ad-hoc signed). With --install, copies it to ~/Applications.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(dirname "$HERE")"
APP="$ROOT/build/DeviceHop.app"

install=0
case "${1:-}" in
    "") ;;
    --install) install=1 ;;
    *) echo "usage: $0 [--install]" >&2; exit 2 ;;
esac

cargo build --release --manifest-path "$ROOT/Cargo.toml"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"

swiftc -O -swift-version 6 -target "$(uname -m)-apple-macos13.0" \
    "$HERE/DeviceHop/main.swift" -o "$APP/Contents/MacOS/DeviceHop" \
    -framework AppKit -framework Carbon -framework IOKit -framework ServiceManagement

# Named devicehop-cli: `devicehop` would collide with `DeviceHop` on a case-insensitive volume.
cp "$ROOT/target/release/devicehop" "$APP/Contents/MacOS/devicehop-cli"
cp "$HERE/Info.plist" "$APP/Contents/Info.plist"
mkdir -p "$APP/Contents/Resources"
cp "$HERE/icon/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
if [ -f "$HERE/icon/MenuBarIcon.pdf" ]; then
    cp "$HERE/icon/MenuBarIcon.pdf" "$APP/Contents/Resources/MenuBarIcon.pdf"
fi
cp "$ROOT/LICENSE" "$ROOT/THIRD-PARTY-NOTICES.md" "$APP/Contents/Resources/"

codesign --force --sign - "$APP/Contents/MacOS/devicehop-cli"
codesign --force --sign - "$APP"
codesign --verify --deep --strict "$APP"

if [ "$install" -eq 1 ]; then
    pkill -x DeviceHop || true
    mkdir -p "$HOME/Applications"
    rm -rf "$HOME/Applications/DeviceHop.app"
    cp -R "$APP" "$HOME/Applications/DeviceHop.app"
    echo "Installed: $HOME/Applications/DeviceHop.app"
else
    echo "Built: $APP"
fi
