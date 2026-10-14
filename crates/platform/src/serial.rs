//! Serial port contract (design S2). Implemented by `platform-linux`; [`crate::fake`]
//! has an in-memory port for tests.
//!
//! Errors are typed through [`rxt_core::Error`]: `PermissionDenied` (message names the
//! owning group), `Busy`, `NotFound`, `Gone` (device removed) and `Unsupported`.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rxt_core::LineConfig;
pub use rxt_core::PortInfo;

use crate::{Error, Result};

/// Serial port enumeration and opening.
pub trait SerialPorts: Send + Sync {
    fn list(&self) -> Result<Vec<PortInfo>>;
    /// Open `path` for exclusive use with `cfg`; a second open reports `Error::Busy`.
    fn open(&self, path: &str, cfg: &LineConfig) -> Result<Box<dyn SerialStream>>;
}

/// Blocking serial stream with a ~100 ms read timeout.
pub trait SerialStream: Send {
    /// `Ok(0)` on timeout; `Err(Error::Gone)` when the device disappeared.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    fn write_all(&mut self, bytes: &[u8]) -> Result<()>;
    fn set_lines(&mut self, dtr: Option<bool>, rts: Option<bool>) -> Result<()>;
    fn send_break(&mut self, dur: Duration) -> Result<()>;
    fn reconfigure(&mut self, cfg: &LineConfig) -> Result<()>;
    fn try_clone_writer(&self) -> Result<Box<dyn SerialStream>>;
}

/// State of one [`FakeSerial`] device, as seen by tests.
#[derive(Debug, Clone, Default)]
pub struct FakeDevice {
    pub info: PortInfo,
    /// Line configuration of the last open/reconfigure.
    pub cfg: Option<LineConfig>,
    /// Open handles (stream plus cloned writers); non-zero makes a new open `Busy`.
    pub handles: usize,
    /// Unplugged: every handle fails with `Error::Gone`.
    pub gone: bool,
    /// Bytes the device will send to the host.
    pub to_host: std::collections::VecDeque<u8>,
    /// Bytes the host wrote.
    pub written: Vec<u8>,
    pub breaks: Vec<Duration>,
    pub dtr: Option<bool>,
    pub rts: Option<bool>,
    /// When set, opening fails with `PermissionDenied` naming this group.
    pub denied_group: Option<String>,
}

type Dev = Arc<Mutex<FakeDevice>>;

/// In-memory serial ports for tests: plug/unplug devices, feed bytes, inspect writes.
/// Clones share state.
#[derive(Clone, Default)]
pub struct FakeSerial {
    devices: Arc<Mutex<Vec<Dev>>>,
}

fn poisoned() -> Error {
    Error::Io("fake serial lock poisoned".into())
}

fn lock(d: &Dev) -> Result<MutexGuard<'_, FakeDevice>> {
    d.lock().map_err(|_| poisoned())
}

impl FakeSerial {
    /// Plug a device (a fresh one, even if the path was used before).
    pub fn plug(&self, info: PortInfo) {
        if let Ok(mut ds) = self.devices.lock() {
            ds.push(Arc::new(Mutex::new(FakeDevice { info, ..FakeDevice::default() })));
        }
    }

    /// Unplug the device at `path`: it leaves the list and open handles fail with `Gone`.
    pub fn unplug(&self, path: &str) {
        if let Ok(mut ds) = self.devices.lock() {
            ds.retain(|d| {
                let Ok(mut d) = d.lock() else { return false };
                let hit = d.info.path == path;
                d.gone |= hit;
                !hit
            });
        }
    }

    /// Queue bytes the device at `path` sends.
    pub fn send(&self, path: &str, bytes: &[u8]) {
        self.edit(path, |d| d.to_host.extend(bytes));
    }

    /// Make opening `path` fail with `PermissionDenied` naming `group`.
    pub fn deny(&self, path: &str, group: &str) {
        self.edit(path, |d| d.denied_group = Some(group.into()));
    }

    /// Snapshot of the plugged device at `path`.
    pub fn device(&self, path: &str) -> Option<FakeDevice> {
        self.find(path).and_then(|d| d.lock().ok().map(|d| d.clone()))
    }

    fn edit(&self, path: &str, f: impl FnOnce(&mut FakeDevice)) {
        if let Some(d) = self.find(path)
            && let Ok(mut d) = d.lock()
        {
            f(&mut d);
        }
    }

    fn find(&self, path: &str) -> Option<Dev> {
        let ds = self.devices.lock().ok()?;
        ds.iter()
            .find(|d| d.lock().is_ok_and(|d| d.info.path == path || d.info.by_id.as_deref() == Some(path)))
            .cloned()
    }
}

