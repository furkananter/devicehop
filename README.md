# devicehop

Switch Logitech multi-host keyboards and mice to another Easy-Switch host
channel from the command line, without any vendor software. It sends the
HID++ 2.0 "Change Host" command (feature 0x1814) to devices connected directly
over Bluetooth LE. Works on macOS and Linux.

devicehop is not affiliated with, endorsed by, or sponsored by Logitech.
Logitech, MX Keys and MX Master are trademarks of their respective owners.

## Status

Early. Verified on macOS with an MX Keys Mini and an MX Master 4 (listing hosts,
switching both to another host, and follow mode). **Linux support has not yet been run on real
hardware**: the build, the tests, the installer and the tray's menu were checked on Ubuntu 24.04 in
a container, but not with real Bluetooth devices, udev or a desktop session. Expect rough edges
and please report what you find.

## Supported setup

- Devices connected **directly over Bluetooth** (e.g. MX Keys Mini, MX Master 4).
- Bolt and Unifying receivers are **not supported yet**.

## Build

```sh
cargo build --release
```

The binary is `target/release/devicehop`.

- macOS: no extra requirements.
- Linux: the `linux-static-hidraw` feature compiles the bundled hidapi hidraw
  backend with a C compiler and needs the **libudev development files** found
  via pkg-config (Debian/Ubuntu: `sudo apt install build-essential pkg-config libudev-dev`;
  Fedora: `systemd-devel`). It does not need libusb.

## Usage

```sh
devicehop list                    # devices, their hosts (paired state, name), current host
devicehop switch 2                # switch every found device to host 2
devicehop switch office-pc        # switch every device to the host named like "office-pc"
devicehop switch keys=2 master=3  # per device; name is a case-insensitive substring
devicehop --help
```

Host numbers are 1-based, as printed on the Easy-Switch keys. `switch` is
two-phase: every targeted device is opened and checked first, and nothing is
sent if any of them fails, so a keyboard and mouse are never split across hosts.
Devices already on the requested host are skipped.

A keyboard and a mouse often have the same computer on different host numbers.
Switching by host name handles that: each device looks up which of its hosts
carries that name (as shown by `list`; case-insensitive substring). If the name
matches more than one host on a device, devicehop refuses and you use numbers.

After sending, devicehop confirms that each device really left and resends the
command if it is still there. Exit codes: 0 success,
1 runtime failure, 2 usage error.

### macOS: Input Monitoring

macOS requires Input Monitoring permission to open HID devices. Grant it to the
app that launches devicehop (Terminal, or your hotkey tool) in
System Settings -> Privacy & Security -> Input Monitoring, then restart that app.
If you see "not permitted" or `0xE00002E2`, this is the cause.

### Linux: one-step install

```sh
packaging/linux/install.sh
```

Run it as your normal user (it uses `sudo` only for apt and the udev rule). It
installs the build requirements (`build-essential pkg-config libudev-dev`),
needs Rust 1.88 or newer (use [rustup](https://rustup.rs); Ubuntu's apt `cargo`
is too old), builds and installs `devicehop` and `devicehop-tray` to
`~/.local/bin`, installs the udev rule, the follow service (systemd user unit),
the tray's autostart entry and icon, shows your devices, asks where the devices
should follow to, and starts the tray. It is safe to run again.

Options: `--yes` / `-y` asks no questions (an existing follow target is kept),
`--target "<spec>"` sets the follow target (`"Office PC"` or `"keys=2 master=3"`),
`--uninstall` removes everything the installer put in place (it asks before
removing the udev rule; `--yes` removes it without asking).

**Not yet verified on real hardware.** The manual steps follow.

### Linux: tray icon

`devicehop-tray` puts a DeviceHop icon in the top bar with the same menu as the
macOS app: each device and the host it is on, "Switch all devices to" a host,
and a "Follow mode" submenu. It uses the StatusNotifierItem protocol, so it
works on KDE, XFCE and others, and on GNOME through the **"AppIndicator and
KStatusNotifierItem Support"** extension (Ubuntu ships it enabled; on other
GNOME systems install and enable it, otherwise the tray prints an error and
exits).

The tray never talks to a device itself: it runs `devicehop list --porcelain`
and `devicehop switch <host>` and shows the output. The menu shows the last
known state at once and refreshes in the background when it is opened; there
are no timers. If a device cannot be opened (permission denied) the menu says
so; install the udev rule below.

### Linux: tray and follow service

