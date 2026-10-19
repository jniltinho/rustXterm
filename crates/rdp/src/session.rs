//! Connection sequence and active-stage driver.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use ironrdp::cliprdr::backend::CliprdrBackend;
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest, FileContentsResponse,
    FormatDataRequest, FormatDataResponse, LockDataId, OwnedFormatDataResponse,
};
use ironrdp::cliprdr::{CliprdrClient, CliprdrSvcMessages};
use ironrdp::connector::connection_activation::{ConnectionActivationFactory, ConnectionActivationState};
use ironrdp::connector::credssp::CredsspSequence;
use ironrdp::connector::{
    BitmapConfig, ClientConnector, ClientConnectorState, Config, ConnectorError, ConnectorErrorKind, Credentials,
    DesktopSize, Sequence, ServerName, Written,
};
use ironrdp::core::{ReadCursor, WriteBuf, decode_cursor, impl_as_any};
use ironrdp::displaycontrol::client::DisplayControlClient;
use ironrdp::dvc::DrdynvcClient;
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::graphics::pointer::DecodedPointer;
use ironrdp::input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp::pdu::Action;
use ironrdp::pdu::fast_path::{FastPathHeader, FastPathUpdatePdu, Fragmentation};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::pdu::input::fast_path::FastPathInputEvent;
use ironrdp::pdu::nego::FailureCode;
use ironrdp::pdu::rdp::capability_sets::{MajorPlatformType, client_codecs_capabilities};
use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp::pdu::rdp::headers::ShareDataPdu;
use ironrdp::pdu::rdp::refresh_rectangle::RefreshRectanglePdu;
use ironrdp::pdu::rdp::suppress_output::SuppressOutputPdu;
use ironrdp::session::fast_path::ProcessorBuilder;
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput, SessionError, SessionErrorKind};
use rxt_core::desktop::{MAX_CURSOR_EDGE, MAX_EDGE, RemoteCursor};
use rxt_core::{DesktopEvent, DesktopInput, Error, RdpResolution, Rect, Result, Session};
use tokio::net::TcpStream;
use tokio::sync::{Semaphore, mpsc};
use tokio::time::timeout;

use crate::tls::{self, Tls};
use crate::trust::{decide_then_password, public_key, trust_then_password};
use crate::wire::{Limits, Wire};
use crate::{Context, Queued, keymap, legacy};

fn proto(e: impl std::fmt::Display) -> Error {
    Error::Protocol(e.to_string())
}

/// The server refused TLS/NLA because its Security Layer is "RDP" (`SSL_NOT_ALLOWED_BY_SERVER`).
pub(crate) const STANDARD_ONLY: &str = "the server only allows the legacy Standard RDP Security layer (no TLS/NLA). \
     Enable 'Allow legacy RDP security' in the session's RDP settings to connect (insecure), or set the \
     server's Remote Desktop \"Security Layer\" to Negotiate or SSL (GPO: Require use of specific security \
     layer for remote (RDP) connections)";

/// The refusal arrived for a host already trusted over TLS (it is unauthenticated and could be forged).
/// Undecodable PDUs tolerated in a row before giving up (old servers send odd PDUs at logout).
const MAX_DECODE_ERRORS: u32 = 32;

const DOWNGRADE: &str = "the server refused TLS, but this host was trusted over TLS before: the refusal may \
     be a downgrade attempt, so legacy RDP security was not used. If the server's administrator really switched \
     it to the legacy layer, connect with another name for it (such as its IP address): trust is saved per \
     host name and port";

fn connector_err(e: ConnectorError) -> Error {
    if let ConnectorErrorKind::Negotiation(f) = e.kind()
        && f.code() == FailureCode::SSL_NOT_ALLOWED_BY_SERVER
    {
        return Error::Protocol(STANDARD_ONLY.into());
    }
    Error::Protocol(e.to_string())
}

fn session_err(e: SessionError) -> Error {
    Error::Protocol(e.to_string())
}

/// Rejects desktop sizes over [`MAX_EDGE`] (and empty ones).
pub(crate) fn check_size(width: u16, height: u16) -> Result<()> {
    let ok = width > 0 && height > 0 && u32::from(width) <= MAX_EDGE && u32::from(height) <= MAX_EDGE;
    if ok { Ok(()) } else { Err(Error::Protocol(format!("desktop size {width}x{height} not allowed"))) }
}

fn activation_size(state: ConnectionActivationState) -> Option<DesktopSize> {
    match state {
        ConnectionActivationState::ConnectionFinalization { desktop_size, .. }
        | ConnectionActivationState::Finalized { desktop_size, .. } => Some(desktop_size),
        _ => None,
    }
}

/// Desktop size the connector has negotiated so far (after the server's Demand Active).
fn negotiated_size(connector: &ClientConnector) -> Option<DesktopSize> {
    match &connector.state {
        ClientConnectorState::ConnectionFinalization { connection_activation } => {
            activation_size(connection_activation.connection_activation_state())
        }
        ClientConnectorState::Connected { result } => Some(result.desktop_size),
        _ => None,
    }
}

