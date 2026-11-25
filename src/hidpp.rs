//! Minimal HID++ 2.0 support for direct Bluetooth LE devices.
//!
//! The packet functions are pure and testable without hardware. `Device`
//! wraps an opened hidapi handle.

use std::fmt;
use std::time::{Duration, Instant};

use hidapi::HidDevice;

pub const REPORT_LONG: u8 = 0x11;
pub const DEVICE_INDEX: u8 = 0xFF;
pub const SW_ID: u8 = 0x0B;
pub const REQUEST_LEN: usize = 20;
pub const MAX_PARAMS: usize = REQUEST_LEN - 4;

pub const ROOT_INDEX: u8 = 0x00;
pub const FEATURE_CHANGE_HOST: u16 = 0x1814;
pub const FEATURE_HOSTS_INFO: u16 = 0x1815;
pub const FEATURE_REPROG_CONTROLS: u16 = 0x1B04;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const READ_SLICE_MS: i32 = 250;

#[derive(Debug)]
pub enum Error {
    Hid(String),
    Timeout,
    /// The device answered with a HID++ error code.
    Device(u8),
    Unsupported(u16),
    Malformed(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Hid(m) => write!(f, "{m}"),
            Error::Timeout => write!(f, "timed out waiting for the device to answer"),
            Error::Device(c) => write!(f, "device returned HID++ error 0x{c:02X}"),
            Error::Unsupported(id) => {
                write!(f, "feature 0x{id:04X} is not supported by this device")
            }
            Error::Malformed(m) => write!(f, "malformed response: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Error::Hid(e.to_string())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Response {
    Match(Vec<u8>),
    Error(u8),
    Unrelated,
}

/// True for HID++ long reports. Every other report (mouse movement, keys, ...)
/// is input data that devicehop must never look at or keep.
pub fn is_hidpp(buf: &[u8]) -> bool {
    buf.first() == Some(&REPORT_LONG)
}

/// Builds a 20-byte long-report request. Parameters beyond 16 bytes are ignored.
pub fn build_request(feature_index: u8, function: u8, params: &[u8]) -> [u8; REQUEST_LEN] {
    let mut buf = [0u8; REQUEST_LEN];
    buf[0] = REPORT_LONG;
    buf[1] = DEVICE_INDEX;
    buf[2] = feature_index;
    buf[3] = ((function & 0x0F) << 4) | SW_ID;
    let n = params.len().min(MAX_PARAMS);
    buf[4..4 + n].copy_from_slice(&params[..n]);
    buf
}

/// Decides whether `buf` answers the request (feature_index, function).
pub fn classify_response(buf: &[u8], feature_index: u8, function: u8) -> Response {
    let func_sw = ((function & 0x0F) << 4) | SW_ID;
    if buf.len() < 5 || buf[0] != REPORT_LONG || buf[1] != DEVICE_INDEX {
        return Response::Unrelated;
    }
    if buf[2] == 0xFF {
        if buf.len() >= 6 && buf[3] == feature_index && buf[4] == func_sw {
            return Response::Error(buf[5]);
        }
        return Response::Unrelated;
    }
    if buf[2] == feature_index && buf[3] == func_sw {
        return Response::Match(buf[4..].to_vec());
    }
    Response::Unrelated
}

pub struct Device {
    dev: HidDevice,
}

impl Device {
    pub fn new(dev: HidDevice) -> Self {
        Device { dev }
    }

    fn write(&self, feature_index: u8, function: u8, params: &[u8]) -> Result<(), Error> {
        let req = build_request(feature_index, function, params);
        self.dev.write(&req)?;
        Ok(())
    }

    fn request(&self, feature_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>, Error> {
        self.write(feature_index, function, params)?;
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        let mut buf = [0u8; 64];
        while Instant::now() < deadline {
            let n = self.dev.read_timeout(&mut buf, READ_SLICE_MS)?;
            if n == 0 {
                continue;
            }
            // While a handle is open the OS also delivers the device's ordinary
            // input reports (pointer motion, key presses). Those are wiped here,
            // unread: nothing but a HID++ report is ever inspected or kept.
            if !is_hidpp(&buf[..n]) {
                buf.fill(0);
                continue;
            }
            let res = classify_response(&buf[..n], feature_index, function);
            buf.fill(0);
            match res {
                Response::Match(p) => return Ok(p),
                Response::Error(c) => return Err(Error::Device(c)),
                Response::Unrelated => {}
            }
        }
        Err(Error::Timeout)
    }

    /// Looks up the feature index for `feature_id` via the root feature.
    pub fn feature_index(&self, feature_id: u16) -> Result<u8, Error> {
        let p = self.request(ROOT_INDEX, 0, &feature_id.to_be_bytes())?;
        match p.first() {
            Some(0) => Err(Error::Unsupported(feature_id)),
            Some(&i) => Ok(i),
            None => Err(Error::Malformed("empty getFeature payload")),
        }
    }

    /// Returns (number of hosts, current host 0-based).
    pub fn host_info(&self, change_host_index: u8) -> Result<(u8, u8), Error> {
        let p = self.request(change_host_index, 0, &[])?;
        if p.len() < 2 {
            return Err(Error::Malformed("short getHostInfo payload"));
        }
        Ok((p[0], p[1]))
    }

    /// Returns (paired, friendly name) for `host0` via the HOSTS_INFO feature.
    /// The name is empty when the host never registered one.
    pub fn host_detail(&self, hosts_info_index: u8, host0: u8) -> Result<(bool, String), Error> {
        let p = self.request(hosts_info_index, 1, &[host0])?;
        if p.len() < 5 {
            return Err(Error::Malformed("short HOSTS_INFO payload"));
        }
        let paired = p[1] != 0;
        let len = usize::from(p[4]);
        let mut name = Vec::new();
        while name.len() < len {
            let c = self.request(hosts_info_index, 3, &[host0, name.len() as u8])?;
            let chunk = c.get(2..).unwrap_or(&[]);
            let take = chunk.len().min(len - name.len());
            if take == 0 {
                break;
            }
            name.extend_from_slice(&chunk[..take]);
        }
        Ok((
            paired,
            String::from_utf8_lossy(&name)
                .trim_end_matches('\0')
                .to_string(),
        ))
    }

    /// Lists the device's reprogrammable controls as (control id, task id, flags).
    pub fn controls(&self, reprog_index: u8) -> Result<Vec<(u16, u16, u8)>, Error> {
        let count = *self
            .request(reprog_index, 0, &[])?
            .first()
            .ok_or(Error::Malformed("empty getCount payload"))?;
        let mut out = Vec::new();
        for i in 0..count {
            let p = self.request(reprog_index, 1, &[i])?;
            if p.len() < 5 {
                return Err(Error::Malformed("short getCidInfo payload"));
            }
            out.push((
                u16::from_be_bytes([p[0], p[1]]),
                u16::from_be_bytes([p[2], p[3]]),
                p[4],
            ));
        }
        Ok(out)
    }

    /// Asks the device to switch to `host0` (0-based). The device disconnects
    /// immediately, so no reply is awaited.
    pub fn set_host(&self, change_host_index: u8, host0: u8) -> Result<(), Error> {
        self.write(change_host_index, 1, &[host0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_non_hidpp() {
        assert!(is_hidpp(&[0x11, 0xFF, 0x00]));
        assert!(!is_hidpp(&[0x02, 0x01, 0x00]));
        assert!(!is_hidpp(&[0x10, 0xFF]));
        assert!(!is_hidpp(&[]));
    }

    #[test]
    fn request_layout() {
        let r = build_request(0x06, 1, &[0x02]);
        assert_eq!(r.len(), 20);
        assert_eq!(&r[..5], &[0x11, 0xFF, 0x06, 0x1B, 0x02]);
        assert!(r[5..].iter().all(|&b| b == 0));
    }

    #[test]
    fn request_root_get_feature() {
        let r = build_request(0, 0, &0x1814u16.to_be_bytes());
        assert_eq!(&r[..6], &[0x11, 0xFF, 0x00, 0x0B, 0x18, 0x14]);
    }

    #[test]
    fn request_truncates_long_params() {
        let r = build_request(1, 0, &[0xAA; 40]);
        assert_eq!(r.len(), 20);
        assert_eq!(r[19], 0xAA);
    }

    fn frame(head: &[u8]) -> Vec<u8> {
        let mut v = head.to_vec();
        v.resize(20, 0);
        v
    }

    #[test]
    fn classify_match() {
        let f = frame(&[0x11, 0xFF, 0x06, 0x0B, 0x03, 0x01]);
        match classify_response(&f, 0x06, 0) {
            Response::Match(p) => assert_eq!(&p[..2], &[3, 1]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn classify_error() {
        let f = frame(&[0x11, 0xFF, 0xFF, 0x06, 0x1B, 0x05]);
        assert_eq!(classify_response(&f, 0x06, 1), Response::Error(5));
    }

    #[test]
    fn classify_unrelated() {
        // other feature index
        let f = frame(&[0x11, 0xFF, 0x07, 0x0B]);
        assert_eq!(classify_response(&f, 0x06, 0), Response::Unrelated);
        // other sw id (notification)
        let f = frame(&[0x11, 0xFF, 0x06, 0x00]);
        assert_eq!(classify_response(&f, 0x06, 0), Response::Unrelated);
        // other function
        let f = frame(&[0x11, 0xFF, 0x06, 0x1B]);
        assert_eq!(classify_response(&f, 0x06, 0), Response::Unrelated);
        // error for a different request
        let f = frame(&[0x11, 0xFF, 0xFF, 0x09, 0x0B, 0x01]);
        assert_eq!(classify_response(&f, 0x06, 0), Response::Unrelated);
        // wrong report id, too short
        assert_eq!(
            classify_response(&[0x10, 0xFF, 0x06, 0x0B, 0], 0x06, 0),
            Response::Unrelated
        );
        assert_eq!(
            classify_response(&[0x11, 0xFF], 0x06, 0),
            Response::Unrelated
        );
    }
}
