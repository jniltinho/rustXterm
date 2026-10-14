//! Quick-connect parser: `[user@]host[:port]`, `scheme://[user@]host[:port]` or
//! `serial://<device>[?baud=&data=&parity=&stop=&flow=]` (`COM<n>` or `\\.\COM<n>` on Windows).

use crate::{DataBits, Error, FlowControl, LineConfig, Parity, Protocol, Result, Session, StopBits};

/// Parse quick-connect input into an unsaved session.
pub fn parse(input: &str) -> Result<Session> {
    parse_for(input, cfg!(windows))
}

/// [`parse`] for a target OS: on Windows a serial device name is used as typed (`COM1`,
/// `\\.\COM1`), elsewhere a bare name is taken under `/dev`.
pub fn parse_for(input: &str, windows: bool) -> Result<Session> {
    let input = input.trim();
    let (protocol, rest) = match input.split_once("://") {
        Some((scheme, rest)) => {
            let p =
                Protocol::from_scheme(scheme).ok_or_else(|| Error::Invalid(format!("unknown protocol '{scheme}'")))?;
            (p, rest)
        }
        None => (Protocol::Ssh, input),
    };
    if protocol == Protocol::Serial {
        return parse_serial(rest, windows);
    }
    let rest = rest.trim_end_matches('/');
    let (user, hostport) = match rest.rsplit_once('@') {
        Some((u, h)) if !u.is_empty() => (Some(u.to_string()), h),
        Some(_) => return Err(Error::Invalid("empty user name".into())),
        None => (None, rest),
    };
    let (host, port) = split_host_port(hostport)?;
    if host.is_empty() {
        return Err(Error::Invalid("missing host".into()));
    }
    if host.chars().any(|c| c.is_whitespace() || c == '/') {
        return Err(Error::Invalid(format!("invalid host '{host}'")));
    }
    Ok(Session::new(protocol, host, user, port))
}

/// `serial:///dev/ttyUSB0?baud=9600` or `serial://ttyUSB0` (a name under `/dev`); on Windows
/// `serial://COM1` or `serial://\\.\COM1`.
fn parse_serial(rest: &str, windows: bool) -> Result<Session> {
    let (device, query) = rest.split_once('?').unwrap_or((rest, ""));
    let device = device.trim_end_matches('/');
    if device.is_empty() || device.chars().any(char::is_whitespace) {
        return Err(Error::Invalid("missing serial device".into()));
    }
    let path = if windows || device.starts_with('/') { device.to_string() } else { format!("/dev/{device}") };
    let mut line = LineConfig::default();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let bad = || Error::Invalid(format!("invalid {key} '{value}'"));
        match key {
            "baud" => line.baud = value.parse().ok().filter(|b| *b > 0).ok_or_else(bad)?,
            "data" => line.data_bits = value.parse().ok().and_then(DataBits::from_bits).ok_or_else(bad)?,
            "parity" => line.parity = Parity::parse(value).ok_or_else(bad)?,
            "stop" => {
                line.stop_bits = match value {
                    "1" => StopBits::One,
                    "2" => StopBits::Two,
                    _ => return Err(bad()),
                }
            }
            "flow" => line.flow = FlowControl::parse(value).ok_or_else(bad)?,
            _ => return Err(Error::Invalid(format!("unknown serial parameter '{key}'"))),
        }
    }
    Ok(Session::serial(path, line))
}

fn split_host_port(s: &str) -> Result<(&str, Option<u16>)> {
    // Bracketed IPv6: [::1]:22
    if let Some(inner) = s.strip_prefix('[') {
        let (host, after) = inner.split_once(']').ok_or_else(|| Error::Invalid("unclosed '['".into()))?;
        return match after.strip_prefix(':') {
            Some(p) => Ok((host, Some(parse_port(p)?))),
            None if after.is_empty() => Ok((host, None)),
            None => Err(Error::Invalid(format!("unexpected '{after}'"))),
        };
    }
    match s.rsplit_once(':') {
        // Bare IPv6 without brackets: treat the whole string as host.
        Some((h, _)) if h.contains(':') => Ok((s, None)),
        Some((h, p)) => Ok((h, Some(parse_port(p)?))),
        None => Ok((s, None)),
    }
}

