#!/usr/bin/env bash
# devicehop installer for Linux (Debian/Ubuntu and other systemd distros).
#
# Builds devicehop, installs the udev rule that lets your user talk to
# Logitech Bluetooth devices, and sets up the follow service so that pressing
# Easy-Switch on one device makes the others follow. Run it as your normal
# user from anywhere inside the repository; it asks for sudo only for the
# package install and the udev rule.
#
# STATUS: not yet verified on hardware.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
bin="$HOME/.cargo/bin/devicehop"
unit_dir="$HOME/.config/systemd/user"
unit="$unit_dir/devicehop-follow.service"

say() { printf '\n==> %s\n' "$*"; }

if [ "$(id -u)" -eq 0 ]; then
    echo "Run this as your normal user, not with sudo." >&2
    exit 1
fi

say "Checking build requirements"
if command -v apt-get >/dev/null 2>&1; then
    missing=()
    for pkg in build-essential pkg-config libudev-dev; do
        dpkg -s "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
    done
    if [ "${#missing[@]}" -gt 0 ]; then
        echo "Installing: ${missing[*]}"
        sudo apt-get install -y "${missing[@]}"
    fi
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust is not installed. Install it from https://rustup.rs (or 'sudo apt install cargo') and run this again." >&2
    exit 1
fi

say "Building and installing devicehop to $bin"
cargo install --path "$repo" --locked

say "Installing the udev rule (needs sudo)"
sudo install -m 0644 "$repo/packaging/linux/70-devicehop.rules" /etc/udev/rules.d/70-devicehop.rules
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=hidraw

say "Your devices"
sleep 1
if ! "$bin" list; then
    echo
    echo "No devices answered. Make sure the keyboard and mouse are connected to this computer over Bluetooth," >&2
    echo "turn them off and on once (so the new udev rule applies), then run this script again." >&2
    exit 1
fi

cat <<'EOF'

Follow mode sends the remaining devices to another computer when one device
leaves with its Easy-Switch key. Enter where they should go, in the same form
as `devicehop switch`:
  - part of the other computer's name as shown above, e.g.  macbook
  - or host numbers per device, e.g.                       keys=2 master=3
    (use numbers if the name appears on more than one host of a device)
Leave empty to skip follow mode.
EOF
read -r -p "Follow target: " target

if [ -z "$target" ]; then
    echo "Skipped. You can still run: devicehop switch <target>"
    exit 0
fi

say "Installing the follow service"
mkdir -p "$unit_dir"
cat > "$unit" <<EOF
[Unit]
Description=devicehop follow mode (devices follow each other on Easy-Switch)
After=bluetooth.target

[Service]
ExecStart=$bin follow $target
Restart=on-failure
RestartSec=5
# The kernel refuses internet sockets for this service (no network, ever).
NoNewPrivileges=yes
RestrictAddressFamilies=AF_UNIX AF_NETLINK

[Install]
WantedBy=default.target
EOF
systemctl --user daemon-reload
systemctl --user enable --now devicehop-follow.service
sleep 1
systemctl --user --no-pager --lines=5 status devicehop-follow.service || true

cat <<EOF

Done. Press Easy-Switch on the keyboard: the mouse should follow.
  Logs:      journalctl --user -u devicehop-follow -f
  Change:    edit $unit, then: systemctl --user restart devicehop-follow
  Uninstall: systemctl --user disable --now devicehop-follow && rm "$unit"
             sudo rm /etc/udev/rules.d/70-devicehop.rules
EOF
