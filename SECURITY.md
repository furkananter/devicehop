# Security and privacy

DeviceHop sends one kind of command to your keyboard and mouse: "switch to
host N". This page states exactly what it can see, what it does with it, and
how you can check each claim in the source.

## What DeviceHop does not do

- **No network.** There is no networking code in DeviceHop. It never connects
  to anything, has no telemetry, no update check and no account.
  *Check:* `grep -rn "std::net\|socket\|http\|URLSession" src/ macos/DeviceHop/` finds nothing, and
  the only library dependency of `devicehop` is `hidapi` (`cargo tree`).
- **No data files.** The only things it writes are: in the macOS app, the one
  preference "which computer to follow to"; on Linux, the file
  `~/.config/devicehop/follow-target` with that same one line; and, if you run
  follow mode as a background service, its status lines ("<device>: left", "followed to host N")
  in the service log. Neither contains input from your devices.
- **No root.** It runs as your user.
- **No keystroke or pointer logging.** See the next section for the precise
  mechanics.

## What the operating system hands to DeviceHop, and what happens to it

To send a command, DeviceHop must open the device. macOS and Linux do not let a
program open "only the configuration channel" of a Bluetooth keyboard or mouse:
while the device is open, the system delivers **all** of that device's input
reports to the program, including key presses and pointer motion. This is why
macOS asks for Input Monitoring permission. DeviceHop limits this in two ways:

1. **Devices are open only for moments.** A device is opened to list hosts or
   to send a switch command, typically for well under a second (at most a few
   seconds if the device does not answer), then closed. **Follow mode keeps no
   device open while it waits**: it only watches the system's list of connected
   devices and opens the remaining devices at the moment another one leaves.
   *Check:* `cmd_follow` and `present_devices` in `src/main.rs`.
2. **Anything that is not a HID++ configuration report is discarded unread.**
   While a device is open, every incoming report whose first byte is not `0x11`
   is dropped without being parsed, and DeviceHop's own copy is overwritten
   with zeros; it is never stored, printed or passed on. *Check:*
   `Device::request` in `src/hidpp.rs` is the only place that reads from a
   device.

So: during the short moments a device is open, key presses made in that moment
do reach the DeviceHop process. They are never inspected or recorded. To be
exact about memory: the HID library DeviceHop uses (hidapi) keeps its own short
queue of incoming reports (at most 30) while a device is open and frees it when
the device is closed; DeviceHop zeroes its own buffer but cannot zero that
library queue.

## The macOS app

The menu bar app is a thin shell: it runs the bundled `devicehop-cli` to do all
device work and shows the result. It needs Input Monitoring for the reason
above and no other permission. The global hotkey uses the system hotkey
service and does not observe other key presses.

## The Linux udev rule: a trade-off you should know about

On Linux, DeviceHop needs your user to be able to open the devices' `hidraw`
nodes. The udev rule grants that to the user logged in at the seat, for
Logitech Bluetooth devices only. The consequence: **any program running as
your user** (not only DeviceHop) can then open those nodes and read the
devices' raw reports, including key presses. On X11 this adds little, because
any program can already observe the keyboard; on Wayland it weakens the
isolation Wayland otherwise provides. The alternatives (running a root helper)
have their own risks. If this trade-off is not acceptable on a machine, do not
install the rule there.

## The Linux tray

`devicehop-tray` is a separate program (in `linux/devicehop-tray/`). It never
opens a device: all device work is done by running the `devicehop` command line
as a subprocess. It talks only to the D-Bus session bus (to show its icon and
menu), opens no network connection, and needs no root. Its follow-mode choice
is written to `~/.config/devicehop/follow-target` and applied with
`systemctl --user`. Unlike `devicehop`, it has library dependencies (a D-Bus
implementation; see `linux/devicehop-tray/Cargo.lock`).

## The Linux service

The systemd user unit runs follow mode with `NoNewPrivileges=yes` and
`RestrictAddressFamilies=AF_UNIX AF_NETLINK`, so the kernel itself refuses any
internet socket, whatever the program tries to do. It reads its target from
`~/.config/devicehop/follow-target`.

## Limits

- This has not been independently audited.
- You are trusting the build you run. Build from source if you want to be sure
  the binary matches the code you read.

## Reporting a problem

Please report security issues privately to the copyright holder rather than in
a public issue.
