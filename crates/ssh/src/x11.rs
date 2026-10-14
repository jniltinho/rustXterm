//! X11 forwarding: fake MIT-MAGIC-COOKIE-1 per session, cookie validation and
//! substitution of the local display's real cookie (design D4).

use std::path::PathBuf;

use russh::Channel;
use russh::client::Msg;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MIT_COOKIE: &[u8] = b"MIT-MAGIC-COOKIE-1";

/// Why X11 forwarding is off when [`X11::from_env`] finds no usable display.
pub const UNAVAILABLE: &str = if cfg!(windows) {
    "no local X server is configured (start VcXsrv or X410 and set DISPLAY=127.0.0.1:0)"
} else {
    "no usable $DISPLAY"
};

/// Parsed `$DISPLAY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    /// Empty (or `unix`) means the local unix socket.
    pub host: String,
    pub number: u32,
    pub screen: u32,
}

impl Display {
    pub fn parse(s: &str) -> Option<Self> {
        let (host, rest) = s.rsplit_once(':')?;
        let (number, screen) = match rest.split_once('.') {
            Some((n, sc)) => (n.parse().ok()?, sc.parse().ok()?),
            None => (rest.parse().ok()?, 0),
        };
        let host = if host == "unix" { String::new() } else { host.to_owned() };
        Some(Self { host, number, screen })
    }

    fn is_unix(&self) -> bool {
        self.host.is_empty()
    }
}

/// X11 forwarding state for one session.
#[derive(Debug, Clone)]
pub struct X11 {
    pub fake: [u8; 16],
    pub display: Display,
    /// Real cookie from Xauthority; `None` forwards without authentication.
    pub real: Option<Vec<u8>>,
}

impl X11 {
    /// From `$DISPLAY` and `$XAUTHORITY` (or `~/.Xauthority`); `None` without a display.
    pub fn from_env() -> Option<Self> {
        let display = Display::parse(&std::env::var("DISPLAY").ok()?)?;
        // Without Unix sockets only TCP displays (`127.0.0.1:0`) can be reached.
        if cfg!(not(unix)) && display.is_unix() {
            return None;
        }
        let path = std::env::var_os("XAUTHORITY")
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|h| h.join(".Xauthority")));
        let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
        let real = path
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|data| find_cookie(&data, display.number, hostname.trim()));
        if real.is_none() {
            let n = display.number;
            tracing::warn!("no X11 cookie found for display :{n}; forwarding without authentication");
        }
        Some(Self { fake: rand::random(), display, real })
    }

    pub fn fake_hex(&self) -> String {
        data_encoding::HEXLOWER.encode(&self.fake)
    }

    /// Serve one server-opened X11 channel: validate the fake cookie, then proxy to the local display.
    pub async fn serve(self, channel: Channel<Msg>) {
        let mut remote = channel.into_stream();
        let header = match read_setup(&mut remote).await {
            Ok(h) => h,
            Err(e) => return tracing::warn!("X11 channel: {e}"),
        };
        let Some(setup) = rewrite_setup(&header, &self.fake, self.real.as_deref()) else {
            return tracing::warn!("X11 channel rejected: wrong or missing cookie");
        };
        let result = async {
            if self.display.is_unix() {
                #[cfg(unix)]
                {
                    let path = format!("/tmp/.X11-unix/X{}", self.display.number);
                    let local = tokio::net::UnixStream::connect(path).await?;
                    return proxy(local, remote, &setup).await;
                }
                #[cfg(not(unix))]
                return Err(std::io::Error::other("unix X11 sockets unsupported"));
            }
            let port = 6000 + u16::try_from(self.display.number).map_err(std::io::Error::other)?;
            let local = tokio::net::TcpStream::connect((self.display.host.as_str(), port)).await?;
            proxy(local, remote, &setup).await
        };
        if let Err(e) = result.await {
            tracing::debug!("X11 channel closed: {e}");
        }
    }
}

