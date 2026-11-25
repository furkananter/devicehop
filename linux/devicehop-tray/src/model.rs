//! Pure logic of the tray: parsing `devicehop list --porcelain`, deriving the
//! host names offered in the menu, and the follow-target config file.
//! Nothing here touches D-Bus, processes or devices.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub number: u32,
    pub paired: bool,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceInfo {
    pub name: String,
    pub current: u32,
    pub hosts: Vec<HostInfo>,
    pub error: Option<String>,
}

impl DeviceInfo {
    /// Name of the host this device is on now, or "host N" if it has no name.
    pub fn current_name(&self) -> String {
        match self.hosts.iter().find(|h| h.number == self.current) {
            Some(h) if !h.name.is_empty() => h.name.clone(),
            _ => format!("host {}", self.current),
        }
    }

    fn current_host_name(&self) -> Option<&str> {
        self.hosts
            .iter()
            .find(|h| h.number == self.current)
            .map(|h| h.name.as_str())
    }
}

/// What the menu knows about the devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListState {
    Loading,
    Failed(String),
    Devices(Vec<DeviceInfo>),
}

/// Parses the output of `devicehop list --porcelain`:
/// `device\t<name>\t<count>\t<current>`, `host\t<n>\t<paired 1|0>\t<name>`,
/// `error\t<name>\t<message>`. Unknown or malformed lines are ignored.
pub fn parse_porcelain(text: &str) -> Vec<DeviceInfo> {
    let mut devices: Vec<DeviceInfo> = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        match f[0] {
            "device" if f.len() >= 4 => devices.push(DeviceInfo {
                name: f[1].to_string(),
                current: f[3].parse().unwrap_or(0),
                ..DeviceInfo::default()
            }),
            "host" if f.len() >= 3 => {
                if let Some(d) = devices.last_mut() {
                    d.hosts.push(HostInfo {
                        number: f[1].parse().unwrap_or(0),
                        paired: f[2] == "1",
                        name: f.get(3).unwrap_or(&"").to_string(),
                    });
                }
            }
            "error" if f.len() >= 3 => devices.push(DeviceInfo {
                name: f[1].to_string(),
                error: Some(f[2].to_string()),
                ..DeviceInfo::default()
            }),
            _ => {}
        }
    }
    devices
}

/// Distinct non-empty paired host names across all working devices, in first-seen order.
pub fn host_names(devices: &[DeviceInfo]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in devices.iter().filter(|d| d.error.is_none()) {
        for h in d.hosts.iter().filter(|h| h.paired && !h.name.is_empty()) {
            if !out.contains(&h.name) {
                out.push(h.name.clone());
            }
        }
    }
    out
}

/// The host names worth offering as "Switch all devices to": every name except
/// one that all working devices are already on.
pub fn switch_candidates(devices: &[DeviceInfo]) -> Vec<String> {
    let working: Vec<&DeviceInfo> = devices.iter().filter(|d| d.error.is_none()).collect();
    host_names(devices)
        .into_iter()
        .filter(|n| !working.iter().all(|d| d.current_host_name() == Some(n)))
        .collect()
}

/// The entries of the follow-mode submenu: Off (`None`), each host name, and the
/// saved target even when no device currently lists it.
pub fn follow_choices(names: &[String], saved: Option<&str>) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = vec![None];
    out.extend(names.iter().cloned().map(Some));
    if let Some(t) = saved
        && !names.iter().any(|n| n == t)
    {
        out.push(Some(t.to_string()));
    }
    out
}

/// True if a device (or the whole listing) failed because it may not be opened.
pub fn access_denied(state: &ListState) -> bool {
    let denied = |s: &str| s.to_lowercase().contains("permission denied");
    match state {
        ListState::Loading => false,
        ListState::Failed(m) => denied(m),
        ListState::Devices(ds) => ds.iter().any(|d| d.error.as_deref().is_some_and(denied)),
    }
}