/// Read the sequence's next PDU (if it expects one) and step it; the caller writes `buf`.
async fn step_read<S>(wire: &mut Wire<S>, seq: &mut dyn Sequence, buf: &mut WriteBuf) -> Result<Written>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    buf.clear();
    match seq.next_pdu_hint() {
        Some(hint) => {
            let pdu = wire.read_by_hint(hint).await?;
            seq.step(&pdu, buf).map_err(connector_err)
        }
        None => seq.step_no_input(buf).map_err(connector_err),
    }
}

async fn write_written<S>(wire: &mut Wire<S>, buf: &WriteBuf, written: Written) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    write_written_sealed(wire, buf, written, false).await
}

async fn write_written_sealed<S>(wire: &mut Wire<S>, buf: &WriteBuf, written: Written, has_header: bool) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if written.size().is_some() {
        wire.write_sealed(buf.filled(), has_header).await?;
    }
    Ok(())
}

/// Requested desktop size: the tab size for `FitToTab` (display-control rules), else the fixed size.
fn requested_size(session: &Session, tab: (u32, u32)) -> DesktopSize {
    let (w, h) = match session.rdp.resolution {
        RdpResolution::FitToTab => tab,
        RdpResolution::Fixed { width, height } => (width, height),
    };
    let (w, h) = adjust(w, h);
    DesktopSize { width: u16::try_from(w).unwrap_or(1024), height: u16::try_from(h).unwrap_or(768) }
}

/// Display-control constraints: 200..=8192, even width.
fn adjust(w: u32, h: u32) -> (u32, u32) {
    (w.clamp(200, MAX_EDGE) & !1, h.clamp(200, MAX_EDGE))
}

/// User name and domain. `DOMAIN\user` in the user field fills an empty Domain field, as in `mstsc`.
/// `.\user` is a local account of the server: the domain `.` is sent as is, which Windows resolves to
/// its own account database. An empty domain is not the same: a domain-joined server (an Oracle Cloud
/// or migrated instance, say) passes it to its domain, and a local account then fails with
/// `STATUS_LOGON_FAILURE`.
fn account(session: &Session) -> (String, Option<String>) {
    let user = session.user.clone().unwrap_or_default();
    let domain = session.rdp.domain.clone().filter(|d| !d.is_empty());
    match user.split_once('\\') {
        Some((d, u)) if domain.is_none() && !u.is_empty() => {
            (u.to_owned(), Some(d.to_owned()).filter(|d| !d.is_empty()))
        }
        _ => (user, domain),
    }
}

