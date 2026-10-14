//! Control-channel client: one request per connection over the platform transport
//! (`rxt_platform::control` framing). Never starts the GUI.

use rxt_core::control::{AgentOp, EntryPoint, ErrorCode, Request, Response};
use rxt_platform::ControlTransport;
use rxt_platform::control::{read_message, write_message};
use serde_json::Value;

use crate::ControlError;

/// Send `op` to the running instance and return its result.
pub async fn call(
    control: &dyn ControlTransport,
    client: &str,
    entry: EntryPoint,
    op: AgentOp,
) -> Result<Value, ControlError> {
    let mut stream = control.connect().await.map_err(|e| match e {
        // No socket, nothing listening, or no transport on this platform.
        rxt_core::Error::NotFound(_) | rxt_core::Error::Unsupported(_) => {
            ControlError::new(ErrorCode::NotRunning, "rustXterm is not running, or agent control is off")
        }
        rxt_core::Error::PermissionDenied(m) => ControlError::new(ErrorCode::Denied, m),
        other => ControlError::new(ErrorCode::Failed, format!("control channel: {other}")),
    })?;
    let request = Request { id: 1, client: client.to_owned(), entry, op };
    let fail = |e: rxt_core::Error| ControlError::new(ErrorCode::Failed, format!("control channel: {e}"));
    // A refused client gets its `denied` reason even when the server closed before our write
    // finished, so read the answer before reporting a write error.
    let written = write_message(&mut stream, &request).await;
    match read_message(&mut stream).await {
        Ok(Some(Response::Ok { result, .. })) => Ok(result),
        Ok(Some(Response::Err { code, message, .. })) => Err(ControlError::new(code, message)),
        Ok(None) => Err(written
            .err()
            .map_or_else(|| ControlError::new(ErrorCode::Failed, "control channel closed without a response"), fail)),
        Err(e) => Err(fail(written.err().unwrap_or(e))),
    }
}

/// Blocking [`call`] for the CLI.
pub fn call_blocking(
    control: &dyn ControlTransport,
    client: &str,
    entry: EntryPoint,
    op: AgentOp,
) -> Result<Value, ControlError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ControlError::new(ErrorCode::Failed, e.to_string()))?
        .block_on(call(control, client, entry, op))
}

#[cfg(test)]
#[cfg(target_os = "linux")]
pub(crate) mod fake {
    //! Fake instance: a real Unix control socket answering each connection with the next reply.
    use std::path::Path;
    use std::sync::Arc;

    use rxt_platform::ControlTransport;
    use rxt_platform_linux::UnixControl;
    use tokio::sync::mpsc;

    use super::*;

    /// Listen in `dir` (call inside a runtime); received requests go to the returned channel.
    pub fn serve(dir: &Path, replies: Vec<Response>) -> (Arc<dyn ControlTransport>, mpsc::UnboundedReceiver<Request>) {
        let transport: Arc<dyn ControlTransport> = Arc::new(UnixControl::new(Some(dir.to_owned())));
        let listener = transport.listen().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            for reply in replies {
                let mut s = listener.accept().await.unwrap();
                let req: Request = read_message(&mut s).await.unwrap().unwrap();
                tx.send(req).unwrap();
                write_message(&mut s, &reply).await.unwrap();
            }
            // Keep the socket until the test runtime ends.
            std::future::pending::<()>().await;
        });
        (transport, rx)
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use rxt_core::TabId;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    #[test]
    fn no_instance_is_not_running() {
        let none = rxt_platform_linux::UnixControl::new(Some("/nonexistent/rxt".into()));
        let err = call_blocking(&none, "t", EntryPoint::CliLive, AgentOp::ListTabs).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotRunning);
        let fake = rxt_platform::fake::Fake::default();
        assert_eq!(
            call_blocking(&fake, "t", EntryPoint::CliLive, AgentOp::ListTabs).unwrap_err().code,
            ErrorCode::NotRunning
        );
    }

    /// A server that refuses at once (answers, then closes without reading) still reaches the
    /// CLI as `denied` with its reason.
    #[test]
    fn refusal_reason_reaches_the_client() {
        let dir = std::env::temp_dir().join(format!("rxt-agent-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        rt().block_on(async {
            let control = rxt_platform_linux::UnixControl::new(Some(dir.clone()));
            let listener = control.listen().unwrap();
            tokio::spawn(async move {
                let mut s = listener.accept().await.unwrap();
                let reason = Response::Err { id: 0, code: ErrorCode::Denied, message: "different elevation".into() };
                write_message(&mut s, &reason).await.unwrap();
                drop(s);
                std::future::pending::<()>().await;
            });
            let big = AgentOp::Send { tab: TabId(1), data: "x".repeat(1 << 20) };
            let err = call(&control, "c", EntryPoint::CliLive, big).await.unwrap_err();
            assert_eq!(err, ControlError::new(ErrorCode::Denied, "different elevation"));
            assert_eq!(crate::exit_code(err.code), 4);
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn round_trip_and_error_mapping() {
        let dir = std::env::temp_dir().join(format!("rxt-agent-client-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        rt().block_on(async {
            let (control, mut rx) = fake::serve(
                &dir,
                vec![
                    Response::Ok { id: 1, result: serde_json::json!({"sent": 3}) },
                    Response::Err { id: 1, code: ErrorCode::Denied, message: "denied by policy".into() },
                ],
            );
            let op = AgentOp::Send { tab: TabId(2), data: "ls\n".into() };
            assert_eq!(call(control.as_ref(), "c", EntryPoint::CliLive, op).await.unwrap()["sent"], 3);
            let req = rx.recv().await.unwrap();
            assert_eq!((req.client.as_str(), req.entry), ("c", EntryPoint::CliLive));
            let err = call(control.as_ref(), "c", EntryPoint::CliLive, AgentOp::ListTabs).await.unwrap_err();
            assert_eq!(err, ControlError::new(ErrorCode::Denied, "denied by policy"));
        });
        let _ = std::fs::remove_dir_all(dir);
    }
}
