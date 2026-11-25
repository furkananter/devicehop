mod hidpp;

use std::collections::HashSet;
use std::ffi::CString;
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use hidapi::HidApi;
use hidpp::{Device, FEATURE_CHANGE_HOST, FEATURE_HOSTS_INFO, FEATURE_REPROG_CONTROLS};

const VENDOR_LOGITECH: u16 = 0x046D;
const USAGE_PAGE_HIDPP: u16 = 0xFF43;

const USAGE: &str = "\
devicehop - switch Logitech Bluetooth devices to another Easy-Switch host

USAGE:
    devicehop list [--porcelain]
    devicehop switch <N>
    devicehop switch <host-name>
    devicehop switch <name>=<N> [<name>=<N> ...]
    devicehop follow <N | host-name | name=N ...>
    devicehop --help | --version

COMMANDS:
    list     Show devices, number of hosts and the current host (1-based)
    switch   Switch every device to host N, to the host whose name (as shown by
             `list`) contains <host-name>, or per device with name=N, where
             name is a case-insensitive substring of the product name
             (e.g. `devicehop switch keys=2 master=3`)
             `list --porcelain` prints tab-separated records for scripts.
    follow   Run in the foreground; when one device is switched away with its
             Easy-Switch key, move the other devices to the same target
             (same forms as `switch`). Experimental. Ctrl-C stops it.

Diagnostics: `controls` (for developers).
";

#[derive(Debug, PartialEq, Eq)]
enum Spec {
    All(u8),
    /// Switch every device to the host whose friendly name contains this text.
    ByName(String),
    PerDevice(Vec<(String, u8)>),
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    List {
        porcelain: bool,
    },
    /// Diagnostic: dump reprogrammable controls of every device.
    Controls,
    Switch(Spec),
    Follow(Spec),
}

fn parse_host(s: &str) -> Result<u8, String> {
    match s.parse::<u8>() {
        Ok(0) => Err("host numbers start at 1".to_string()),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("`{s}` is not a valid host number")),
    }
}

/// Parses a target (`<N>`, `<host-name>` or `name=N ...`) shared by `switch` and `follow`.
fn parse_spec(rest: &[String], cmd: &str) -> Result<Spec, String> {
    if rest.is_empty() {
        return Err(format!(
            "`{cmd}` needs a host number, a host name or name=N selectors"
        ));
    }
    if !rest.iter().any(|a| a.contains('=')) {
        if rest.len() > 1 {
            return Err(format!("unexpected argument `{}`", rest[1]));
        }
        if rest[0].is_empty() {
            return Err("empty host".to_string());
        }
        if rest[0].bytes().all(|b| b.is_ascii_digit()) {
            return Ok(Spec::All(parse_host(&rest[0])?));
        }
        return Ok(Spec::ByName(rest[0].clone()));
    }
    let mut sel = Vec::new();
    for a in rest {
        let (name, n) = a
            .split_once('=')
            .ok_or_else(|| format!("`{a}` is not of the form name=N"))?;
        if name.is_empty() {
            return Err(format!("empty device name in `{a}`"));
        }
        sel.push((name.to_string(), parse_host(n)?));
    }
    Ok(Spec::PerDevice(sel))
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let first = args.first().ok_or("missing command")?;
    match first.as_str() {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "-V" | "--version" => Ok(Command::Version),
        "list" => {
            let porcelain = args.get(1).is_some_and(|a| a == "--porcelain");
            let extra = if porcelain { 2 } else { 1 };
            if let Some(x) = args.get(extra) {
                return Err(format!("unexpected argument `{x}`"));
            }
            Ok(Command::List { porcelain })
        }
        "controls" => Ok(Command::Controls),
        "switch" => Ok(Command::Switch(parse_spec(&args[1..], "switch")?)),
        "follow" => Ok(Command::Follow(parse_spec(&args[1..], "follow")?)),
        other => Err(format!("unknown argument `{other}`")),
    }
}