/// `standard`: request Standard RDP Security (no TLS, no CredSSP).
fn config(session: &Session, tab: (u32, u32), standard: bool) -> Result<Config> {
    let codecs = client_codecs_capabilities(&[]).map_err(proto)?;
    Ok(Config {
        desktop_size: requested_size(session, tab),
        desktop_scale_factor: 0,
        enable_tls: !standard,
        enable_credssp: !standard,
        legacy_server: standard || session.rdp.allow_legacy_tls,
        // The password is filled in only after the certificate is trusted.
        credentials: Credentials::UsernamePassword { username: account(session).0, password: String::new() },
        domain: account(session).1,
        client_build: 0,
        client_name: "rustxterm".into(),
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout: session.rdp.keyboard_layout,
        ime_file_name: String::new(),
        bitmap: Some(BitmapConfig { lossy_compression: true, color_depth: 32, codecs }),
        dig_product_id: String::new(),
        client_dir: String::new(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        platform: MajorPlatformType::UNIX,
        hardware_id: None,
        request_data: None,
        autologon: true,
        enable_audio_playback: false,
        performance_flags: PerformanceFlags::default(),
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        // No bulk compression: fast-path fragment sizes are then exact for the reassembly cap.
        compression_type: None,
        enable_server_pointer: true,
        // The UI draws the server pointer at the local mouse position (no network round trip).
        pointer_software_rendering: false,
        multitransport_flags: None,
    })
}

/// Runs the connection sequence up to the active stage. A refusal of TLS/NLA is retried with
/// Standard RDP Security only when the session opts in and the host was never trusted over TLS.
pub(crate) async fn establish(session: &Session, ctx: &Context, tab: (u32, u32), limits: Limits) -> Result<Driver> {
    match establish_with(session, ctx, tab, limits, false).await {
        Err(Error::Protocol(m)) if m == STANDARD_ONLY && session.rdp.allow_standard_security => {
            if ctx.trust.get(&session.host, session.port).is_some_and(|fp| !fp.starts_with(legacy::KEY_PREFIX)) {
                return Err(Error::HostKey(DOWNGRADE.into()));
            }
            tracing::warn!(host = %session.host, "server allows only Standard RDP Security; reconnecting with it");
            establish_with(session, ctx, tab, limits, true).await
        }
        r => r,
    }
}

async fn establish_with(
    session: &Session,
    ctx: &Context,
    tab: (u32, u32),
    limits: Limits,
    standard: bool,
) -> Result<Driver> {
    let host = session.host.as_str();
    let port = session.port;
    let tcp = timeout(limits.stall, TcpStream::connect((host, port)))
        .await
        .map_err(|_| Error::Connection(format!("{host}:{port}: connect timed out")))?
        .map_err(|e| Error::Connection(format!("{host}:{port}: {e}")))?;
    tcp.set_nodelay(true)?;
    let client_addr: SocketAddr = tcp.local_addr()?;

    let clip_events = Arc::new(Mutex::new(Vec::new()));
    let mut connector = ClientConnector::new(config(session, tab, standard)?, client_addr)
        .with_static_channel(DrdynvcClient::new().with_dynamic_channel(DisplayControlClient::new(|_| Ok(Vec::new()))));
    if session.rdp.clipboard {
        connector.attach_static_channel(CliprdrClient::new(Box::new(ClipBackend(clip_events.clone()))));
    }

    let mut wire = Wire::new(tcp, limits);
    let mut buf = WriteBuf::new();
    while !connector.should_perform_security_upgrade() {
        let written = step_read(&mut wire, &mut connector, &mut buf).await?;
        write_written(&mut wire, &buf, written).await?;
    }

    let user = account(session).0;
    let mut wire = if standard {
        // Standard RDP Security: no TLS; the trust decision on the server's RSA key and the
        // Security Exchange happen right before the Client Info PDU (below).
        connector.mark_security_upgrade_as_done();
        Wire::new(Box::new(wire.into_inner()?) as Tls, limits)
    } else {
        // TLS: the leaf is captured unverified; the trust decision follows before any credential.
        let (stream, leaf) = tls::upgrade(wire.into_inner()?, host, session.rdp.allow_legacy_tls, limits.stall).await?;
        let password = trust_then_password(&*ctx.prompter, &*ctx.trust, host, port, &user, &leaf).await?;
        connector.config.credentials = Credentials::UsernamePassword { username: user.clone(), password };
        connector.mark_security_upgrade_as_done();
        let mut wire = Wire::new(stream, limits);
        if connector.should_perform_credssp() {
            credssp(&mut wire, &mut connector, host, public_key(&leaf)?).await?;
        }
        wire
    };
    loop {
        if standard
            && wire.legacy.is_none()
            && let ClientConnectorState::SecureSettingsExchange { io_channel_id, user_channel_id } = connector.state
        {
            let data = connector
                .server_security
                .clone()
                .ok_or_else(|| Error::Protocol("server sent no security data for legacy RDP security".into()))?;
            let key = legacy::ServerKey::parse(&data.server_cert)?;
            let exchange = legacy::exchange(&data, &key, user_channel_id, io_channel_id)?;
            let password = decide_then_password(
                &*ctx.prompter,
                &*ctx.trust,
                host,
                port,
                &user,
                key.fingerprint(),
                legacy::PROMPT_SUBJECT.into(),
            )
            .await?;
            connector.config.credentials = Credentials::UsernamePassword { username: user.clone(), password };
            wire.write(&exchange.pdu).await?;
            wire.legacy = Some(Box::new(exchange.security));
        }
        // Client Info and licensing PDUs already carry a basic security header.
        let has_header = matches!(
            connector.state,
            ClientConnectorState::SecureSettingsExchange { .. }
                | ClientConnectorState::ConnectTimeAutoDetection { .. }
                | ClientConnectorState::LicensingExchange { .. }
        );
        let written = step_read(&mut wire, &mut connector, &mut buf).await?;
        if let Some(size) = negotiated_size(&connector) {
            check_size(size.width, size.height)?;
        }
        write_written_sealed(&mut wire, &buf, written, has_header).await?;
        if !matches!(connector.state, ClientConnectorState::Connected { .. }) {
            continue;
        }
        if let ClientConnectorState::Connected { result } = std::mem::take(&mut connector.state) {
            let size = result.desktop_size;
            let stage = ActiveStageBuilder {
                static_channels: result.static_channels,
                user_channel_id: result.user_channel_id,
                io_channel_id: result.io_channel_id,
                message_channel_id: result.message_channel_id,
                share_id: result.share_id,
                compression_type: result.compression_type,
                enable_server_pointer: result.enable_server_pointer,
                pointer_software_rendering: result.pointer_software_rendering,
            }
            .build();
            return Ok(Driver {
                wire,
                stage,
                image: DecodedImage::new(PixelFormat::RgbA32, size.width, size.height),
                factory: result.activation_factory,
                limits,
                db: Database::new(),
                visible: true,
                fit: matches!(session.rdp.resolution, RdpResolution::FitToTab),
                pending_resize: None,
                decode_errors: 0,
                fragmented: 0,
                clip_events,
                local_clip: None,
            });
        }
    }
}

/// CredSSP/NLA (NTLM) over the guarded framing.
async fn credssp(wire: &mut Wire<Tls>, connector: &mut ClientConnector, host: &str, server_key: Vec<u8>) -> Result<()> {
    let ClientConnectorState::Credssp { selected_protocol } = connector.state else {
        return Err(Error::Protocol("invalid connector state for CredSSP".into()));
    };
    if let Credentials::UsernamePassword { username, .. } = &connector.config.credentials {
        tracing::debug!(host, user = %username, domain = ?connector.config.domain, ?selected_protocol, "RDP NLA (CredSSP/NTLM)");
    }
    let (mut seq, mut request) = CredsspSequence::init(
        connector.config.credentials.clone(),
        connector.config.domain.as_deref(),
        selected_protocol,
        ServerName::new(host),
        server_key,
        None,
    )
    .map_err(connector_err)?;
    let mut buf = WriteBuf::new();
    loop {
        let state =
            seq.process_ts_request(request).resolve_to_result().map_err(|e| Error::Auth(format!("CredSSP: {e}")))?;
        buf.clear();
        let written = seq.handle_process_result(state, &mut buf).map_err(connector_err)?;
        write_written(wire, &buf, written).await?;
        let Some(hint) = seq.next_pdu_hint() else { break };
        let pdu = wire.read_by_hint(hint).await.map_err(credssp_read_err)?;
        match seq.decode_server_message(&pdu).map_err(connector_err)? {
            Some(next) => request = next,
            None => break,
        }
    }
    connector.mark_credssp_as_done();
    Ok(())
}

/// Servers without CredSSP error codes (e.g. Windows Server 2008) reject bad credentials
/// with a TLS `access_denied` alert (OpenSSL and rustls spellings).
fn credssp_read_err(e: Error) -> Error {
    match e {
        Error::Io(m) if m.contains("access denied") || m.contains("AccessDenied") => {
            Error::Auth("the server rejected the user name, password or domain".into())
        }
        e => e,
    }
}

/// Clipboard backend: records what the channel reports; the driver acts on it after each PDU.
#[derive(Debug)]
struct ClipBackend(Arc<Mutex<Vec<ClipEvent>>>);

#[derive(Debug)]
enum ClipEvent {
    RemoteHasText,
    DataRequest(ClipboardFormatId),
    RemoteText(String),
    FormatListWanted,
}

impl_as_any!(ClipBackend);

impl ClipBackend {
    fn push(&self, e: ClipEvent) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(e);
    }
}

