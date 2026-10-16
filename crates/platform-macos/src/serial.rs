//! Serial ports on macOS through `serialport` (IOKit enumeration in the crate). Only the
//! callout devices `/dev/cu.*` are listed; the matching `/dev/tty.*` dial-in twins are dropped
//! (opening a `tty.*` blocks on carrier detect). Ports open exclusively, so a second open is
//! reported as busy.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use rxt_core::{DataBits, FlowControl, LineConfig, Parity, StopBits};
use rxt_platform::{Error, PortInfo, Result, SerialPorts, SerialStream};
use serialport::{SerialPort, SerialPortType, TTYPort};

const READ_TIMEOUT: Duration = Duration::from_millis(100);
const WRITE_DEADLINE: Duration = Duration::from_secs(10);

/// macOS serial ports.
pub struct MacSerial;

/// A callout device `/dev/cu.*` (not its `/dev/tty.*` dial-in twin).
fn is_callout(name: &str) -> bool {
    name.starts_with("/dev/cu.")
}

/// Whether `path` is still enumerated (to tell "gone" from other errors).
fn listed(path: &str) -> bool {
    serialport::available_ports().is_ok_and(|ports| ports.iter().any(|p| p.port_name == path))
}

fn parity(p: Parity) -> Result<serialport::Parity> {
    match p {
        Parity::None => Ok(serialport::Parity::None),
        Parity::Odd => Ok(serialport::Parity::Odd),
        Parity::Even => Ok(serialport::Parity::Even),
        Parity::Mark | Parity::Space => {
            Err(Error::Unsupported(format!("parity {p:?} on macOS serial ports").to_lowercase()))
        }
    }
}

fn data_bits(d: DataBits) -> serialport::DataBits {
    match d {
        DataBits::Five => serialport::DataBits::Five,
        DataBits::Six => serialport::DataBits::Six,
        DataBits::Seven => serialport::DataBits::Seven,
        DataBits::Eight => serialport::DataBits::Eight,
    }
}

fn stop_bits(s: StopBits) -> serialport::StopBits {
    match s {
        StopBits::One => serialport::StopBits::One,
        StopBits::Two => serialport::StopBits::Two,
    }
}

fn flow(f: FlowControl) -> serialport::FlowControl {
    match f {
        FlowControl::None => serialport::FlowControl::None,
        FlowControl::Hardware => serialport::FlowControl::Hardware,
        FlowControl::Software => serialport::FlowControl::Software,
    }
}

/// `ENXIO`/`ENODEV`: the device was unplugged.
fn serial_gone(raw: Option<i32>) -> bool {
    matches!(raw, Some(6) | Some(19))
}

impl SerialPorts for MacSerial {
    fn list(&self) -> Result<Vec<PortInfo>> {
        let ports = serialport::available_ports().map_err(|e| Error::Io(format!("serial ports: {e}")))?;
        let mut out: Vec<PortInfo> = ports
            .into_iter()
            .filter(|p| is_callout(&p.port_name))
            .map(|p| {
                let mut info = PortInfo { path: p.port_name.clone(), ..PortInfo::default() };
                if let SerialPortType::UsbPort(usb) = p.port_type {
                    info.vid = Some(usb.vid);
                    info.pid = Some(usb.pid);
                    info.serial_number = usb.serial_number;
                    let words: Vec<String> = [usb.manufacturer, usb.product].into_iter().flatten().collect();
                    info.description = (!words.is_empty()).then(|| words.join(" "));
                }
                info
            })
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    fn open(&self, path: &str, cfg: &LineConfig) -> Result<Box<dyn SerialStream>> {
        let port = serialport::new(path, cfg.baud)
            .data_bits(data_bits(cfg.data_bits))
            .parity(parity(cfg.parity)?)
            .stop_bits(stop_bits(cfg.stop_bits))
            .flow_control(flow(cfg.flow))
            .timeout(READ_TIMEOUT)
            .open_native()
            .map_err(|e| match e.kind {
                serialport::ErrorKind::NoDevice => Error::Busy(path.into()),
                serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => {
                    Error::PermissionDenied(format!("{path} (in use or not permitted)"))
                }
                serialport::ErrorKind::Io(std::io::ErrorKind::NotFound) => {
                    Error::NotFound(format!("serial port {path}"))
                }
                serialport::ErrorKind::InvalidInput => {
                    Error::Unsupported(format!("serial settings {cfg} on {path}: {}", e.description))
                }
                _ if !listed(path) => Error::NotFound(format!("serial port {path}")),
                _ => Error::Io(format!("opening {path}: {}", e.description)),
            })?;
        Ok(Box::new(MacStream { port, path: path.to_owned() }))
    }

    fn example_port(&self) -> &'static str {
        "/dev/cu.usbserial"
    }
}

