use std::time::Instant;

use rxt_core::{LineConfig, Parity};
use rxt_platform::fake::FakeSerial;

use super::*;

const USB0: &str = "/dev/ttyUSB0";
const BY_ID: &str = "/dev/serial/by-id/usb-FTDI_A10K-if00-port0";

fn usb(path: &str) -> PortInfo {
    PortInfo {
        path: path.into(),
        by_id: Some(BY_ID.into()),
        vid: Some(0x0403),
        pid: Some(0x6001),
        serial_number: Some("A10K".into()),
        ..PortInfo::default()
    }
}

fn setup(opts: impl FnOnce(&mut SerialOptions)) -> (FakeSerial, SerialSession) {
    let fake = FakeSerial::default();
    fake.plug(usb(USB0));
    let mut o = SerialOptions { port: USB0.into(), ..SerialOptions::default() };
    opts(&mut o);
    let s = open(Arc::new(fake.clone()), &o, CancellationToken::new()).unwrap();
    (fake, s)
}

async fn wait_written(fake: &FakeSerial, path: &str, want: &[u8]) {
    let start = Instant::now();
    while fake.device(path).unwrap().written != want {
        assert!(start.elapsed() < Duration::from_secs(5), "written {:?}", fake.device(path).unwrap().written);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn enter_variants_reach_the_device() {
    for (enter, want) in
        [(SerialEnter::Cr, &b"help\r"[..]), (SerialEnter::Lf, b"help\n"), (SerialEnter::CrLf, b"help\r\n")]
    {
        let (fake, s) = setup(|o| o.enter = enter);
        s.writer().write(b"help\r").await.unwrap();
        wait_written(&fake, USB0, want).await;
    }
}

#[tokio::test]
async fn received_bytes_with_and_without_lf_expansion() {
    let (fake, mut s) = setup(|_| {});
    fake.send(USB0, b"ok\n");
    assert_eq!(s.read().await.unwrap().unwrap(), b"ok\n");
    let (fake, mut s) = setup(|o| o.lf_to_crlf = true);
    fake.send(USB0, b"a\nb\r");
    assert_eq!(s.read().await.unwrap().unwrap(), b"a\r\nb\r");
}

#[tokio::test]
async fn local_echo_shows_typed_input_once() {
    let (fake, mut s) = setup(|o| o.local_echo = true);
    s.writer().write(b"ls\r").await.unwrap();
    assert_eq!(s.read().await.unwrap().unwrap(), b"ls\r\n");
    wait_written(&fake, USB0, b"ls\r").await;
    let (_, mut quiet) = setup(|_| {});
    quiet.writer().write(b"x").await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(50), quiet.read()).await.is_err());
}

#[tokio::test]
async fn break_lines_and_initial_state() {
    let (fake, s) = setup(|o| {
        o.dtr = Some(true);
        o.rts = Some(false);
        o.line = LineConfig { baud: 9600, parity: Parity::Even, ..LineConfig::default() };
    });
    let d = fake.device(USB0).unwrap();
    assert_eq!((d.dtr, d.rts), (Some(true), Some(false)));
    assert_eq!(d.cfg.unwrap().baud, 9600);
    let w = s.writer();
    w.send_break(250).await.unwrap();
    w.set_lines(None, Some(true)).await.unwrap();
    let d = fake.device(USB0).unwrap();
    assert_eq!(d.breaks, vec![Duration::from_millis(250)]);
    assert_eq!((d.dtr, d.rts), (Some(true), Some(true)));
}