impl CliprdrBackend for ClipBackend {
    fn temporary_directory(&self) -> &str {
        ""
    }
    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::empty()
    }
    fn on_ready(&mut self) {}
    fn on_request_format_list(&mut self) {
        self.push(ClipEvent::FormatListWanted);
    }
    fn on_process_negotiated_capabilities(&mut self, _: ClipboardGeneralCapabilityFlags) {}
    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        if formats.iter().any(|f| f.id() == ClipboardFormatId::CF_UNICODETEXT) {
            self.push(ClipEvent::RemoteHasText);
        }
    }
    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.push(ClipEvent::DataRequest(request.format));
    }
    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if !response.is_error()
            && let Ok(text) = response.to_unicode_string()
        {
            self.push(ClipEvent::RemoteText(text));
        }
    }
    fn on_file_contents_request(&mut self, _: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _: LockDataId) {}
    fn on_unlock(&mut self, _: LockDataId) {}
}

/// Active stage: decodes PDUs into RGBA events and encodes input.
pub(crate) struct Driver {
    wire: Wire<Tls>,
    stage: ActiveStage,
    image: DecodedImage,
    factory: ConnectionActivationFactory,
    limits: Limits,
    db: Database,
    visible: bool,
    fit: bool,
    /// Requested size not sent yet (display control not ready).
    pending_resize: Option<(u32, u32)>,
    /// Consecutive undecodable PDUs tolerated before the session is dropped.
    decode_errors: u32,
    /// Bytes of the fast-path update being reassembled.
    fragmented: usize,
    clip_events: Arc<Mutex<Vec<ClipEvent>>>,
    local_clip: Option<String>,
}

/// `rect` (inclusive) clipped to the image, as row bands of at most `max_bytes` RGBA each.
fn bands(rect: &InclusiveRectangle, width: u16, height: u16, max_bytes: usize) -> Vec<Rect> {
    let right = rect.right.min(width.saturating_sub(1));
    let bottom = rect.bottom.min(height.saturating_sub(1));
    if width == 0 || height == 0 || rect.left > right || rect.top > bottom {
        return Vec::new();
    }
    let w = u32::from(right - rect.left) + 1;
    let per_band = u32::try_from(max_bytes / (w as usize * 4)).unwrap_or(u32::MAX).max(1);
    let (top, bottom) = (u32::from(rect.top), u32::from(bottom));
    (top..=bottom)
        .step_by(per_band as usize)
        .map(|y| Rect { x: rect.left.into(), y, width: w, height: per_band.min(bottom - y + 1) })
        .collect()
}

/// Fast-path reassembly cap (ironrdp-session does not bound it): `open` is the size of the
/// update being reassembled.
fn track_fragments(frame: &[u8], open: &mut usize, cap: usize) -> Result<()> {
    let mut cur = ReadCursor::new(frame);
    decode_cursor::<FastPathHeader>(&mut cur).map_err(proto)?;
    while !cur.is_empty() {
        let update = decode_cursor::<FastPathUpdatePdu<'_>>(&mut cur).map_err(proto)?;
        let len = update.data.len();
        *open = match update.fragmentation {
            Fragmentation::Single => len,
            Fragmentation::First => len,
            Fragmentation::Next | Fragmentation::Last => open.saturating_add(len),
        };
        if *open > cap {
            return Err(Error::Protocol(format!("fragmented update exceeds {cap} bytes")));
        }
        if matches!(update.fragmentation, Fragmentation::Single | Fragmentation::Last) {
            *open = 0;
        }
    }
    Ok(())
}