struct MacStream {
    port: TTYPort,
    path: String,
}

impl MacStream {
    fn io_error(&self, e: &std::io::Error) -> Error {
        // serialport wraps some tty read/write errors with `io::Error::new`, dropping the raw
        // errno, so `serial_gone` alone can miss an unplug. Fall back to a device-presence check:
        // a port that vanished from enumeration is a hangup → `Gone`.
        if serial_gone(e.raw_os_error()) || !listed(&self.path) {
            tracing::debug!(path = %self.path, error = %e, "serial device gone");
            Error::Gone(self.path.clone())
        } else {
            Error::Io(format!("{}: {e}", self.path))
        }
    }

    fn control_error(&self, what: &str, e: &serialport::Error) -> Error {
        if !listed(&self.path) {
            Error::Gone(self.path.clone())
        } else {
            Error::Unsupported(format!("{what} on {} ({})", self.path, e.description))
        }
    }
}

fn is_timeout(e: &std::io::Error) -> bool {
    use std::io::ErrorKind as K;
    matches!(e.kind(), K::TimedOut | K::WouldBlock | K::Interrupted)
}

impl SerialStream for MacStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.port.read(buf) {
            // A read that returns 0 bytes with success is a tty hangup (device gone), as on Linux;
            // a real timeout surfaces as `TimedOut`/`WouldBlock` below → `Ok(0)` (no data).
            Ok(0) => Err(Error::Gone(self.path.clone())),
            Ok(n) => Ok(n),
            Err(e) if is_timeout(&e) => Ok(0),
            Err(e) => Err(self.io_error(&e)),
        }
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> Result<()> {
        let deadline = Instant::now() + WRITE_DEADLINE;
        while !bytes.is_empty() {
            match self.port.write(bytes) {
                Ok(0) => return Err(Error::Gone(self.path.clone())),
                Ok(n) => bytes = &bytes[n..],
                Err(e) if is_timeout(&e) && Instant::now() < deadline => {}
                Err(e) if is_timeout(&e) => {
                    return Err(Error::Io(format!("write to {} blocked by flow control", self.path)));
                }
                Err(e) => return Err(self.io_error(&e)),
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
        let p = parity(cfg.parity)?;
        let err = |e: serialport::Error| {
            Error::Unsupported(format!("serial settings {cfg} on {}: {}", self.path, e.description))
        };
        self.port.set_parity(p).map_err(err)?;
        self.port.set_data_bits(data_bits(cfg.data_bits)).map_err(err)?;
        self.port.set_stop_bits(stop_bits(cfg.stop_bits)).map_err(err)?;
        self.port.set_flow_control(flow(cfg.flow)).map_err(err)?;
        self.port.set_baud_rate(cfg.baud).map_err(err)
    }

    fn try_clone_writer(&self) -> Result<Box<dyn SerialStream>> {
        let port = self.port.try_clone_native().map_err(|e| Error::Io(format!("{}: {}", self.path, e.description)))?;
        Ok(Box::new(MacStream { port, path: self.path.clone() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callout_filter_drops_dialin_twins() {
        assert!(is_callout("/dev/cu.usbserial-1420"));
        assert!(!is_callout("/dev/tty.usbserial-1420"));
        assert!(!is_callout("/dev/ttyS0"));
    }

    #[test]
    fn enumeration_never_fails_and_missing_port_is_not_found() {
        // Runners and VMs may have zero serial ports; listing must still succeed.
        let ports = MacSerial.list().unwrap();
        assert!(ports.iter().all(|p| p.path.starts_with("/dev/cu.")));
        let e = MacSerial.open("/dev/cu.nonexistent-rxt", &LineConfig::default()).err().unwrap();
        assert!(matches!(e, Error::NotFound(_) | Error::Busy(_)), "{e}");
    }
}
