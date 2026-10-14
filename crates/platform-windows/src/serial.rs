//! COM ports through `serialport` (SetupAPI enumeration in the crate), design W3. Windows opens
//! COM ports exclusively, so a second open is reported as busy.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use rxt_core::{DataBits, FlowControl, LineConfig, Parity, StopBits};
use rxt_platform::{Error, PortInfo, Result, SerialPorts, SerialStream};
use serialport::{COMPort, SerialPort, SerialPortType};

use crate::{com_device_path, com_name, serial_gone, serial_open_error};

const READ_TIMEOUT: Duration = Duration::from_millis(100);
const WRITE_DEADLINE: Duration = Duration::from_secs(10);

/// Windows COM ports.
pub struct WindowsSerial;

fn listed(name: &str) -> bool {
    serialport::available_ports().is_ok_and(|ports| ports.iter().any(|p| com_name(&p.port_name) == name))
}

fn parity(p: Parity) -> Result<serialport::Parity> {
    match p {
        Parity::None => Ok(serialport::Parity::None),
        Parity::Odd => Ok(serialport::Parity::Odd),
        Parity::Even => Ok(serialport::Parity::Even),
        Parity::Mark | Parity::Space => {
            Err(Error::Unsupported(format!("parity {p:?} on Windows serial ports").to_lowercase()))
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

impl SerialPorts for WindowsSerial {
    fn list(&self) -> Result<Vec<PortInfo>> {
        let ports = serialport::available_ports().map_err(|e| Error::Io(format!("serial ports: {e}")))?;
        let mut out: Vec<PortInfo> = ports
            .into_iter()
            .map(|p| {
                let mut info = PortInfo { path: com_name(&p.port_name), ..PortInfo::default() };
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
        // COM2 before COM10.
        out.sort_by_key(|p| (p.path.len(), p.path.clone()));
        Ok(out)
    }

    fn open(&self, path: &str, cfg: &LineConfig) -> Result<Box<dyn SerialStream>> {
        let name = com_name(path);
        let port = serialport::new(com_device_path(path), cfg.baud)
            .data_bits(data_bits(cfg.data_bits))
            .parity(parity(cfg.parity)?)
            .stop_bits(stop_bits(cfg.stop_bits))
            .flow_control(flow(cfg.flow))
            .timeout(READ_TIMEOUT)
            .open_native()
            .map_err(|e| {
                let no_device = e.kind == serialport::ErrorKind::NoDevice;
                serial_open_error(&name, no_device, no_device && listed(&name), &e.description)
            })?;
        Ok(Box::new(ComStream { port, path: name }))
    }

    fn example_port(&self) -> &'static str {
        "COM3"
    }
}

struct ComStream {
    port: COMPort,
    path: String,
}

impl ComStream {
    fn io_error(&self, e: &std::io::Error) -> Error {
        if serial_gone(e.raw_os_error()) {
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

impl SerialStream for ComStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.port.read(buf) {
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
        Ok(Box::new(ComStream { port, path: self.path.clone() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumeration_never_fails_and_missing_port_is_not_found() {
        // Runners and VMs may have zero COM ports; listing must still succeed.
        let ports = WindowsSerial.list().unwrap();
        assert!(ports.iter().all(|p| p.path.starts_with("COM") || p.path.starts_with(r"\\")));
        let e = WindowsSerial.open("COM250", &LineConfig::default()).err().unwrap();
        assert!(matches!(e, Error::NotFound(_)), "{e}");
    }
}