/// Server pointer bitmap (straight-alpha RGBA, accelerated target) → cursor event shape;
/// empty shapes hide the pointer, oversized ones fall back to the local arrow.
pub(crate) fn cursor(p: &DecodedPointer) -> RemoteCursor {
    let (width, height) = (u32::from(p.width), u32::from(p.height));
    if width == 0 || height == 0 {
        return RemoteCursor::Hidden;
    }
    if width > MAX_CURSOR_EDGE
        || height > MAX_CURSOR_EDGE
        || p.bitmap_data.len() as u64 != u64::from(width * height) * 4
    {
        return RemoteCursor::Default;
    }
    RemoteCursor::Bitmap {
        width,
        height,
        hot_x: u32::from(p.hotspot_x).min(width - 1),
        hot_y: u32::from(p.hotspot_y).min(height - 1),
        rgba: p.bitmap_data.clone(),
    }
}

/// Byte-bounded event queue: `push` waits (so the socket is not read) while it is full.
pub(crate) struct Queue {
    sem: Arc<Semaphore>,
    tx: mpsc::UnboundedSender<Queued>,
    cap: usize,
}

impl Queue {
    pub(crate) fn new(tx: mpsc::UnboundedSender<Queued>, cap: usize) -> Self {
        Self { sem: Arc::new(Semaphore::new(cap)), tx, cap }
    }

    async fn push(&self, event: DesktopEvent) -> Result<()> {
        let cost = match &event {
            DesktopEvent::Update { rgba, .. } => rgba.len(),
            DesktopEvent::Clipboard(t) => t.len(),
            DesktopEvent::Cursor(RemoteCursor::Bitmap { rgba, .. }) => rgba.len(),
            _ => 0,
        }
        .clamp(1, self.cap);
        let permit = self
            .sem
            .clone()
            .acquire_many_owned(u32::try_from(cost).unwrap_or(u32::MAX))
            .await
            .map_err(|_| Error::Connection("event queue closed".into()))?;
        self.tx.send((Ok(event), Some(permit))).map_err(|_| Error::Connection("desktop dropped".into()))
    }
}

type Events = Queue;