Follow mode runs as the systemd user service `devicehop-follow`, not inside the
tray, so it keeps working when the tray is closed and starts at login by
itself. The target lives in `~/.config/devicehop/follow-target` (or under
`$XDG_CONFIG_HOME`), one line: a host name (may contain spaces) or `name=N`
selectors separated by spaces (`keys=2 master=3`). An empty or missing file
means follow mode is off and the service does not start. The tray's "Follow
mode" submenu writes that file and restarts (or stops) the service; you can
also edit the file and run `systemctl --user restart devicehop-follow`. Log:
`journalctl --user -u devicehop-follow`.

### Linux: udev rule

```sh
sudo cp packaging/linux/70-devicehop.rules /etc/udev/rules.d/
sudo udevadm control --reload
```

Then disconnect and reconnect the device (or run `sudo udevadm trigger`). The
rule grants the logged-in seat user access to Logitech **Bluetooth** hidraw
nodes only.

## Follow mode

**Experimental.** Verified on macOS (a mouse followed a keyboard that left with its Easy-Switch key); not yet run on real Linux hardware.

```sh
devicehop follow office-pc        # same target forms as `switch`
```

Press the Easy-Switch key on one device and the others follow. Run it on each
computer with the *other* computer as target. It is a foreground process that
keeps the devices open and polls them. The Easy-Switch keys send no
notification before the device leaves, so the only signal is that a device
disappears while the others stay connected. Those remaining devices are then
checked after a short delay and, if they still answer, switched to the target
like `switch` would. If no device answers any more (Bluetooth off, system
sleep) nothing is done. Devices that come back are picked up again
automatically.

Known limitations:

- It cannot tell a host switch from that one device being powered off, going out
  of range or (if the device drops its link when idle) sleeping, so the other
  devices may be sent away in those cases.
- With more than two computers it always sends to the single configured target.

To start it automatically: on Linux the installer sets up the service (see
above); on macOS see the template (untested)
`packaging/macos/dev.devicehop.follow.plist` (launchd LaunchAgent), which needs
the target filled in; under launchd the devicehop binary itself needs Input
Monitoring permission.

## macOS menu bar app

**New and not yet verified on hardware.** A small menu bar app (`DeviceHop`)
so you never have to type commands. It is a UI shell around the bundled
`devicehop-cli` binary (this CLI).

- Lists your devices and the host each is on.
- "Switch all devices to" a host, by name.
- Follow mode: pick a host (or Off); press Easy-Switch on one device and the
  others follow. Kept running while the app runs and restarted if it exits.
- Global hotkey **Control-Option-Command-H** switches to the follow target.
- "Launch at login" and a shortcut to grant Input Monitoring.

Build and install into `~/Applications`:

```sh
macos/build-app.sh --install
```

(`macos/build-app.sh` without `--install` only builds `build/DeviceHop.app`.)

The app itself needs **Input Monitoring** (System Settings -> Privacy &
Security), granted once to DeviceHop; use the menu item "Grant Input
Monitoring…" and restart the app. The build is ad-hoc signed, so macOS asks
again after every rebuild; signing with a Developer ID certificate removes that.

## Binding a hotkey

- macOS: create a Shortcut in the Shortcuts app with a "Run Shell Script" action
  (`/path/to/devicehop switch 2`) and assign a keyboard shortcut in the
  shortcut's settings. Grant Input Monitoring to Shortcuts.
- GNOME: Settings -> Keyboard -> Custom Shortcuts, add a shortcut whose command is
  `/path/to/devicehop switch 2` (e.g. `~/.local/bin/devicehop`).

## Security and privacy

- No network code, no telemetry, no account, no data files.
- No root. Linux uses a udev rule instead; it lets programs running as your
  user open these devices, a trade-off explained in `SECURITY.md`.
- Devices are opened only for the moment it takes to query or switch them.
  Follow mode keeps no device open while it waits.
- While a device is open the OS delivers all of its input reports to the
  program. DeviceHop discards every report that is not a HID++ configuration
  report, unread; it never inspects or records key presses or pointer motion.
- `devicehop` has one library dependency: `hidapi`. The Linux tray is a
  separate program with its own dependencies (a D-Bus library); it only runs
  the `devicehop` command line.

`SECURITY.md` explains each point and where to verify it in the source.

## Licence

Proprietary, all rights reserved. See `LICENSE`. Anyone given access to the
source may read it to review its behaviour; it may not be copied, modified,
built, used or redistributed without written permission from the copyright
holder. The build and install instructions in this file are for the copyright
holder and licensed users. Third-party components are listed in
`THIRD-PARTY-NOTICES.md`. Written from the protocol description, not derived
from Solaar or other GPL code.