/// Maps each device (by name) to its target host (1-based). Devices matching
/// no selector are omitted. A selector matching nothing, or a device matched
/// by several selectors, is an error.
fn resolve_targets(names: &[String], spec: &Spec) -> Result<Vec<(usize, u8)>, String> {
    match spec {
        // Host 0 is a placeholder: ByName is resolved per device in cmd_switch.
        Spec::ByName(_) => Ok((0..names.len()).map(|i| (i, 0)).collect()),
        Spec::All(n) => Ok((0..names.len()).map(|i| (i, *n)).collect()),
        Spec::PerDevice(sel) => {
            let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
            let mut out: Vec<(usize, u8)> = Vec::new();
            for (name, n) in sel {
                let needle = name.to_lowercase();
                let mut matched = false;
                for (i, dn) in lower.iter().enumerate() {
                    if dn.contains(&needle) {
                        matched = true;
                        if out.iter().any(|(j, _)| *j == i) {
                            return Err(format!(
                                "device `{}` is matched by more than one selector",
                                names[i]
                            ));
                        }
                        out.push((i, *n));
                    }
                }
                if !matched {
                    return Err(format!("no device matches `{name}`"));
                }
            }
            out.sort();
            Ok(out)
        }
    }
}

/// Per-device target for a single device by name: `None` when a per-device
/// selector list does not mention it. `ByName` yields host 0 as a placeholder
/// (see `resolve_host`).
fn target_for_name(name: &str, spec: &Spec) -> Result<Option<u8>, String> {
    match spec {
        Spec::All(n) => Ok(Some(*n)),
        Spec::ByName(_) => Ok(Some(0)),
        Spec::PerDevice(sel) => {
            let lower = name.to_lowercase();
            let mut hit = None;
            for (needle, n) in sel {
                if lower.contains(&needle.to_lowercase()) {
                    if hit.is_some() {
                        return Err(format!(
                            "device `{name}` is matched by more than one selector"
                        ));
                    }
                    hit = Some(*n);
                }
            }
            Ok(hit)
        }
    }
}

/// Final host number for one device: looks the name up for `ByName`,
/// otherwise uses `n` as resolved by `resolve_targets` / `target_for_name`.
fn resolve_host(spec: &Spec, n: u8, dev: &Device, count: u8) -> Result<u8, String> {
    match spec {
        Spec::ByName(want) => host_by_name(dev, count, want),
        _ => Ok(n),
    }
}

struct Found {
    path: CString,
    name: String,
}

fn find_devices(api: &HidApi) -> Vec<Found> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for info in api.device_list() {
        if info.vendor_id() != VENDOR_LOGITECH || info.usage_page() != USAGE_PAGE_HIDPP {
            continue;
        }
        if !seen.insert(info.path().to_owned()) {
            continue;
        }
        out.push(Found {
            path: info.path().to_owned(),
            name: info
                .product_string()
                .unwrap_or("Unknown Logitech device")
                .to_string(),
        });
    }
    out
}

fn open_hint(msg: &str) -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    if msg.contains("not permitted") || msg.contains("0xE00002E2") {
        return Some(
            "hint: the app that launches devicehop (Terminal, or your hotkey tool) needs \
             Input Monitoring permission in System Settings -> Privacy & Security -> \
             Input Monitoring; restart that app afterwards.",
        );
    }
    #[cfg(target_os = "linux")]
    if msg.to_lowercase().contains("permission denied") {
        return Some(
            "hint: install packaging/linux/70-devicehop.rules into /etc/udev/rules.d/ \
             (see README) and reconnect the device.",
        );
    }
    let _ = msg;
    None
}

fn open_device(api: &HidApi, f: &Found) -> Result<Device, String> {
    match api.open_path(&f.path) {
        Ok(d) => Ok(Device::new(d)),
        Err(e) => {
            let msg = e.to_string();
            Err(match open_hint(&msg) {
                Some(h) => format!("cannot open: {msg}\n    {h}"),
                None => format!("cannot open: {msg}"),
            })
        }
    }
}

/// Opens a device and reads its Change Host feature index and host info.
fn probe(api: &HidApi, f: &Found) -> Result<(Device, u8, u8, u8), String> {
    let dev = open_device(api, f)?;
    let idx = dev
        .feature_index(FEATURE_CHANGE_HOST)
        .map_err(|e| e.to_string())?;
    let (count, cur) = dev.host_info(idx).map_err(|e| e.to_string())?;
    Ok((dev, idx, count, cur))
}