impl Driver {
    pub(crate) async fn run(mut self, events: Events, mut input: mpsc::Receiver<DesktopInput>) -> Result<()> {
        let events = &events;
        self.emit_resized(events).await?;
        loop {
            tokio::select! {
                i = input.recv() => match i {
                    Some(i) => self.on_input(i, events).await?,
                    None => return Ok(()),
                },
                pdu = self.wire.read_pdu() => {
                    let (action, frame) = pdu?;
                    if !self.on_pdu(action, &frame, events).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn emit_resized(&mut self, events: &Events) -> Result<()> {
        let (width, height) = (self.image.width(), self.image.height());
        events.push(DesktopEvent::Resized { width: width.into(), height: height.into() }).await
    }

    /// Emit `rect` (inclusive, clipped to the image) as RGBA row bands within `max_update_bytes`.
    /// Emit `rect` as RGBA row bands within `max_update_bytes` (nothing while hidden).
    async fn emit_region(&mut self, events: &Events, rect: &InclusiveRectangle) -> Result<()> {
        if !self.visible {
            return Ok(());
        }
        let iw = usize::from(self.image.width());
        for band in bands(rect, self.image.width(), self.image.height(), self.limits.max_update_bytes) {
            let (x, w) = (band.x as usize, band.width as usize);
            let data = self.image.data();
            let mut rgba = Vec::with_capacity(w * band.height as usize * 4);
            for r in band.y as usize..(band.y + band.height) as usize {
                let at = (r * iw + x) * 4;
                rgba.extend_from_slice(&data[at..at + w * 4]);
            }
            rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 0xff);
            events.push(DesktopEvent::Update { rect: band, rgba }).await?;
        }
        Ok(())
    }

    fn full_rect(&self) -> InclusiveRectangle {
        InclusiveRectangle {
            left: 0,
            top: 0,
            right: self.image.width().saturating_sub(1),
            bottom: self.image.height().saturating_sub(1),
        }
    }

    /// Returns `false` when the server ended the session.
    async fn on_pdu(&mut self, action: Action, frame: &[u8], events: &Events) -> Result<bool> {
        if action == Action::FastPath {
            track_fragments(frame, &mut self.fragmented, self.limits.max_fragmented)?;
        }
        let outputs = match self.stage.process(&mut self.image, action, frame) {
            Ok(outputs) => {
                self.decode_errors = 0;
                outputs
            }
            // A PDU whose content IronRDP cannot decode (e.g. an update some old servers send during
            // logout) must not kill the session: the framing is intact, so skip this one PDU and keep
            // going. A run of them means the stream really is lost, so give up then.
            Err(e) if matches!(e.kind(), SessionErrorKind::Decode(_)) => {
                self.decode_errors += 1;
                tracing::warn!(action = ?action, count = self.decode_errors, "skipping an undecodable PDU: {e}");
                if self.decode_errors > MAX_DECODE_ERRORS {
                    return Err(session_err(e));
                }
                return Ok(true);
            }
            Err(e) => return Err(session_err(e)),
        };
        let more = self.on_outputs(outputs, events).await?;
        self.drain_clipboard(events).await?;
        self.send_pending_resize().await?;
        Ok(more)
    }

    async fn on_outputs(&mut self, outputs: Vec<ActiveStageOutput>, events: &Events) -> Result<bool> {
        for out in outputs {
            match out {
                ActiveStageOutput::ResponseFrame(f) if !f.is_empty() => self.wire.write(&f).await?,
                ActiveStageOutput::GraphicsUpdate(rect) => self.emit_region(events, &rect).await?,
                ActiveStageOutput::Terminate(reason) => {
                    tracing::debug!(reason = %reason.description(), "RDP server ended the session");
                    return Ok(false);
                }
                // A Server Deactivate All precedes both a capability re-exchange (resize) and the
                // server's own disconnect (e.g. logout on Windows Server 2008). `reactivate` returns
                // `false` when the server tears down instead of reactivating; end the session cleanly
                // then rather than surfacing its decode/connection error.
                ActiveStageOutput::DeactivateAll => {
                    if !self.reactivate(events).await? {
                        tracing::debug!("server deactivated without reactivating; ending the session");
                        return Ok(false);
                    }
                }
                ActiveStageOutput::PointerDefault => events.push(DesktopEvent::Cursor(RemoteCursor::Default)).await?,
                ActiveStageOutput::PointerHidden => events.push(DesktopEvent::Cursor(RemoteCursor::Hidden)).await?,
                ActiveStageOutput::PointerBitmap(p) => events.push(DesktopEvent::Cursor(cursor(&p))).await?,
                _ => {}
            }
        }
        Ok(true)
    }

    /// Deactivation-Reactivation Sequence (e.g. after a display-control resize). Returns `false`
    /// when the server disconnected instead of reactivating (a teardown PDU the activation sequence
    /// cannot decode, or a closed socket) — the common Windows Server 2008 logout path.
    async fn reactivate(&mut self, events: &Events) -> Result<bool> {
        let mut seq = self.factory.create();
        let mut buf = WriteBuf::new();
        loop {
            let written = match step_read(&mut self.wire, &mut seq, &mut buf).await {
                Ok(written) => written,
                Err(Error::Protocol(_) | Error::Connection(_) | Error::Io(_)) => return Ok(false),
                Err(e) => return Err(e),
            };
            if let Some(size) = activation_size(seq.connection_activation_state()) {
                check_size(size.width, size.height)?;
            }
            write_written(&mut self.wire, &buf, written).await?;
            if let ConnectionActivationState::Finalized {
                desktop_size,
                share_id,
                enable_server_pointer,
                pointer_software_rendering,
            } = seq.connection_activation_state()
            {
                self.image = DecodedImage::new(PixelFormat::RgbA32, desktop_size.width, desktop_size.height);
                self.stage.set_fastpath_processor(
                    ProcessorBuilder {
                        io_channel_id: seq.io_channel_id(),
                        user_channel_id: seq.user_channel_id(),
                        share_id,
                        enable_server_pointer,
                        pointer_software_rendering,
                        bulk_decompressor: None,
                    }
                    .build(),
                );
                self.stage.set_share_id(share_id);
                self.stage.set_enable_server_pointer(enable_server_pointer);
                self.fragmented = 0;
                self.emit_resized(events).await?;
                return Ok(true);
            }
        }
    }

    async fn send_fastpath(&mut self, input: &[FastPathInputEvent], events: &Events) -> Result<()> {
        let outputs = self.stage.process_fastpath_input(&mut self.image, input).map_err(session_err)?;
        self.on_outputs(outputs, events).await.map(drop)
    }

    async fn send_share_data(&mut self, pdu: ShareDataPdu) -> Result<()> {
        let mut buf = WriteBuf::new();
        self.stage.encode_static(&mut buf, pdu).map_err(session_err)?;
        self.wire.write(buf.filled()).await
    }

    async fn send_clip(&mut self, messages: CliprdrSvcMessages<ironrdp::cliprdr::Client>) -> Result<()> {
        let frame = self.stage.process_svc_processor_messages(messages).map_err(session_err)?;
        if frame.is_empty() { Ok(()) } else { self.wire.write(&frame).await }
    }

    fn cliprdr(&mut self) -> Option<&mut CliprdrClient> {
        self.stage.get_svc_processor_mut::<CliprdrClient>()
    }

    async fn advertise_local_clip(&mut self) -> Result<()> {
        if self.local_clip.is_none() {
            return Ok(());
        }
        let Some(c) = self.cliprdr() else { return Ok(()) };
        let m = c.initiate_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]).map_err(proto)?;
        self.send_clip(m).await
    }

    async fn drain_clipboard(&mut self, events: &Events) -> Result<()> {
        let pending = std::mem::take(&mut *self.clip_events.lock().unwrap_or_else(PoisonError::into_inner));
        for e in pending {
            match e {
                ClipEvent::RemoteHasText => {
                    let Some(c) = self.cliprdr() else { continue };
                    let m = c.initiate_paste(ClipboardFormatId::CF_UNICODETEXT).map_err(proto)?;
                    self.send_clip(m).await?;
                }
                ClipEvent::DataRequest(format) => {
                    let response = match (&self.local_clip, format == ClipboardFormatId::CF_UNICODETEXT) {
                        (Some(text), true) => OwnedFormatDataResponse::new_unicode_string(text),
                        _ => OwnedFormatDataResponse::new_error(),
                    };
                    let Some(c) = self.cliprdr() else { continue };
                    let m = c.submit_format_data(response).map_err(proto)?;
                    self.send_clip(m).await?;
                }
                ClipEvent::RemoteText(text) => events.push(DesktopEvent::Clipboard(text)).await?,
                ClipEvent::FormatListWanted => self.advertise_local_clip().await?,
            }
        }
        Ok(())
    }

    async fn on_input(&mut self, input: DesktopInput, events: &Events) -> Result<()> {
        match input {
            DesktopInput::Key { keysym, down } => {
                let op = match (keymap::scancode(keysym), keymap::unicode(keysym)) {
                    (Some(sc), _) if down => Operation::KeyPressed(Scancode::from_u16(sc)),
                    (Some(sc), _) => Operation::KeyReleased(Scancode::from_u16(sc)),
                    (None, Some(ch)) if down => Operation::UnicodeKeyPressed(ch),
                    (None, Some(ch)) => Operation::UnicodeKeyReleased(ch),
                    (None, None) => return Ok(()),
                };
                let fp = self.db.apply([op]);
                self.send_fastpath(&fp, events).await
            }
            DesktopInput::Pointer { x, y, buttons } => {
                let mut ops = vec![Operation::MouseMove(self.position(x, y))];
                for (bit, button) in [(0, MouseButton::Left), (1, MouseButton::Middle), (2, MouseButton::Right)] {
                    let down = buttons & (1 << bit) != 0;
                    if down != self.db.is_mouse_button_pressed(button) {
                        ops.push(if down {
                            Operation::MouseButtonPressed(button)
                        } else {
                            Operation::MouseButtonReleased(button)
                        });
                    }
                }
                let fp = self.db.apply(ops);
                self.send_fastpath(&fp, events).await
            }
            DesktopInput::Wheel { x, y, dx, dy } => {
                let mut ops = vec![Operation::MouseMove(self.position(x, y))];
                for (is_vertical, steps) in [(true, dy), (false, dx)] {
                    if steps != 0 {
                        let rotation_units = steps.saturating_mul(120);
                        ops.push(Operation::WheelRotations(WheelRotations { is_vertical, rotation_units }));
                    }
                }
                let fp = self.db.apply(ops);
                self.send_fastpath(&fp, events).await
            }
            DesktopInput::ReleaseAll => {
                let fp = self.db.release_all();
                self.send_fastpath(&fp, events).await
            }
            DesktopInput::Clipboard(text) => {
                if self.cliprdr().is_some() {
                    self.local_clip = Some(text);
                    self.advertise_local_clip().await?;
                }
                Ok(())
            }
            DesktopInput::Visible(visible) if visible != self.visible => {
                self.visible = visible;
                let full = self.full_rect();
                let rect = visible.then(|| full.clone());
                self.send_share_data(ShareDataPdu::SuppressOutput(SuppressOutputPdu { desktop_rect: rect })).await?;
                if visible {
                    self.send_share_data(ShareDataPdu::RefreshRectangle(RefreshRectanglePdu {
                        areas_to_refresh: vec![full.clone()],
                    }))
                    .await?;
                    // Updates are not emitted while hidden: redraw from the decoded image now.
                    self.emit_region(events, &full).await?;
                }
                Ok(())
            }
            DesktopInput::Visible(_) => Ok(()),
            DesktopInput::Resize { width, height } => {
                if self.fit {
                    self.pending_resize = Some(adjust(width, height));
                    self.send_pending_resize().await?;
                }
                Ok(())
            }
        }
    }

    /// Send the latest requested size once the display-control channel is ready.
    async fn send_pending_resize(&mut self) -> Result<()> {
        let Some((w, h)) = self.pending_resize else { return Ok(()) };
        let ready = self
            .stage
            .get_dvc::<DisplayControlClient>()
            .and_then(|dvc| dvc.channel_processor_downcast_ref::<DisplayControlClient>())
            .is_some_and(DisplayControlClient::ready);
        if !ready {
            return Ok(());
        }
        self.pending_resize = None;
        match self.stage.encode_resize(w, h, None, None) {
            Some(frame) => self.wire.write(&frame.map_err(session_err)?).await,
            None => Ok(()),
        }
    }

    fn position(&self, x: u32, y: u32) -> MousePosition {
        let clamp = |v: u32, max: u16| u16::try_from(v).unwrap_or(u16::MAX).min(max.saturating_sub(1));
        MousePosition { x: clamp(x, self.image.width()), y: clamp(y, self.image.height()) }
    }
}

#[cfg(test)]
mod account_tests {
    use rxt_core::{Protocol, Session};

