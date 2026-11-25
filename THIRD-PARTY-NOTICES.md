# Third-party notices

DeviceHop itself is proprietary (see `LICENSE`). It is built with the
components below, which remain under their own licences.

| Component | Used for | Licence |
|---|---|---|
| [hidapi (Rust crate)](https://crates.io/crates/hidapi) | talking to HID devices | MIT |
| [HIDAPI (C library, bundled by the crate)](https://github.com/libusb/hidapi) | same | used under its BSD-style licence option |
| [libc](https://crates.io/crates/libc), [cfg-if](https://crates.io/crates/cfg-if) | linked by hidapi | MIT OR Apache-2.0 |
| cc, pkg-config, shlex, find-msvc-tools | build tools only, not part of the binary | MIT OR Apache-2.0 |

The full licence texts ship with each component's source. Binary distributions
of DeviceHop must include this file and the licence texts of hidapi and HIDAPI.