impl SerialPorts for FakeSerial {
    fn list(&self) -> Result<Vec<PortInfo>> {
        let ds = self.devices.lock().map_err(|_| poisoned())?;
        ds.iter().map(|d| lock(d).map(|d| d.info.clone())).collect()
    }

    fn open(&self, path: &str, cfg: &LineConfig) -> Result<Box<dyn SerialStream>> {
        let dev = self.find(path).ok_or_else(|| Error::NotFound(format!("serial port {path}")))?;
        {
            let mut d = lock(&dev)?;
            if let Some(g) = &d.denied_group {
                return Err(Error::PermissionDenied(format!("{path} (group `{g}`)")));
            }
            if d.handles > 0 {
                return Err(Error::Busy(path.into()));
            }
            d.handles = 1;
            d.cfg = Some(*cfg);
        }
        Ok(Box::new(FakeStream { dev, path: path.into() }))
    }
}

struct FakeStream {
    dev: Dev,
    path: String,
}

impl FakeStream {
    fn with<T>(&self, f: impl FnOnce(&mut FakeDevice) -> T) -> Result<T> {
        let mut d = lock(&self.dev)?;
        if d.gone {
            return Err(Error::Gone(self.path.clone()));
        }
        Ok(f(&mut d))
    }
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        if let Ok(mut d) = self.dev.lock() {
            d.handles = d.handles.saturating_sub(1);
        }
    }
}

impl SerialStream for FakeStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = self.with(|d| {
            let n = buf.len().min(d.to_host.len());
            buf.iter_mut().zip(d.to_host.drain(..n)).for_each(|(b, v)| *b = v);
            n
        })?;
        if n == 0 {
            // Stand-in for the real 100 ms read timeout, short to keep tests fast.
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(n)
    }
    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.with(|d| d.written.extend_from_slice(bytes))
    }
    fn set_lines(&mut self, dtr: Option<bool>, rts: Option<bool>) -> Result<()> {
        self.with(|d| {
            d.dtr = dtr.or(d.dtr);
            d.rts = rts.or(d.rts);
        })
    }
    fn send_break(&mut self, dur: Duration) -> Result<()> {
        self.with(|d| d.breaks.push(dur))
    }
    fn reconfigure(&mut self, cfg: &LineConfig) -> Result<()> {
        self.with(|d| d.cfg = Some(*cfg))
    }
    fn try_clone_writer(&self) -> Result<Box<dyn SerialStream>> {
        self.with(|d| d.handles += 1)?;
        Ok(Box::new(FakeStream { dev: self.dev.clone(), path: self.path.clone() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(path: &str) -> PortInfo {
        PortInfo { path: path.into(), by_id: Some("/dev/serial/by-id/usb-X_1".into()), ..PortInfo::default() }
    }

    #[test]
    fn fake_contract_busy_gone_and_release() {
        let f = FakeSerial::default();
        f.plug(usb("/dev/ttyUSB0"));
        let cfg = LineConfig::default();
        assert!(matches!(f.open("/dev/ttyUSB1", &cfg), Err(Error::NotFound(_))));
        let mut s = f.open("/dev/serial/by-id/usb-X_1", &cfg).unwrap();
        let w = s.try_clone_writer().unwrap();
        assert!(matches!(f.open("/dev/ttyUSB0", &cfg), Err(Error::Busy(_))));
        f.send("/dev/ttyUSB0", b"hi");
        let mut buf = [0u8; 8];
        assert_eq!(s.read(&mut buf).unwrap(), 2);
        assert_eq!(s.read(&mut buf).unwrap(), 0);
        s.write_all(b"x").unwrap();
        s.set_lines(Some(true), None).unwrap();
        assert_eq!(f.device("/dev/ttyUSB0").unwrap().written, b"x");
        assert_eq!(f.device("/dev/ttyUSB0").unwrap().dtr, Some(true));
        drop((s, w));
        f.deny("/dev/ttyUSB0", "dialout");
        let e = f.open("/dev/ttyUSB0", &cfg).err().unwrap();
        assert!(e.to_string().contains("dialout"), "{e}");
        f.unplug("/dev/ttyUSB0");
        assert!(f.list().unwrap().is_empty());
    }

    #[test]
    fn unplug_fails_open_handles_with_gone() {
        let f = FakeSerial::default();
        f.plug(usb("/dev/ttyUSB0"));
        let mut s = f.open("/dev/ttyUSB0", &LineConfig::default()).unwrap();
        f.unplug("/dev/ttyUSB0");
        assert!(matches!(s.read(&mut [0u8; 4]), Err(Error::Gone(_))));
        assert!(matches!(s.write_all(b"x"), Err(Error::Gone(_))));
    }
}