fn parse_port(p: &str) -> Result<u16> {
    match p.parse::<u16>() {
        Ok(0) | Err(_) => Err(Error::Invalid(format!("invalid port '{p}'"))),
        Ok(n) => Ok(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ssh() {
        let s = parse("alice@lab.example:2222").unwrap();
        assert_eq!(
            (s.protocol, s.host.as_str(), s.port, s.user.as_deref()),
            (Protocol::Ssh, "lab.example", 2222, Some("alice"))
        );
    }

    #[test]
    fn prefixes() {
        let cases = [
            ("ssh://h", Protocol::Ssh, None, 22),
            ("sftp://u@h", Protocol::Sftp, Some("u"), 22),
            ("ftp://u@h", Protocol::Ftp, Some("u"), 21),
            ("ftps://h:990", Protocol::Ftps, None, 990),
            ("rdp://h:3390", Protocol::Rdp, None, 3390),
            ("vnc://h", Protocol::Vnc, None, 5900),
            ("VNC://h/", Protocol::Vnc, None, 5900),
            ("telnet://h", Protocol::Telnet, None, 23),
            ("telnet://u@h:2323", Protocol::Telnet, Some("u"), 2323),
        ];
        for (input, p, user, port) in cases {
            let s = parse(input).unwrap();
            assert_eq!((s.protocol, s.user.as_deref(), s.port, s.host.as_str()), (p, user, port, "h"), "{input}");
        }
    }

    #[test]
    fn ipv6() {
        assert_eq!(parse("[::1]:2200").unwrap().port, 2200);
        assert_eq!(parse("root@[fe80::1]").unwrap().host, "fe80::1");
    }

    #[test]
    fn invalid() {
        for bad in ["", "ssh://", "ssh://u@", "@h", "telnet://", "telnet://u@", "h:0", "h:99999", "h:x", "a b", "[::1"]
        {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn telnet_never_auto_logins() {
        let s = parse("telnet://alice@router.lab:2323").unwrap();
        assert_eq!(
            (s.protocol, s.host.as_str(), s.port, s.user.as_deref()),
            (Protocol::Telnet, "router.lab", 2323, Some("alice"))
        );
        assert!(!s.telnet.auto_login);
    }

    #[test]
    fn serial_defaults_and_parameters() {
        let s = parse("serial:///dev/ttyUSB0").unwrap();
        assert_eq!((s.protocol, s.serial.port.as_str()), (Protocol::Serial, "/dev/ttyUSB0"));
        assert_eq!(s.serial.line, LineConfig::default());
        assert_eq!(s.name, "ttyUSB0 @ 115200");

        let s = parse("serial://ttyUSB0").unwrap();
        assert_eq!(s.serial.port, "/dev/ttyUSB0");

        let s = parse("serial:///dev/ttyS1?baud=9600&data=7&parity=e&stop=2&flow=rtscts").unwrap();
        let want = LineConfig {
            baud: 9600,
            data_bits: DataBits::Seven,
            parity: Parity::Even,
            stop_bits: StopBits::Two,
            flow: FlowControl::Hardware,
        };
        assert_eq!((s.serial.port.as_str(), s.serial.line), ("/dev/ttyS1", want));
        assert_eq!(parse("serial:///dev/ttyUSB0?baud=9600").unwrap().serial.line.to_string(), "9600 8N1");
    }

    #[test]
    fn serial_com_ports_on_windows() {
        for (input, port) in [("serial://COM1?baud=9600", "COM1"), (r"serial://\\.\COM12", r"\\.\COM12")] {
            let s = parse_for(input, true).unwrap();
            assert_eq!((s.protocol, s.serial.port.as_str()), (Protocol::Serial, port), "{input}");
        }
        assert_eq!(parse_for("serial://COM1?baud=9600", true).unwrap().serial.line.baud, 9600);
        assert_eq!(parse_for("serial://COM1", false).unwrap().serial.port, "/dev/COM1");
    }

    #[test]
    fn serial_invalid_names_the_parameter() {
        for (bad, key) in [
            ("serial:///dev/ttyUSB0?parity=q", "parity"),
            ("serial:///dev/ttyUSB0?baud=fast", "baud"),
            ("serial:///dev/ttyUSB0?baud=0", "baud"),
            ("serial:///dev/ttyUSB0?data=9", "data"),
            ("serial:///dev/ttyUSB0?stop=3", "stop"),
            ("serial:///dev/ttyUSB0?flow=x", "flow"),
            ("serial:///dev/ttyUSB0?speed=1", "speed"),
            ("serial://", "device"),
        ] {
            let err = parse(bad).unwrap_err().to_string();
            assert!(err.contains(key), "{bad}: {err}");
        }
    }
}
