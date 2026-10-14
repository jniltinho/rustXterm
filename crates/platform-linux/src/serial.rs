//! Serial ports on Linux (design S1, S5): sysfs enumeration with `std::fs`, I/O through
//! `serialport` (no libudev). Ports open with exclusive access (`TIOCEXCL` + `flock`).

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rxt_core::{DataBits, FlowControl, LineConfig, Parity, StopBits};
use rxt_platform::{Error, PortInfo, Result, SerialPorts, SerialStream};
use serialport::{ErrorKind, SerialPort, TTYPort};

/// Read timeout of the blocking reader (design S2).
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// A write blocked by flow control longer than this fails instead of hanging the tab.
const WRITE_DEADLINE: Duration = Duration::from_secs(10);
/// Accepted difference between the requested and the applied baud rate (driver rounding).
const BAUD_TOLERANCE_PCT: u64 = 3;

/// Linux serial ports. Roots are configurable for fixture tests.
pub struct LinuxSerial {
    sys: PathBuf,
    dev: PathBuf,
    group_file: PathBuf,
}

impl Default for LinuxSerial {
    fn default() -> Self {
        Self { sys: "/sys".into(), dev: "/dev".into(), group_file: "/etc/group".into() }
    }
}

impl LinuxSerial {
    /// Adapter reading `sys`, `dev` and the group file from other roots (tests).
    pub fn with_roots(sys: impl Into<PathBuf>, dev: impl Into<PathBuf>, group_file: impl Into<PathBuf>) -> Self {
        Self { sys: sys.into(), dev: dev.into(), group_file: group_file.into() }
    }

    fn port_info(&self, name: &str, by_id: &[(String, String)]) -> Option<PortInfo> {
        let class = self.sys.join("class/tty").join(name);
        let device = fs::canonicalize(class.join("device")).ok()?;
        // Serial-core ports without hardware (e.g. unused ttyS*) report type 0.
        if read_trim(&class.join("type")).as_deref() == Some("0") {
            return None;
        }
        let mut info = PortInfo {
            path: self.dev.join(name).to_string_lossy().into_owned(),
            by_id: by_id.iter().find(|(_, target)| target == name).map(|(link, _)| link.clone()),
            ..PortInfo::default()
        };
        let usb = device.ancestors().take_while(|p| p.starts_with(&self.sys)).find(|p| p.join("idVendor").is_file());
        if let Some(usb) = usb {
            let hex = |f: &str| read_trim(&usb.join(f)).and_then(|v| u16::from_str_radix(&v, 16).ok());
            info.vid = hex("idVendor");
            info.pid = hex("idProduct");
            info.serial_number = read_trim(&usb.join("serial"));
            let words: Vec<String> =
                ["manufacturer", "product"].iter().filter_map(|f| read_trim(&usb.join(f))).collect();
            info.description = (!words.is_empty()).then(|| words.join(" "));
        }
        if info.description.is_none() {
            info.description = fs::read_link(device.join("driver"))
                .ok()
                .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()));
        }
        Some(info)
    }

    /// `/dev/serial/by-id` links as (link path, target device name).
    fn by_id_links(&self) -> Vec<(String, String)> {
        let Ok(dir) = fs::read_dir(self.dev.join("serial/by-id")) else { return Vec::new() };
        let mut links: Vec<_> = dir
            .flatten()
            .filter_map(|e| {
                let target = fs::read_link(e.path()).ok()?;
                Some((e.path().to_string_lossy().into_owned(), target.file_name()?.to_string_lossy().into_owned()))
            })
            .collect();
        links.sort();
        links
    }

    /// Owning group name of `path` from the group file, or `None` (S5).
    fn owning_group(&self, path: &str) -> Option<String> {
        let gid = fs::metadata(path).ok()?.gid();
        group_name(&fs::read_to_string(&self.group_file).ok()?, gid).or_else(|| Some(gid.to_string()))
    }

    fn open_error(&self, path: &str, cfg: &LineConfig, e: &serialport::Error) -> Error {
        match e.kind {
            ErrorKind::NoDevice => Error::Busy(path.into()),
            ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => {
                Error::PermissionDenied(permission_message(path, self.owning_group(path).as_deref()))
            }
            ErrorKind::Io(std::io::ErrorKind::NotFound) => Error::NotFound(format!("serial port {path}")),
            ErrorKind::InvalidInput => {
                Error::Unsupported(format!("serial settings {cfg} on {path}: {}", e.description))
            }
            _ if !Path::new(path).exists() => Error::NotFound(format!("serial port {path}")),
            _ => Error::Io(format!("opening {path}: {}", e.description)),
        }
    }
}

