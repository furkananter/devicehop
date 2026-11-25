#!/usr/bin/env bash
# DeviceHop installer for Linux (Debian/Ubuntu and other systemd distros).
#
# Builds and installs, for the current user only:
#   - devicehop and devicehop-tray in ~/.local/bin
#   - the follow service (systemd user unit), which starts at login
#   - the tray's autostart entry, launcher entry and icon
# and, with sudo, the udev rule that lets your user talk to Logitech Bluetooth
# devices. It is safe to run again. Run it as your normal user, never as root.
#
#   install.sh                       interactive
#   install.sh --yes [--target SPEC] no questions; SPEC is e.g. "Office PC" or "keys=2 master=3"
#   install.sh --uninstall [--yes]   remove everything this script installed
#
# STATUS: not yet verified on real Linux hardware with Bluetooth devices.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
pkg_dir="$repo/packaging/linux"

bin_dir="$HOME/.local/bin"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
unit_file="$config_home/systemd/user/devicehop-follow.service"
autostart_file="$config_home/autostart/devicehop-tray.desktop"
launcher_file="$data_home/applications/devicehop-tray.desktop"
icon_file="$data_home/icons/hicolor/scalable/apps/devicehop.svg"
target_file="$config_home/devicehop/follow-target"
udev_rule="/etc/udev/rules.d/70-devicehop.rules"

assume_yes=0
uninstall=0
target=""
have_target=0

say() { printf '\n==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: install.sh [--yes|-y] [--target "<spec>"] [--uninstall] [--help]

  --yes, -y          do not ask questions (keeps an existing follow target
                     unless --target is given)
  --target "<spec>"  follow target: a host name ("Office PC") or per-device
                     host numbers ("keys=2 master=3")
  --uninstall        remove what this script installed (asks before removing
                     the udev rule, unless --yes)
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --yes | -y) assume_yes=1 ;;
        --uninstall) uninstall=1 ;;
        --target)
            [ $# -ge 2 ] || die "--target needs a value"
            target="$2"
            have_target=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            die "unknown argument: $1"
            ;;
    esac
    shift
done

[ "$(id -u)" -ne 0 ] || die "run this as your normal user, not as root (it uses sudo where needed)."

# Normalises whitespace; the config file holds exactly one line.
squeeze() { printf '%s' "$*" | tr -s '[:space:]' ' ' | sed 's/^ //; s/ $//'; }

# ask "question": true if the user answers yes (always true with --yes).
ask() {
    [ "$assume_yes" -eq 1 ] && return 0
    [ -t 0 ] || return 1
    local reply
    read -r -p "$1 [y/N] " reply
    [ "$reply" = y ] || [ "$reply" = Y ]
}

# Is there a systemd user session we can talk to?
user_systemd() { systemctl --user show-environment >/dev/null 2>&1; }

# install_file MODE SRC DEST: replaces DEST atomically, so a running binary can be updated.
install_file() {
    mkdir -p "$(dirname "$3")"
    install -m "$1" "$2" "$3.new"
    mv -f "$3.new" "$3"
}

# ---------------------------------------------------------------- uninstall --

do_uninstall() {
    say "Stopping DeviceHop"
    if user_systemd; then
        systemctl --user disable --now devicehop-follow.service 2>/dev/null || true
    fi
    pkill -x devicehop-tray 2>/dev/null || true

    say "Removing installed files"
    rm -f "$unit_file" "$autostart_file" "$launcher_file" "$icon_file" \
        "$bin_dir/devicehop" "$bin_dir/devicehop-tray" "$target_file"
    rmdir "$(dirname "$target_file")" 2>/dev/null || true
    if user_systemd; then
        systemctl --user daemon-reload || true
    fi

    if [ -e "$udev_rule" ]; then
        if ask "Remove the udev rule $udev_rule (needs sudo)? Other programs may rely on it."; then
            sudo rm -f "$udev_rule"
            sudo udevadm control --reload || true
        else
            echo "Kept $udev_rule. Remove it later with: sudo rm $udev_rule"
        fi
    fi

    cat <<EOF

DeviceHop is uninstalled. Left in place: build tools installed with apt, and the
build directories inside $repo (target/ and linux/devicehop-tray/target/).
EOF
}

if [ "$uninstall" -eq 1 ]; then
    do_uninstall
    exit 0
fi

# ------------------------------------------------------------------ install --

say "Checking build requirements"
if command -v apt-get >/dev/null 2>&1; then
    missing=()
    for pkg in build-essential pkg-config libudev-dev; do
        dpkg -s "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
    done
    if [ "${#missing[@]}" -gt 0 ]; then
        echo "Installing with apt: ${missing[*]}"
        sudo apt-get install -y "${missing[@]}"
    fi
else
    echo "apt-get not found: make sure a C compiler, pkg-config and the libudev development files are installed."
fi
command -v cargo >/dev/null 2>&1 || die "Rust is not installed. Install it from https://rustup.rs and run this again."
rust_minor="$(rustc --version | sed -n 's/^rustc 1\.\([0-9]*\)\..*/\1/p')"
if [ -n "$rust_minor" ] && [ "$rust_minor" -lt 88 ]; then
    die "Rust 1.88 or newer is needed (found $(rustc --version)). Install a current toolchain from https://rustup.rs (the one from apt is too old)."
fi

say "Building devicehop and devicehop-tray"
cargo build --release --locked --manifest-path "$repo/Cargo.toml"
cargo build --release --locked --manifest-path "$repo/linux/devicehop-tray/Cargo.toml"

say "Installing to $bin_dir"
install_file 0755 "$repo/target/release/devicehop" "$bin_dir/devicehop"
install_file 0755 "$repo/linux/devicehop-tray/target/release/devicehop-tray" "$bin_dir/devicehop-tray"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) warn "$bin_dir is not in your PATH; add it to run devicehop from a terminal (Ubuntu adds it at next login if the directory exists)." ;;
esac

