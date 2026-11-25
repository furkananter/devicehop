//! Running external programs (`devicehop`, `systemctl`, `notify-send`) as
//! subprocesses. This is the only way the tray reaches a device or the system:
//! it never opens a device itself.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const LIST_TIMEOUT: Duration = Duration::from_secs(20);
pub const SWITCH_TIMEOUT: Duration = Duration::from_secs(30);
const SERVICE_TIMEOUT: Duration = Duration::from_secs(20);
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Output {
    /// `None` if the program could not be started or was killed (timeout).
    pub status: Option<i32>,
    pub out: String,
    pub err: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    /// stderr and stdout together, for error messages.
    pub fn message(&self) -> String {
        let text = [self.err.trim(), self.out.trim()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            format!("exited with status {}", self.status.unwrap_or(-1))
        } else {
            text
        }
    }

    fn failed(msg: String) -> Self {
        Output {
            status: None,
            out: String::new(),
            err: msg,
        }
    }
}

/// Runs `program args...` and waits at most `timeout`. Both pipes are drained
/// on their own threads, so a chatty child cannot fill a pipe and stall.
/// Blocking: call only from a worker thread.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Output {
    let spawned = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return Output::failed(format!("cannot start {program}: {e}")),
    };
    fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<String> {
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s.code()),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "{program} did not finish within {} s",
                    timeout.as_secs()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => break Err(format!("cannot wait for {program}: {e}")),
        }
    };
    let (out, mut err) = (
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    );
    match status {
        Ok(code) => Output {
            status: code,
            out,
            err,
        },
        Err(msg) => {
            err = format!("{msg}\n{err}");
            Output {
                status: None,
                out,
                err,
            }
        }
    }
}

/// The `devicehop` binary: next to this executable if present, else from PATH.
pub fn devicehop_path() -> String {
    let beside: Option<PathBuf> = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("devicehop")))
        .filter(|p| p.is_file());
    match beside {
        Some(p) => p.to_string_lossy().into_owned(),
        None => "devicehop".to_string(),
    }
}

pub fn list(devicehop: &str) -> Output {
    run(devicehop, &["list", "--porcelain"], LIST_TIMEOUT)
}

pub fn switch(devicehop: &str, target: &str) -> Output {
    run(devicehop, &["switch", target], SWITCH_TIMEOUT)
}

/// Restarts the follow service (it reads its target from the config file) or
/// stops it when follow mode is off.
pub fn follow_service(on: bool) -> Output {
    let action = if on { "restart" } else { "stop" };
    run(
        "systemctl",
        &["--user", action, "devicehop-follow.service"],
        SERVICE_TIMEOUT,
    )
}

/// Shows a desktop notification via `notify-send`, or prints to stderr if that
/// is not installed.
pub fn notify(title: &str, body: &str) {
    let shown = run(
        "notify-send",
        &["--app-name=DeviceHop", "--", title, body],
        NOTIFY_TIMEOUT,
    );
    if !shown.success() {
        eprintln!("devicehop-tray: {title}: {body}");
    }
}
