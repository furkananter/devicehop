//! DeviceHop tray: a StatusNotifierItem menu around the `devicehop` CLI.
//!
//! The tray never opens a device. It runs `devicehop list|switch` as a
//! subprocess and shows the result. Follow mode is the systemd user service
//! `devicehop-follow`; the tray only writes its target to the config file and
//! restarts or stops the service.

mod cmd;
mod model;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Category, Icon, MenuItem, Tray};

use model::{ListState, follow_choices, host_names, switch_candidates};

const ICON_22: &[u8] = include_bytes!("../icon/glyph-22.argb");
const ICON_44: &[u8] = include_bytes!("../icon/glyph-44.argb");
const _: () = assert!(ICON_22.len() == 22 * 22 * 4 && ICON_44.len() == 44 * 44 * 4);

/// Devices that left take a moment to disappear from the system.
const REFRESH_AFTER_SWITCH: Duration = Duration::from_millis(1500);

enum Msg {
    Refresh,
    Switch(String),
    SetFollow(Option<String>),
    Quit,
}

struct DeviceHopTray {
    /// Last known device state: shown at once when the menu opens, then refreshed.
    list: ListState,
    switching: bool,
    follow: Option<String>,
    tx: Sender<Msg>,
}

impl DeviceHopTray {
    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    fn start_switch(&mut self, name: String) {
        if !self.switching {
            self.switching = true;
            self.send(Msg::Switch(name));
        }
    }

    fn set_follow(&mut self, target: Option<String>) {
        self.follow = target.clone();
        self.send(Msg::SetFollow(target));
    }

    fn item(label: impl Into<String>, enabled: bool) -> StandardItem<Self> {
        StandardItem {
            label: label.into(),
            enabled,
            ..Default::default()
        }
    }

    fn header(label: impl Into<String>) -> MenuItem<Self> {
        Self::item(label, false).into()
    }
}