/// User-facing permission message; never suggests anything but joining the group (S5).
pub fn permission_message(path: &str, group: Option<&str>) -> String {
    match group {
        Some(g) => format!(
            "{path} (group `{g}`). Add your user to the group (`sudo usermod -aG {g} $USER`) and log out and back in."
        ),
        None => format!("{path}. Ask an administrator for access to the device."),
    }
}

/// Group name for `gid` in `/etc/group` content.
pub fn group_name(groups: &str, gid: u32) -> Option<String> {
    groups.lines().find_map(|l| {
        let mut f = l.split(':');
        let name = f.next()?;
        (f.nth(1)?.trim().parse::<u32>().ok()? == gid).then(|| name.to_string())
    })
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Settings the backend cannot express are rejected before opening (spec: name the setting).
fn backend_parity(p: Parity) -> Result<serialport::Parity> {
    match p {
        Parity::None => Ok(serialport::Parity::None),
        Parity::Odd => Ok(serialport::Parity::Odd),
        Parity::Even => Ok(serialport::Parity::Even),
        Parity::Mark | Parity::Space => {
            Err(Error::Unsupported(format!("parity {p:?} on Linux serial ports").to_lowercase()))
        }
    }
}

fn backend_data_bits(d: DataBits) -> serialport::DataBits {
    match d {
        DataBits::Five => serialport::DataBits::Five,
        DataBits::Six => serialport::DataBits::Six,
        DataBits::Seven => serialport::DataBits::Seven,
        DataBits::Eight => serialport::DataBits::Eight,
    }
}

fn backend_stop_bits(s: StopBits) -> serialport::StopBits {
    match s {
        StopBits::One => serialport::StopBits::One,
        StopBits::Two => serialport::StopBits::Two,
    }
}

fn backend_flow(f: FlowControl) -> serialport::FlowControl {
    match f {
        FlowControl::None => serialport::FlowControl::None,
        FlowControl::Hardware => serialport::FlowControl::Hardware,
        FlowControl::Software => serialport::FlowControl::Software,
    }
}

fn check_baud(port: &TTYPort, path: &str, want: u32) -> Result<()> {
    let got = port.baud_rate().unwrap_or(0);
    if want == 0 || u64::from(got.abs_diff(want)) * 100 > u64::from(want) * BAUD_TOLERANCE_PCT {
        return Err(Error::Unsupported(format!("baud rate {want} on {path} (device applied {got})")));
    }
    Ok(())
}

impl SerialPorts for LinuxSerial {
    fn list(&self) -> Result<Vec<PortInfo>> {
        let by_id = self.by_id_links();
        let mut ports: Vec<PortInfo> = fs::read_dir(self.sys.join("class/tty"))?
            .flatten()
            .filter_map(|e| self.port_info(&e.file_name().to_string_lossy(), &by_id))
            .collect();
        ports.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(ports)
    }

    fn open(&self, path: &str, cfg: &LineConfig) -> Result<Box<dyn SerialStream>> {
        let parity = backend_parity(cfg.parity)?;
        let port = serialport::new(path, cfg.baud)
            .data_bits(backend_data_bits(cfg.data_bits))
            .parity(parity)
            .stop_bits(backend_stop_bits(cfg.stop_bits))
            .flow_control(backend_flow(cfg.flow))
            .timeout(READ_TIMEOUT)
            .exclusive(true)
            .open_native()
            .map_err(|e| self.open_error(path, cfg, &e))?;
        // Dropping `port` on error closes it: no half-configured port stays open.
        check_baud(&port, path, cfg.baud)?;
        Ok(Box::new(LinuxStream { port, path: path.into() }))
    }
}

struct LinuxStream {
    port: TTYPort,
    path: String,
}

impl LinuxStream {
    /// I/O failure on an open port: the device went away (EIO/ENXIO/ENODEV, hangup) (S4).
    fn gone(&self, e: &dyn std::fmt::Display) -> Error {
        tracing::debug!(path = %self.path, error = %e, "serial device gone");
        Error::Gone(self.path.clone())
    }

    /// Control call failure: `Gone` when the device vanished, else the line does not support it.
    fn control_error(&self, what: &str, e: &serialport::Error) -> Error {
        if Path::new(&self.path).exists() {
            Error::Unsupported(format!("{what} on {} ({})", self.path, e.description))
        } else {
            self.gone(e)
        }
    }
}

fn is_timeout(e: &std::io::Error) -> bool {
    use std::io::ErrorKind as K;
    matches!(e.kind(), K::TimedOut | K::WouldBlock | K::Interrupted)
}

impl SerialStream for LinuxStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.port.read(buf) {
            // Raw mode (VMIN=1) only returns 0 bytes on a hung-up tty.
            Ok(0) => Err(self.gone(&"hangup")),
            Ok(n) => Ok(n),
            Err(e) if is_timeout(&e) => Ok(0),
            Err(e) => Err(self.gone(&e)),
        }
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> Result<()> {
        let deadline = Instant::now() + WRITE_DEADLINE;
        while !bytes.is_empty() {
            match self.port.write(bytes) {
                Ok(0) => return Err(self.gone(&"write returned 0")),
                Ok(n) => bytes = &bytes[n..],
                Err(e) if is_timeout(&e) && Instant::now() < deadline => {}
                Err(e) if is_timeout(&e) => {
                    return Err(Error::Io(format!("write to {} blocked by flow control", self.path)));
                }
                Err(e) => return Err(self.gone(&e)),
            }
        }
        Ok(())
    }

    fn set_lines(&mut self, dtr: Option<bool>, rts: Option<bool>) -> Result<()> {
        if let Some(v) = dtr {
            self.port.write_data_terminal_ready(v).map_err(|e| self.control_error("DTR control", &e))?;
        }
        if let Some(v) = rts {
            self.port.write_request_to_send(v).map_err(|e| self.control_error("RTS control", &e))?;
        }
        Ok(())
    }

    fn send_break(&mut self, dur: Duration) -> Result<()> {
        self.port.set_break().map_err(|e| self.control_error("break", &e))?;
        std::thread::sleep(dur);
        self.port.clear_break().map_err(|e| self.control_error("break", &e))
    }

    fn reconfigure(&mut self, cfg: &LineConfig) -> Result<()> {
        let parity = backend_parity(cfg.parity)?;
        let err = |e: serialport::Error| {
            Error::Unsupported(format!("serial settings {cfg} on {}: {}", self.path, e.description))
        };
        self.port.set_parity(parity).map_err(err)?;
        self.port.set_data_bits(backend_data_bits(cfg.data_bits)).map_err(err)?;
        self.port.set_stop_bits(backend_stop_bits(cfg.stop_bits)).map_err(err)?;
        self.port.set_flow_control(backend_flow(cfg.flow)).map_err(err)?;
        self.port.set_baud_rate(cfg.baud).map_err(err)?;
        check_baud(&self.port, &self.path, cfg.baud)
    }

    fn try_clone_writer(&self) -> Result<Box<dyn SerialStream>> {
        let port = self.port.try_clone_native().map_err(|e| Error::Io(format!("{}: {}", self.path, e.description)))?;
        Ok(Box::new(LinuxStream { port, path: self.path.clone() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    /// Fixture: one FTDI adapter (by-id link), one on-board UART, one unused ttyS, tty0 (no device).
    fn fixture() -> (PathBuf, LinuxSerial) {
        let root = std::env::temp_dir().join(format!("rxt-sysfs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (sys, dev) = (root.join("sys"), root.join("dev"));
        let usb = sys.join("devices/pci0000:00/usb1/1-2");
        let iface = usb.join("1-2:1.0/ttyUSB0");
        for (f, v) in [
            ("idVendor", "0403\n"),
            ("idProduct", "6001\n"),
            ("serial", "A10K\n"),
            ("manufacturer", "FTDI\n"),
            ("product", "FT232R USB UART\n"),
        ] {
            write(&usb.join(f), v);
        }
        fs::create_dir_all(&iface).unwrap();
        let uart = sys.join("devices/platform/serial8250/tty/ttyS0");
        let unused = sys.join("devices/platform/serial8250/tty/ttyS1");
        let drivers = sys.join("bus/platform/drivers/serial8250");
        fs::create_dir_all(&drivers).unwrap();
        fs::create_dir_all(&unused).unwrap();
        fs::create_dir_all(&uart).unwrap();
        symlink(&drivers, uart.join("driver")).unwrap();
        let class = sys.join("class/tty");
        fs::create_dir_all(class.join("tty0")).unwrap();
        for (name, target) in [("ttyUSB0", &iface), ("ttyS0", &uart), ("ttyS1", &unused)] {
            fs::create_dir_all(class.join(name)).unwrap();
            symlink(target, class.join(name).join("device")).unwrap();
        }
        write(&class.join("ttyS0/type"), "4\n");
        write(&class.join("ttyS1/type"), "0\n");
        fs::create_dir_all(dev.join("serial/by-id")).unwrap();
        symlink("../../ttyUSB0", dev.join("serial/by-id/usb-FTDI_FT232R_USB_UART_A10K-if00-port0")).unwrap();
        let group = root.join("group");
        write(&group, "root:x:0:\ndialout:x:20:alice\nuucp:x:14:\n");
        (root.clone(), LinuxSerial::with_roots(sys, dev, group))
    }

    #[test]
    fn sysfs_enumeration_with_usb_metadata_and_by_id() {
        let (root, s) = fixture();
        let ports = s.list().unwrap();
        let dev = root.join("dev");
        assert_eq!(ports.len(), 2, "{ports:#?}");
        let uart = &ports[0];
        assert_eq!(uart.path, dev.join("ttyS0").to_string_lossy());
        assert_eq!(uart.description.as_deref(), Some("serial8250"));
        assert_eq!((uart.vid, uart.by_id.as_deref()), (None, None));
        let usb = &ports[1];
        assert_eq!(usb.path, dev.join("ttyUSB0").to_string_lossy());
        assert_eq!((usb.vid, usb.pid), (Some(0x0403), Some(0x6001)));
        assert_eq!(usb.serial_number.as_deref(), Some("A10K"));
        assert_eq!(usb.description.as_deref(), Some("FTDI FT232R USB UART"));
        assert!(usb.by_id.as_deref().unwrap().ends_with("serial/by-id/usb-FTDI_FT232R_USB_UART_A10K-if00-port0"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn group_lookup_and_permission_message() {
        let groups = "root:x:0:\ndialout:x:20:alice\nbroken\nuucp:x:14:\n";
        assert_eq!(group_name(groups, 20).as_deref(), Some("dialout"));
        assert_eq!(group_name(groups, 14).as_deref(), Some("uucp"));
        assert_eq!(group_name(groups, 99), None);
        let m = permission_message("/dev/ttyUSB0", Some("dialout"));
        assert!(
            m.contains("/dev/ttyUSB0") && m.contains("group `dialout`") && m.contains("sudo usermod -aG dialout $USER")
        );
        assert!(Error::PermissionDenied(m).to_string().starts_with("permission denied: /dev/ttyUSB0"));
        assert!(!permission_message("/dev/ttyS0", None).contains("usermod"));
    }

    #[test]
    fn open_errors_are_typed() {
        let s = LinuxSerial::default();
        let cfg = LineConfig::default();
        let e = s.open("/dev/rxt-no-such-port", &cfg).err().unwrap();
        assert!(matches!(e, Error::NotFound(_)), "{e}");
        let mark = LineConfig { parity: Parity::Mark, ..cfg };
        let e = s.open("/dev/rxt-no-such-port", &mark).err().unwrap();
        assert!(matches!(&e, Error::Unsupported(m) if m.contains("parity mark")), "{e}");
    }
}