#[tokio::test]
async fn cancel_ends_reads_and_drop_releases_the_port() {
    let fake = FakeSerial::default();
    fake.plug(usb(USB0));
    let ports: Arc<dyn SerialPorts> = Arc::new(fake.clone());
    let opts = SerialOptions { port: USB0.into(), ..SerialOptions::default() };
    let cancel = CancellationToken::new();
    let mut s = open(ports.clone(), &opts, cancel.clone()).unwrap();
    assert!(matches!(open(ports.clone(), &opts, CancellationToken::new()), Err(Error::Busy(_))));
    cancel.cancel();
    assert!(s.read().await.is_none());
    drop(s);
    let start = Instant::now();
    while fake.device(USB0).unwrap().handles > 0 {
        assert!(start.elapsed() < Duration::from_secs(5), "port never released");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(open(ports, &opts, CancellationToken::new()).is_ok());
}

#[test]
fn open_errors_pass_through_typed() {
    let fake = FakeSerial::default();
    fake.plug(usb(USB0));
    fake.deny(USB0, "dialout");
    let ports: Arc<dyn SerialPorts> = Arc::new(fake);
    let err = |port: &str| {
        open(ports.clone(), &SerialOptions { port: port.into(), ..SerialOptions::default() }, CancellationToken::new())
            .err()
            .unwrap()
    };
    assert!(matches!(err(""), Error::Invalid(_)));
    assert!(matches!(err("/dev/ttyUSB9"), Error::NotFound(_)));
    assert!(matches!(err(USB0), Error::PermissionDenied(m) if m.contains("dialout")));
}

#[tokio::test]
async fn unplug_reports_gone_then_replug_by_id_on_new_path_reconnects() {
    let (fake, mut s) = setup(|_| {});
    let last = s.port_info().cloned();
    assert_eq!(last.as_ref().map(|p| p.path.as_str()), Some(USB0));
    fake.unplug(USB0);
    assert!(matches!(s.read().await, Some(Err(Error::Gone(p))) if p == USB0));
    assert!(s.read().await.is_none());

    let ports: Arc<dyn SerialPorts> = Arc::new(fake.clone());
    let opts = SerialOptions { port: USB0.into(), ..SerialOptions::default() };
    let replug = fake.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        // A different adapter takes the old path; the session must follow its own device.
        replug.plug(PortInfo { path: USB0.into(), ..PortInfo::default() });
        replug.plug(usb("/dev/ttyUSB1"));
    });
    let start = Instant::now();
    let mut s = reopen(ports, &opts, last.as_ref(), CancellationToken::new()).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(s.port_info().map(|p| p.path.as_str()), Some("/dev/ttyUSB1"));
    fake.send("/dev/ttyUSB1", b"back");
    assert_eq!(s.read().await.unwrap().unwrap(), b"back");
}

#[tokio::test]
async fn reopen_stops_on_cancel_and_on_permission_error() {
    let fake = FakeSerial::default();
    let ports: Arc<dyn SerialPorts> = Arc::new(fake.clone());
    let opts = SerialOptions { port: USB0.into(), ..SerialOptions::default() };
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(reopen(ports.clone(), &opts, None, cancel).await, Err(Error::Cancelled)));
    fake.plug(usb(USB0));
    fake.deny(USB0, "uucp");
    assert!(matches!(reopen(ports, &opts, None, CancellationToken::new()).await, Err(Error::PermissionDenied(_))));
}

#[test]
fn find_port_prefers_identity_then_path() {
    let other = PortInfo { path: "/dev/ttyUSB0".into(), ..PortInfo::default() };
    let moved = usb("/dev/ttyUSB1");
    let list = [other, moved.clone()];
    assert_eq!(find_port(&list, USB0, Some(&usb(USB0))).as_deref(), Some("/dev/ttyUSB1"));
    let no_by_id = PortInfo { by_id: None, ..usb(USB0) };
    assert_eq!(find_port(&list, USB0, Some(&no_by_id)).as_deref(), Some("/dev/ttyUSB1"));
    assert_eq!(find_port(&list, USB0, None).as_deref(), Some(USB0));
    assert_eq!(find_port(&list, BY_ID, None).as_deref(), Some("/dev/ttyUSB1"));
    assert_eq!(find_port(&[], USB0, None), None);
}

#[test]
fn translations() {
    assert_eq!(translate(b"a\rb", b"\r\n"), b"a\r\nb");
    assert_eq!(lf_to_crlf(b"\n\r\n"), b"\r\n\r\r\n");
}