impl Tray for DeviceHopTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "devicehop-tray".into()
    }

    fn title(&self) -> String {
        "DeviceHop".into()
    }

    fn category(&self) -> Category {
        Category::Hardware
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        [(22, ICON_22), (44, ICON_44)]
            .into_iter()
            .map(|(size, data)| Icon {
                width: size,
                height: size,
                data: data.to_vec(),
            })
            .collect()
    }

    fn menu_about_to_show(&mut self) {
        self.send(Msg::Refresh);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut menu: Vec<MenuItem<Self>> = Vec::new();

        if model::access_denied(&self.list) {
            menu.push(Self::header(
                "Devices not accessible — see README (udev rule)",
            ));
            menu.push(MenuItem::Separator);
        }

        let mut names = Vec::new();
        match &self.list {
            ListState::Loading => menu.push(Self::header("Loading…")),
            ListState::Failed(msg) => menu.push(Self::header(if msg.trim().is_empty() {
                "No devices found".to_string()
            } else {
                model::first_line(msg.trim(), 80)
            })),
            ListState::Devices(devices) => {
                names = host_names(devices);
                for d in devices {
                    menu.push(Self::header(match &d.error {
                        Some(e) => format!("{} — {}", d.name, model::first_line(e, 80)),
                        None => format!("{} — {}", d.name, d.current_name()),
                    }));
                }
                let candidates = switch_candidates(devices);
                if !candidates.is_empty() {
                    menu.push(MenuItem::Separator);
                    menu.push(Self::header(if self.switching {
                        "Switching…"
                    } else {
                        "Switch all devices to"
                    }));
                    for name in candidates {
                        let target = name.clone();
                        menu.push(
                            StandardItem {
                                activate: Box::new(move |t: &mut Self| {
                                    t.start_switch(target.clone());
                                }),
                                ..Self::item(name, !self.switching)
                            }
                            .into(),
                        );
                    }
                }
            }
        }

        menu.push(MenuItem::Separator);
        let choices = follow_choices(&names, self.follow.as_deref());
        let selected = choices.iter().position(|c| *c == self.follow).unwrap_or(0);
        let options = choices
            .iter()
            .map(|c| RadioItem {
                label: c.clone().unwrap_or_else(|| "Off".into()),
                ..Default::default()
            })
            .collect();
        menu.push(
            SubMenu {
                label: format!("Follow mode: {}", self.follow.as_deref().unwrap_or("Off")),
                submenu: vec![
                    RadioGroup {
                        selected,
                        select: Box::new(move |t: &mut Self, i| t.set_follow(choices[i].clone())),
                        options,
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
        );
        menu.push(Self::header(
            "Press Easy-Switch on one device; the others follow.",
        ));

        menu.push(MenuItem::Separator);
        menu.push(
            StandardItem {
                activate: Box::new(|t: &mut Self| t.send(Msg::Quit)),
                ..Self::item("Quit DeviceHop Tray", true)
            }
            .into(),
        );
        menu
    }
}

/// Does all the slow work (subprocesses, file writes) so that the D-Bus thread
/// never blocks. Runs on the main thread after the tray has been spawned.
fn worker(rx: mpsc::Receiver<Msg>, handle: Handle<DeviceHopTray>, config: Option<PathBuf>) {
    let devicehop = cmd::devicehop_path();
    let mut pending: VecDeque<Msg> = VecDeque::new();
    loop {
        let msg = match pending.pop_front() {
            Some(m) => m,
            None => match rx.recv() {
                Ok(m) => m,
                Err(_) => return,
            },
        };
        match msg {
            Msg::Refresh => {
                // Menu opens that arrived while we were busy need only one refresh.
                pending.extend(rx.try_iter().filter(|m| !matches!(m, Msg::Refresh)));
                let r = cmd::list(&devicehop);
                let devices = model::parse_porcelain(&r.out);
                let state = if devices.is_empty() {
                    ListState::Failed(r.message())
                } else {
                    ListState::Devices(devices)
                };
                handle.update(|t| {
                    if t.list != state {
                        t.list = state;
                    }
                });
            }
            Msg::Switch(name) => {
                let r = cmd::switch(&devicehop, &name);
                if !r.success() {
                    cmd::notify(&format!("Could not switch to “{name}”"), &r.message());
                }
                handle.update(|t| t.switching = false);
                thread::sleep(REFRESH_AFTER_SWITCH);
                pending.push_back(Msg::Refresh);
            }
            Msg::SetFollow(target) => {
                let saved = match &config {
                    Some(path) => model::write_target(path, target.as_deref())
                        .map_err(|e| format!("cannot write {}: {e}", path.display())),
                    None => Err("cannot find the config directory (HOME is not set)".into()),
                };
                if let Err(e) = saved {
                    cmd::notify("Could not save the follow target", &e);
                    continue;
                }
                let r = cmd::follow_service(target.is_some());
                if !r.success() {
                    cmd::notify("Could not change follow mode", &r.message());
                }
            }
            // Returning ends the process; the bus drops the tray item with our connection.
            Msg::Quit => return,
        }
    }
}

fn main() -> ExitCode {
    let config = model::config_path(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    );
    let (tx, rx) = mpsc::channel();
    let tray = DeviceHopTray {
        list: ListState::Loading,
        switching: false,
        follow: config.as_deref().and_then(model::read_target),
        tx: tx.clone(),
    };
    let handle = match tray.spawn() {
        Ok(h) => h,
        Err(e @ (ksni::Error::WontShow | ksni::Error::Watcher(_))) => {
            eprintln!(
                "devicehop-tray: no StatusNotifier host on the desktop ({e}); GNOME needs the \
                 \"AppIndicator and KStatusNotifierItem Support\" extension (Ubuntu enables it by default)"
            );
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("devicehop-tray: {e}");
            return ExitCode::FAILURE;
        }
    };
    let _ = tx.send(Msg::Refresh);
    worker(rx, handle, config);
    ExitCode::SUCCESS
}