async fn proxy<L, R>(mut local: L, mut remote: R, setup: &[u8]) -> std::io::Result<()>
where
    L: AsyncRead + AsyncWrite + Unpin,
    R: AsyncRead + AsyncWrite + Unpin,
{
    local.write_all(setup).await?;
    tokio::io::copy_bidirectional(&mut local, &mut remote).await.map(|_| ())
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

/// Read the client connection setup (12-byte prefix + padded auth name and data).
async fn read_setup<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; 12];
    r.read_exact(&mut buf).await?;
    let (n, d) = auth_lens(&buf).ok_or_else(|| std::io::Error::other("bad X11 byte order"))?;
    let mut rest = vec![0u8; pad4(n) + pad4(d)];
    r.read_exact(&mut rest).await?;
    buf.extend_from_slice(&rest);
    Ok(buf)
}

fn auth_lens(h: &[u8]) -> Option<(usize, usize)> {
    let u16_at = |i: usize| -> Option<usize> {
        let b = [*h.get(i)?, *h.get(i + 1)?];
        Some(usize::from(match h.first()? {
            b'B' => u16::from_be_bytes(b),
            b'l' => u16::from_le_bytes(b),
            _ => return None,
        }))
    };
    Some((u16_at(6)?, u16_at(8)?))
}

/// Validate the fake cookie in a full setup message and rebuild it with `real` (or no auth).
/// `None` when the cookie is wrong.
pub fn rewrite_setup(setup: &[u8], fake: &[u8], real: Option<&[u8]>) -> Option<Vec<u8>> {
    let (n, d) = auth_lens(setup)?;
    let name = setup.get(12..12 + n)?;
    let data = setup.get(12 + pad4(n)..12 + pad4(n) + d)?;
    if name != MIT_COOKIE || data != fake {
        return None;
    }
    let (name, data): (&[u8], &[u8]) = match real {
        Some(r) => (MIT_COOKIE, r),
        None => (b"", b""),
    };
    let big = setup.first() == Some(&b'B');
    let enc = |v: usize| -> Option<[u8; 2]> {
        let v = u16::try_from(v).ok()?;
        Some(if big { v.to_be_bytes() } else { v.to_le_bytes() })
    };
    let mut out = setup.get(..6)?.to_vec();
    out.extend_from_slice(&enc(name.len())?);
    out.extend_from_slice(&enc(data.len())?);
    out.extend_from_slice(&[0, 0]);
    for part in [name, data] {
        out.extend_from_slice(part);
        out.resize(pad4(out.len()), 0);
    }
    Some(out)
}