/// First line of `text`, shortened to at most `max` characters.
pub fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() > max {
        let cut: String = line.chars().take(max).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

// --- follow target config file ---------------------------------------------

/// `$XDG_CONFIG_HOME/devicehop/follow-target` (default `~/.config/...`).
/// A relative `XDG_CONFIG_HOME` is ignored, as the XDG specification says.
pub fn config_path(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let base = match xdg_config_home.map(Path::new) {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        _ => Path::new(home?).join(".config"),
    };
    Some(base.join("devicehop").join("follow-target"))
}

/// The saved follow target: the first non-blank line, trimmed. Missing,
/// unreadable or empty means follow mode is off.
pub fn read_target(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Saves the follow target as a single line; `None` leaves the file empty.
pub fn write_target(path: &Path, target: Option<&str>) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let line = target.map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "));
    let content = match line {
        Some(l) if !l.is_empty() => format!("{l}\n"),
        _ => String::new(),
    };
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
device\tMX Keys Mini\t3\t2
host\t1\t1\tOffice PC
host\t2\t1\tFurkan's MacBook
host\t3\t0\t
device\tMX Master 4\t3\t2
host\t1\t1\tOffice PC
host\t2\t1\tFurkan's MacBook
host\t3\t1\tTablet
error\tMX Anywhere\tcannot open: Permission denied
";

    #[test]
    fn parses_devices_hosts_and_errors() {
        let d = parse_porcelain(SAMPLE);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].name, "MX Keys Mini");
        assert_eq!(d[0].current, 2);
        assert_eq!(d[0].hosts.len(), 3);
        assert!(!d[0].hosts[2].paired);
        assert_eq!(d[0].hosts[2].name, "");
        assert_eq!(d[0].current_name(), "Furkan's MacBook");
        assert_eq!(
            d[2].error.as_deref(),
            Some("cannot open: Permission denied")
        );
    }

    #[test]
    fn ignores_garbage() {
        assert!(parse_porcelain("").is_empty());
        assert!(parse_porcelain("hello\nhost\t1\t1\tx\ndevice\tonly-two\n").is_empty());
        // A host line without a name field still parses.
        let d = parse_porcelain("device\tK\t2\t1\nhost\t1\t0\n");
        assert_eq!(d[0].hosts.len(), 1);
        assert_eq!(d[0].hosts[0].name, "");
    }

    #[test]
    fn current_name_falls_back_to_number() {
        let d = parse_porcelain("device\tK\t3\t3\nhost\t3\t0\t\n");
        assert_eq!(d[0].current_name(), "host 3");
        let d = parse_porcelain("device\tK\t3\t2\n");
        assert_eq!(d[0].current_name(), "host 2");
    }

    #[test]
    fn host_names_are_distinct_paired_and_ordered() {
        let d = parse_porcelain(SAMPLE);
        assert_eq!(host_names(&d), ["Office PC", "Furkan's MacBook", "Tablet"]);
    }

    #[test]
    fn candidates_skip_the_host_everyone_is_on() {
        let d = parse_porcelain(SAMPLE);
        // Both working devices are on "Furkan's MacBook".
        assert_eq!(switch_candidates(&d), ["Office PC", "Tablet"]);
        let one = parse_porcelain("device\tK\t2\t1\nhost\t1\t1\tA\nhost\t2\t1\tB\n");
        assert_eq!(switch_candidates(&one), ["B"]);
        assert!(switch_candidates(&[]).is_empty());
    }

    #[test]
    fn follow_choices_include_off_and_saved_target() {
        let names = vec!["A".to_string(), "B".to_string()];
        assert_eq!(
            follow_choices(&names, Some("B")),
            [None, Some("A".into()), Some("B".into())]
        );
        assert_eq!(
            follow_choices(&names, Some("keys=2 master=3")),
            [
                None,
                Some("A".into()),
                Some("B".into()),
                Some("keys=2 master=3".into())
            ]
        );
        assert_eq!(follow_choices(&[], None), [None]);
    }

    #[test]
    fn detects_access_denied() {
        let d = parse_porcelain(SAMPLE);
        assert!(access_denied(&ListState::Devices(d)));
        assert!(access_denied(&ListState::Failed(
            "Permission denied".into()
        )));
        assert!(!access_denied(&ListState::Failed("No devices".into())));
        assert!(!access_denied(&ListState::Loading));
        assert!(!access_denied(&ListState::Devices(parse_porcelain(
            "device\tK\t2\t1\n"
        ))));
    }

    #[test]
    fn first_line_truncates() {
        assert_eq!(first_line("a\nb", 10), "a");
        assert_eq!(first_line("abcdef", 3), "abc…");
        assert_eq!(first_line("", 3), "");
    }

    #[test]
    fn config_path_follows_xdg() {
        let p =
            |x: Option<&str>, h: Option<&str>| config_path(x.map(OsStr::new), h.map(OsStr::new));
        let tail = "devicehop/follow-target";
        assert_eq!(
            p(Some("/x/cfg"), Some("/h")),
            Some(Path::new("/x/cfg").join(tail))
        );
        assert_eq!(
            p(None, Some("/h")),
            Some(Path::new("/h/.config").join(tail))
        );
        assert_eq!(
            p(Some(""), Some("/h")),
            Some(Path::new("/h/.config").join(tail))
        );
        assert_eq!(
            p(Some("rel"), Some("/h")),
            Some(Path::new("/h/.config").join(tail))
        );
        assert_eq!(p(None, None), None);
    }

    #[test]
    fn config_round_trip() {
        let dir = std::env::temp_dir().join(format!("devicehop-tray-test-{}", std::process::id()));
        let path = dir.join("devicehop").join("follow-target");
        assert_eq!(read_target(&path), None);
        write_target(&path, Some("  keys=2   master=3 ")).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "keys=2 master=3\n");
        assert_eq!(read_target(&path).as_deref(), Some("keys=2 master=3"));
        write_target(&path, Some("Furkan's MacBook")).unwrap();
        assert_eq!(read_target(&path).as_deref(), Some("Furkan's MacBook"));
        write_target(&path, None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
        assert_eq!(read_target(&path), None);
        write_target(&path, Some("   ")).unwrap();
        assert_eq!(read_target(&path), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