/// Finds the 1-based host whose friendly name contains `want` (case-insensitive).
fn host_by_name(dev: &Device, count: u8, want: &str) -> Result<u8, String> {
    let hi = dev
        .feature_index(FEATURE_HOSTS_INFO)
        .map_err(|e| format!("cannot look up hosts by name ({e}); use host numbers"))?;
    let want = want.to_lowercase();
    let mut hits = Vec::new();
    for h in 0..count {
        if let Ok((true, name)) = dev.host_detail(hi, h) {
            if name.to_lowercase().contains(&want) {
                hits.push((h + 1, name));
            }
        }
    }
    match hits.as_slice() {
        [(n, _)] => Ok(*n),
        [] => Err(format!("no paired host is named like `{want}`")),
        many => Err(format!(
            "`{want}` matches several hosts ({}); use host numbers instead",
            many.iter()
                .map(|(n, name)| format!("{n}: \"{name}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Confirms a device left after the switch command, resending if it did not.
///
/// A device drops its link the moment it accepts the command, so the write
/// itself may report an error even though the switch worked. Therefore the
/// write result is not trusted: after a short wait the device is asked for its
/// host info again. No answer means it is gone (success); an answer still
/// showing another host means the command was lost, so it is sent again.
fn confirm_switch(dev: &Device, idx: u8, host0: u8) -> bool {
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(300));
        match dev.host_info(idx) {
            Ok((_, cur)) if cur != host0 => {
                let _ = dev.set_host(idx, host0);
            }
            _ => return true,
        }
    }
    false
}

fn new_api() -> Result<HidApi, String> {
    #[allow(unused_mut)]
    let mut api = HidApi::new().map_err(|e| format!("cannot initialise HID: {e}"))?;
    #[cfg(target_os = "macos")]
    api.set_open_exclusive(false);
    Ok(api)
}

/// Makes text safe for one tab-separated field.
fn tsv(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn cmd_list(porcelain: bool) -> u8 {
    let api = match new_api() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let devices = find_devices(&api);
    if devices.is_empty() {
        eprintln!("No Logitech Bluetooth HID++ devices found.");
        return 1;
    }
    for f in &devices {
        match probe(&api, f) {
            Ok((dev, _, count, cur)) if porcelain => {
                println!(
                    "device\t{}\t{}\t{}",
                    tsv(&f.name),
                    count,
                    u16::from(cur) + 1
                );
                if let Ok(hi) = dev.feature_index(FEATURE_HOSTS_INFO) {
                    for h in 0..count {
                        if let Ok((paired, name)) = dev.host_detail(hi, h) {
                            println!(
                                "host\t{}\t{}\t{}",
                                u16::from(h) + 1,
                                u8::from(paired),
                                tsv(&name)
                            );
                        }
                    }
                }
            }
            Err(e) if porcelain => println!("error\t{}\t{}", tsv(&f.name), tsv(&e)),
            Ok((dev, _, count, cur)) => {
                println!(
                    "{}: {} hosts, current host {}",
                    f.name,
                    count,
                    u16::from(cur) + 1
                );
                // Best effort: per-host pairing state and name, if the device offers it.
                if let Ok(hi) = dev.feature_index(FEATURE_HOSTS_INFO) {
                    for h in 0..count {
                        if let Ok((paired, name)) = dev.host_detail(hi, h) {
                            let state = if paired { "paired" } else { "empty" };
                            let mark = if h == cur { " (current)" } else { "" };
                            let name = if name.is_empty() {
                                String::new()
                            } else {
                                format!(" \"{name}\"")
                            };
                            println!("    host {}: {state}{name}{mark}", u16::from(h) + 1);
                        }
                    }
                }
            }
            Err(e) => println!("{}: {}", f.name, e),
        }
    }
    0
}

fn cmd_controls() -> u8 {
    let api = match new_api() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    for f in &find_devices(&api) {
        println!("{}:", f.name);
        let res = open_device(&api, f).and_then(|dev| {
            let idx = dev
                .feature_index(FEATURE_REPROG_CONTROLS)
                .map_err(|e| e.to_string())?;
            dev.controls(idx).map_err(|e| e.to_string())
        });
        match res {
            Ok(list) => {
                for (cid, tid, flags) in list {
                    let d = if flags & 0x20 != 0 { " divertable" } else { "" };
                    println!("    cid 0x{cid:04X} task 0x{tid:04X} flags 0x{flags:02X}{d}");
                }
            }
            Err(e) => println!("    {e}"),
        }
    }
    0
}

fn cmd_switch(spec: &Spec) -> u8 {
    let api = match new_api() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let devices = find_devices(&api);
    if devices.is_empty() {
        eprintln!("No Logitech Bluetooth HID++ devices found.");
        return 1;
    }
    let names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();
    let targets = match resolve_targets(&names, spec) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    // Phase 1: probe and validate everything before sending anything.
    let mut ready = Vec::new();
    let mut failed = false;
    for (i, n) in targets {
        let f = &devices[i];
        match probe(&api, f) {
            Ok((dev, idx, count, cur)) => {
                let n = match resolve_host(spec, n, &dev, count) {
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("{}: {}", f.name, e);
                        failed = true;
                        continue;
                    }
                };
                if n > count {
                    eprintln!(
                        "{}: host {} is out of range (device has {} hosts)",
                        f.name, n, count
                    );
                    failed = true;
                } else {
                    ready.push((f, dev, idx, n, cur));
                }
            }
            Err(e) => {
                eprintln!("{}: {}", f.name, e);
                failed = true;
            }
        }
    }
    if failed {
        eprintln!("Aborting: nothing was sent.");
        return 1;
    }

    // Phase 2: send.
    let mut code = 0;
    // Send to every device first so they leave together, then confirm each.
    let mut sent = Vec::new();
    for (f, dev, idx, n, cur) in &ready {
        if *cur == n - 1 {
            println!("{}: already on host {}, skipping", f.name, n);
            continue;
        }
        let _ = dev.set_host(*idx, n - 1);
        sent.push((f, dev, *idx, *n));
    }
    for (f, dev, idx, n) in sent {
        if confirm_switch(dev, idx, n - 1) {
            println!("{}: switched to host {}", f.name, n);
        } else {
            eprintln!("{}: failed to switch to host {}", f.name, n);
            code = 1;
        }
    }
    code
}

fn say(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

/// Lists the devices that are present right now, keyed by path, WITHOUT opening them.
fn present_devices(api: &mut HidApi) -> Vec<Found> {
    let _ = api.refresh_devices();
    find_devices(api)
}

/// Called after a device left: sends the devices that are still here to the target.
///
/// Devices are opened only here, for the moment it takes to send the command.
/// Follow mode holds no open device handle while it waits, so the operating
/// system delivers none of the devices' input to this process during normal use.
fn follow_departure(api: &mut HidApi, spec: &Spec) -> Vec<CString> {
    std::thread::sleep(Duration::from_millis(700));
    let here = present_devices(api);
    let mut ready = Vec::new();
    for f in &here {
        // A device that does not answer is on its way out too; leave it alone.
        let Ok((dev, idx, count, cur)) = probe(api, f) else {
            continue;
        };
        ready.push((f, dev, idx, count, cur));
    }
    // Nothing answers: Bluetooth is off or the system is going to sleep.
    let mut sent = Vec::new();
    for (f, dev, idx, count, cur) in &ready {
        let n = match target_for_name(&f.name, spec)
            .and_then(|t| t.map(|n| resolve_host(spec, n, dev, *count)).transpose())
        {
            Ok(Some(n)) => n,
            Ok(None) => continue,
            Err(e) => {
                say(&format!("{}: {e}", f.name));
                continue;
            }
        };
        if *cur == n.wrapping_sub(1) {
            continue;
        }
        if n == 0 || n > *count {
            say(&format!(
                "{}: host {n} is out of range (device has {count} hosts)",
                f.name
            ));
            continue;
        }
        let _ = dev.set_host(*idx, n - 1);
        sent.push((*f, dev, *idx, n));
    }
    let mut followed = Vec::new();
    for (f, dev, idx, n) in sent {
        if confirm_switch(dev, idx, n - 1) {
            say(&format!("{}: followed to host {n}", f.name));
            followed.push(f.path.clone());
        } else {
            say(&format!("{}: failed to follow to host {n}", f.name));
        }
    }
    followed
}

fn cmd_follow(spec: &Spec) -> u8 {
    let mut api = match new_api() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    say(
        "follow: when one device is switched away with its Easy-Switch key, \
         the other devices follow to the target. Press Ctrl-C to stop.",
    );
    // Paths and names of the devices seen on the previous poll. Presence is
    // read from the system's device list; no device is opened while waiting.
    let mut known: Vec<(CString, String)> = Vec::new();
    loop {
        let now = present_devices(&mut api);
        let mut left = false;
        for (path, name) in &known {
            if !now.iter().any(|f| &f.path == path) {
                say(&format!("{name}: left"));
                left = true;
            }
        }
        for f in &now {
            if !known.iter().any(|(p, _)| p == &f.path) {
                say(&format!("{}: present", f.name));
            }
        }
        known = now.into_iter().map(|f| (f.path, f.name)).collect();
        if left && !known.is_empty() {
            let followed = follow_departure(&mut api, spec);
            // Followed devices are gone by our own doing; do not report them as "left".
            known.retain(|(p, _)| !followed.contains(p));
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Command::Help) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("devicehop {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::List { porcelain }) => ExitCode::from(cmd_list(porcelain)),
        Ok(Command::Controls) => ExitCode::from(cmd_controls()),
        Ok(Command::Switch(spec)) => ExitCode::from(cmd_switch(&spec)),
        Ok(Command::Follow(spec)) => ExitCode::from(cmd_follow(&spec)),
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_basic() {
        assert_eq!(
            parse_args(&a(&["list"])),
            Ok(Command::List { porcelain: false })
        );
        assert_eq!(
            parse_args(&a(&["list", "--porcelain"])),
            Ok(Command::List { porcelain: true })
        );
        assert!(parse_args(&a(&["list", "--porcelain", "x"])).is_err());
        assert_eq!(tsv("a\tb\nc"), "a b c");
        assert_eq!(parse_args(&a(&["-h"])), Ok(Command::Help));
        assert_eq!(parse_args(&a(&["--version"])), Ok(Command::Version));
        assert_eq!(
            parse_args(&a(&["switch", "2"])),
            Ok(Command::Switch(Spec::All(2)))
        );
    }

    #[test]
    fn parses_selectors() {
        assert_eq!(
            parse_args(&a(&["switch", "keys=2", "master=3"])),
            Ok(Command::Switch(Spec::PerDevice(vec![
                ("keys".into(), 2),
                ("master".into(), 3)
            ])))
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_args(&a(&[])).is_err());
        assert!(parse_args(&a(&["switch"])).is_err());
        assert!(parse_args(&a(&["switch", "0"])).is_err());
        assert_eq!(
            parse_args(&a(&["switch", "abc"])),
            Ok(Command::Switch(Spec::ByName("abc".into())))
        );
        assert!(parse_args(&a(&["switch", ""])).is_err());
        assert!(parse_args(&a(&["switch", "keys=0"])).is_err());
        assert!(parse_args(&a(&["switch", "keys=x"])).is_err());
        assert!(parse_args(&a(&["switch", "=2"])).is_err());
        assert!(parse_args(&a(&["switch", "keys=2", "3"])).is_err());
        assert!(parse_args(&a(&["switch", "1", "2"])).is_err());
        assert!(parse_args(&a(&["list", "x"])).is_err());
        assert!(parse_args(&a(&["bogus"])).is_err());
    }

    #[test]
    fn parses_follow() {
        assert_eq!(
            parse_args(&a(&["follow", "2"])),
            Ok(Command::Follow(Spec::All(2)))
        );
        assert_eq!(
            parse_args(&a(&["follow", "office-pc"])),
            Ok(Command::Follow(Spec::ByName("office-pc".into())))
        );
        assert_eq!(
            parse_args(&a(&["follow", "keys=2"])),
            Ok(Command::Follow(Spec::PerDevice(vec![("keys".into(), 2)])))
        );
        assert!(parse_args(&a(&["follow"])).is_err());
        assert!(parse_args(&a(&["follow", "0"])).is_err());
        assert!(parse_args(&a(&["follow", "1", "2"])).is_err());
    }

    #[test]
    fn target_for_single_device() {
        let spec = Spec::PerDevice(vec![("keys".into(), 2)]);
        assert_eq!(target_for_name("MX Keys Mini", &spec), Ok(Some(2)));
        assert_eq!(target_for_name("MX Master 4", &spec), Ok(None));
        assert_eq!(target_for_name("x", &Spec::All(3)), Ok(Some(3)));
        let spec = Spec::PerDevice(vec![("mx".into(), 2), ("keys".into(), 3)]);
        assert!(target_for_name("MX Keys Mini", &spec).is_err());
    }

    #[test]
    fn resolves_selectors() {
        let names = a(&["MX Keys Mini", "MX Master 4"]);
        let spec = Spec::PerDevice(vec![("KEYS".into(), 2)]);
        assert_eq!(resolve_targets(&names, &spec), Ok(vec![(0, 2)]));
        assert_eq!(
            resolve_targets(&names, &Spec::All(1)),
            Ok(vec![(0, 1), (1, 1)])
        );
        let spec = Spec::PerDevice(vec![("keys".into(), 2), ("master".into(), 3)]);
        assert_eq!(resolve_targets(&names, &spec), Ok(vec![(0, 2), (1, 3)]));
        let spec = Spec::PerDevice(vec![("trackball".into(), 2)]);
        assert!(resolve_targets(&names, &spec).is_err());
        let spec = Spec::PerDevice(vec![("mx".into(), 2), ("keys".into(), 3)]);
        assert!(resolve_targets(&names, &spec).is_err());
    }
}