    use super::account;

    #[test]
    fn domain_backslash_user_fills_an_empty_domain() {
        let s = |user: &str, domain: Option<&str>| {
            let mut s = Session::new(Protocol::Rdp, "h", Some(user.into()), None);
            s.rdp.domain = domain.map(Into::into);
            account(&s)
        };
        assert_eq!(s("TRD-APP01\\administrator", None), ("administrator".into(), Some("TRD-APP01".into())));
        // `.` reaches the server: an empty domain would send a local account to the server's domain.
        assert_eq!(s(".\\administrator", None), ("administrator".into(), Some(".".into())));
        assert_eq!(s("\\administrator", None), ("administrator".into(), None));
        assert_eq!(s("administrator", Some("LAB")), ("administrator".into(), Some("LAB".into())));
        // An explicit Domain field wins; the user name is then sent as typed.
        assert_eq!(s("A\\bob", Some("LAB")), ("A\\bob".into(), Some("LAB".into())));
        assert_eq!(s("bob@lab.example", None), ("bob@lab.example".into(), None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credssp_access_denied_alert_is_an_auth_error() {
        let ssl = "error:0A000419:SSL routines:ssl3_read_bytes:tlsv1 alert access denied:ssl/record/x.c:918";
        assert!(matches!(credssp_read_err(Error::Io(ssl.into())), Error::Auth(_)));
        assert!(matches!(credssp_read_err(Error::Io("received fatal alert: AccessDenied".into())), Error::Auth(_)));
        assert_eq!(credssp_read_err(Error::Io("reset".into())), Error::Io("reset".into()));
    }

    #[test]
    fn desktop_size_cap() {
        assert!(check_size(8192, 8192).is_ok());
        assert!(check_size(8193, 600).is_err());
        assert!(check_size(800, 9000).is_err());
        assert!(check_size(0, 600).is_err());
    }

    #[test]
    fn update_bands_respect_the_cap_and_clip() {
        let r = InclusiveRectangle { left: 10, top: 0, right: 109, bottom: 99 };
        // 100 px wide rows of 400 bytes; 4000-byte cap → 10 rows per band.
        let b = bands(&r, 200, 200, 4000);
        assert_eq!(b.len(), 10);
        assert!(b.iter().all(|r| r.area() * 4 <= 4000 && r.x == 10 && r.width == 100));
        assert_eq!(b.iter().map(|r| r.height).sum::<u32>(), 100);
        // Clipped to the image; a row wider than the cap still yields one-row bands.
        let b = bands(&r, 50, 20, 16);
        assert_eq!((b.len(), b[0].width, b[0].height), (20, 40, 1));
        assert!(bands(&InclusiveRectangle { left: 60, top: 0, right: 70, bottom: 5 }, 50, 20, 4000).is_empty());
    }

    /// Fast-path PDU with one update of `len` bytes and the given fragmentation bits.
    fn fastpath(frag: u8, len: usize) -> Vec<u8> {
        let mut body = vec![0x01 | (frag << 4)];
        body.extend_from_slice(&u16::try_from(len).unwrap().to_le_bytes());
        body.resize(body.len() + len, 0);
        let total = body.len() + 3;
        let mut pdu = vec![0x00, 0x80 | (total >> 8) as u8, total as u8];
        pdu.extend_from_slice(&body);
        pdu
    }

    #[test]
    fn fragment_reassembly_cap() {
        let mut open = 0;
        track_fragments(&fastpath(0, 900), &mut open, 1000).unwrap();
        assert_eq!(open, 0);
        track_fragments(&fastpath(2, 600), &mut open, 1000).unwrap(); // First
        assert_eq!(open, 600);
        assert!(track_fragments(&fastpath(3, 600), &mut open, 1000).is_err()); // Next over cap
        open = 0;
        track_fragments(&fastpath(2, 400), &mut open, 1000).unwrap();
        track_fragments(&fastpath(1, 400), &mut open, 1000).unwrap(); // Last
        assert_eq!(open, 0);
    }

    #[tokio::test]
    async fn queue_blocks_when_full_until_consumed() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let q = Queue::new(tx, 100);
        let update = |n| DesktopEvent::Update { rect: Rect::default(), rgba: vec![0; n] };
        q.push(update(60)).await.unwrap();
        let blocked = tokio::time::timeout(std::time::Duration::from_millis(100), q.push(update(60))).await;
        assert!(blocked.is_err(), "second push must wait while 60 of 100 bytes are queued");
        drop(rx.recv().await.unwrap()); // consumer takes the first event (permit released)
        q.push(update(60)).await.unwrap();
    }

    #[test]
    fn pointer_bitmap_maps_to_cursor() {
        let p = |width, height, len| DecodedPointer {
            width,
            height,
            hotspot_x: 5,
            hotspot_y: 1,
            bitmap_data: (0..len).map(|i| i as u8).collect(),
        };
        let RemoteCursor::Bitmap { width, height, hot_x, hot_y, rgba } = cursor(&p(4, 2, 32)) else {
            panic!("bitmap expected");
        };
        assert_eq!((width, height, hot_x, hot_y), (4, 2, 3, 1), "hotspot clamped inside");
        assert_eq!(rgba, p(4, 2, 32).bitmap_data);
        assert_eq!(cursor(&p(0, 0, 0)), RemoteCursor::Hidden);
        assert_eq!(cursor(&p(300, 1, 1200)), RemoteCursor::Default);
        assert_eq!(cursor(&p(4, 2, 31)), RemoteCursor::Default);
        let mut fb = rxt_core::Framebuffer::default();
        fb.apply(&DesktopEvent::Cursor(cursor(&p(4, 2, 32)))).unwrap();
    }

    #[test]
    fn display_control_adjustments() {
        assert_eq!(adjust(1001, 50), (1000, 200));
        assert_eq!(adjust(20000, 20000), (8192, 8192));
        let mut s = Session::default();
        s.rdp.resolution = RdpResolution::Fixed { width: 1280, height: 720 };
        assert_eq!(requested_size(&s, (300, 300)), DesktopSize { width: 1280, height: 720 });
        s.rdp.resolution = RdpResolution::FitToTab;
        assert_eq!(requested_size(&s, (1366, 700)), DesktopSize { width: 1366, height: 700 });
    }
}