/// Find the MIT-MAGIC-COOKIE-1 for `display` in an Xauthority file.
/// Prefers FamilyWild or FamilyLocal entries for this host, then any entry for the display.
pub fn find_cookie(data: &[u8], display: u32, hostname: &str) -> Option<Vec<u8>> {
    const LOCAL: u16 = 256;
    const WILD: u16 = 65535;
    let number = display.to_string();
    let mut fallback = None;
    let mut rest = data;
    fn take(rest: &mut &[u8]) -> Option<Vec<u8>> {
        let len = usize::from(u16::from_be_bytes([*rest.first()?, *rest.get(1)?]));
        let field = rest.get(2..2 + len)?.to_vec();
        *rest = &rest[2 + len..];
        Some(field)
    }
    while rest.len() >= 2 {
        let family = u16::from_be_bytes([rest[0], rest[1]]);
        rest = &rest[2..];
        let (addr, num, name, cookie) = (take(&mut rest)?, take(&mut rest)?, take(&mut rest)?, take(&mut rest)?);
        if num != number.as_bytes() || name != MIT_COOKIE {
            continue;
        }
        if family == WILD || (family == LOCAL && addr == hostname.as_bytes()) {
            return Some(cookie);
        }
        fallback.get_or_insert(cookie);
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_notice_names_display() {
        assert!(UNAVAILABLE.contains("DISPLAY"));
        assert_eq!(cfg!(windows), UNAVAILABLE.contains("DISPLAY=127.0.0.1:0"));
    }

    fn setup(big: bool, name: &[u8], data: &[u8]) -> Vec<u8> {
        let enc = |v: usize| if big { (v as u16).to_be_bytes() } else { (v as u16).to_le_bytes() };
        let mut v = vec![if big { b'B' } else { b'l' }, 0];
        v.extend_from_slice(&enc(11));
        v.extend_from_slice(&enc(0));
        v.extend_from_slice(&enc(name.len()));
        v.extend_from_slice(&enc(data.len()));
        v.extend_from_slice(&[0, 0]);
        for p in [name, data] {
            v.extend_from_slice(p);
            v.resize(pad4(v.len()), 0);
        }
        v
    }

    #[test]
    fn display_parse() {
        assert_eq!(Display::parse(":0"), Some(Display { host: String::new(), number: 0, screen: 0 }));
        assert_eq!(Display::parse("unix:1.2"), Some(Display { host: String::new(), number: 1, screen: 2 }));
        assert_eq!(Display::parse("10.0.0.1:10.0").map(|d| (d.host, d.number)), Some(("10.0.0.1".into(), 10)));
        assert_eq!(Display::parse("nodisplay"), None);
    }

    #[test]
    fn cookie_validation_and_substitution() {
        let fake = [9u8; 16];
        let real = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        for big in [true, false] {
            let good = setup(big, MIT_COOKIE, &fake);
            assert_eq!(read_lens(&good), (18, 16));
            assert_eq!(rewrite_setup(&good, &fake, Some(&real)), Some(setup(big, MIT_COOKIE, &real)));
            assert_eq!(rewrite_setup(&good, &fake, None), Some(setup(big, b"", b"")));
            assert_eq!(rewrite_setup(&setup(big, MIT_COOKIE, &[8u8; 16]), &fake, Some(&real)), None);
            assert_eq!(rewrite_setup(&setup(big, b"XDM-AUTHORIZATION-1", &fake), &fake, Some(&real)), None);
            assert_eq!(rewrite_setup(&setup(big, b"", b""), &fake, Some(&real)), None);
            assert_eq!(rewrite_setup(&good[..20], &fake, Some(&real)), None);
        }
    }

    fn read_lens(h: &[u8]) -> (usize, usize) {
        auth_lens(h).unwrap()
    }

    #[test]
    fn fake_cookie_is_random_per_session() {
        let d = Display { host: String::new(), number: 0, screen: 0 };
        let a = X11 { fake: rand::random(), display: d.clone(), real: None };
        let b = X11 { fake: rand::random(), display: d, real: None };
        assert_ne!(a.fake, b.fake);
        assert_eq!(a.fake_hex().len(), 32);
    }

    #[test]
    fn xauthority_lookup() {
        let rec = |family: u16, addr: &[u8], num: &[u8], cookie: &[u8]| {
            let mut v = family.to_be_bytes().to_vec();
            for f in [addr, num, MIT_COOKIE, cookie] {
                v.extend_from_slice(&(f.len() as u16).to_be_bytes());
                v.extend_from_slice(f);
            }
            v
        };
        let mut file = rec(0, b"\x7f\0\0\x01", b"0", b"tcp");
        file.extend(rec(256, b"other", b"0", b"wrong-host"));
        file.extend(rec(256, b"box", b"0", b"right"));
        file.extend(rec(256, b"box", b"1", b"display1"));
        assert_eq!(find_cookie(&file, 0, "box"), Some(b"right".to_vec()));
        assert_eq!(find_cookie(&file, 1, "box"), Some(b"display1".to_vec()));
        assert_eq!(find_cookie(&file, 0, "nohost"), Some(b"tcp".to_vec()));
        assert_eq!(find_cookie(&file, 5, "box"), None);
        assert_eq!(find_cookie(&file[..10], 0, "box"), None);
    }
}