say "Installing the udev rule"
if cmp -s "$pkg_dir/70-devicehop.rules" "$udev_rule" 2>/dev/null; then
    echo "Already up to date: $udev_rule"
else
    sudo install -m 0644 "$pkg_dir/70-devicehop.rules" "$udev_rule"
    if ! { sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=hidraw; }; then
        warn "could not reload udev; reconnect the devices or reboot for the rule to apply."
    fi
fi

say "Installing the follow service, tray entries and icon"
install_file 0644 "$pkg_dir/devicehop-follow.service" "$unit_file"
mkdir -p "$(dirname "$autostart_file")" "$(dirname "$launcher_file")"
for desktop in "$autostart_file" "$launcher_file"; do
    sed "s|@BINDIR@|$bin_dir|" "$pkg_dir/devicehop-tray.desktop" >"$desktop.new"
    mv -f "$desktop.new" "$desktop"
done
install_file 0644 "$repo/linux/devicehop-tray/icon/devicehop.svg" "$icon_file"

say "Your devices"
sleep 1
if ! "$bin_dir/devicehop" list; then
    warn "no device answered. Make sure the keyboard and mouse are connected to this computer over Bluetooth,"
    warn "then turn them off and on once (so the new udev rule applies) and run: devicehop list"
fi

# ------------------------------------------------------------- follow target --

# Distinct paired host names of the working devices, one per line.
host_names() {
    "$bin_dir/devicehop" list --porcelain 2>/dev/null |
        awk -F'\t' '$1 == "device" { ok = 1 } $1 == "error" { ok = 0 }
                    $1 == "host" && ok && $3 == "1" && $4 != "" && !seen[$4]++ { print $4 }' || true
}

choose_target() {
    local hosts=() choice i
    mapfile -t hosts < <(host_names)
    cat <<'EOF'

Follow mode: when one device is switched away with its Easy-Switch key, the
others follow to a target computer. Choose where they should go:
EOF
    for i in "${!hosts[@]}"; do
        printf '  %d) %s\n' "$((i + 1))" "${hosts[$i]}"
    done
    echo "  t) type a target, e.g. a host name or per-device numbers like: keys=2 master=3"
    echo "  s) skip (leave follow mode as it is)"
    read -r -p "Choice: " choice
    case "$choice" in
        '' | s | S) ;;
        t | T) read -r -p "Target: " target && have_target=1 ;;
        *[!0-9]*) warn "not a valid choice; skipping" ;;
        *)
            if [ "$choice" -ge 1 ] && [ "$choice" -le "${#hosts[@]}" ]; then
                target="${hosts[$((choice - 1))]}"
                have_target=1
            else
                warn "not a valid choice; skipping"
            fi
            ;;
    esac
}

if [ "$have_target" -eq 0 ] && [ "$assume_yes" -eq 0 ] && [ -t 0 ]; then
    choose_target
fi

if [ "$have_target" -eq 1 ]; then
    target="$(squeeze "$target")"
    if [ -n "$target" ]; then
        mkdir -p "$(dirname "$target_file")"
        printf '%s\n' "$target" >"$target_file"
    else
        warn "empty follow target; keeping the current setting"
    fi
fi

follow_target=""
if [ -s "$target_file" ]; then
    follow_target="$(head -n 1 "$target_file")"
fi

say "Starting the follow service"
if user_systemd; then
    systemctl --user daemon-reload
    # Enabled even without a target: the unit is skipped while the config file is empty,
    # and starts at login as soon as the tray (or you) saves a target.
    systemctl --user enable devicehop-follow.service
    if [ -n "$follow_target" ]; then
        systemctl --user restart devicehop-follow.service
        sleep 1
        systemctl --user --no-pager --lines=5 status devicehop-follow.service || true
    else
        echo "No follow target set; the service stays idle. Choose one from the tray menu."
    fi
else
    warn "no systemd user session found; the follow service was installed but not started."
fi

say "Starting the tray"
tray_started=0
if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    pkill -x devicehop-tray 2>/dev/null || true
    nohup "$bin_dir/devicehop-tray" >/dev/null 2>&1 &
    disown
    tray_started=1
    echo "Started. If no icon appears, run $bin_dir/devicehop-tray in a terminal to see why."
else
    echo "No graphical session detected; the tray starts at your next login."
fi

# ------------------------------------------------------------------- summary --

cat <<EOF

DeviceHop is installed.

  Programs:   $bin_dir/devicehop, $bin_dir/devicehop-tray
  Tray:       starts at login ($autostart_file)$([ "$tray_started" -eq 1 ] && echo ", running now")
  Follow:     ${follow_target:+target "$follow_target", }service devicehop-follow, config $target_file
  Access:     $udev_rule

Useful commands:
  devicehop list                          show devices and their hosts
  journalctl --user -u devicehop-follow   follow service log

Uninstall:  $pkg_dir/install.sh --uninstall
EOF
