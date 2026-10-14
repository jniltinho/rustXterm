//! Keyring over the freedesktop Secret Service (D-Bus) via `oo7::dbus` explicitly; the oo7
//! file backend is never used. Items carry `{application: "rustxterm", id}` in the default
//! collection. Each call runs on a short-lived thread with its own current-thread tokio runtime
//! (the trait is sync and may be called from inside another runtime).

use std::future::Future;
use std::time::Duration;

use oo7::dbus::{Collection, Service};
use rxt_platform::{Error, Keyring, Result};

const APPLICATION: &str = "rustxterm";
/// Connecting to a missing or stuck Secret Service must fail fast.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Operations may show an unlock prompt; bounded so a forgotten prompt cannot hang the caller.
const OP_TIMEOUT: Duration = Duration::from_secs(120);

/// Secret Service keyring.
pub struct SecretService;

fn attrs(id: &str) -> [(&'static str, &str); 2] {
    [("application", APPLICATION), ("id", id)]
}

fn err(e: oo7::dbus::Error) -> Error {
    match e {
        oo7::dbus::Error::Dismissed => Error::Cancelled,
        oo7::dbus::Error::ZBus(_) => Error::Unsupported(format!("secret service: {e}")),
        other => Error::Io(format!("secret service: {other}")),
    }
}

/// Run `op` against the default collection (unlocked first) on a dedicated runtime thread.
fn with_collection<T, F, Fut>(op: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(Collection) -> Fut + Send + 'static,
    Fut: Future<Output = std::result::Result<T, oo7::dbus::Error>>,
{
    let run = move || -> Result<T> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Io(format!("keyring runtime: {e}")))?;
        rt.block_on(async move {
            let service = tokio::time::timeout(CONNECT_TIMEOUT, Service::new())
                .await
                .map_err(|_| Error::Unsupported("secret service: connect timed out".into()))?
                .map_err(|e| Error::Unsupported(format!("secret service: {e}")))?;
            let work = async {
                let collection = service.default_collection().await?;
                if collection.is_locked().await? {
                    collection.unlock(None).await?;
                }
                op(collection).await
            };
            tokio::time::timeout(OP_TIMEOUT, work)
                .await
                .map_err(|_| Error::Unsupported("secret service: timed out".into()))?
                .map_err(err)
        })
    };
    std::thread::Builder::new()
        .name("rxt-keyring".into())
        .spawn(run)
        .map_err(|e| Error::Io(format!("keyring thread: {e}")))?
        .join()
        .map_err(|_| Error::Io("keyring thread panicked".into()))?
}

impl Keyring for SecretService {
    fn get(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let id = id.to_owned();
        with_collection(move |c| async move {
            match c.search_items(&attrs(&id)).await?.first() {
                Some(item) => Ok(Some(item.secret().await?.as_bytes().to_vec())),
                None => Ok(None),
            }
        })
    }

    fn set(&self, id: &str, secret: &[u8]) -> Result<()> {
        let id = id.to_owned();
        let secret = oo7::Secret::blob(secret);
        with_collection(move |c| async move {
            c.create_item(&format!("rustXterm database key ({id})"), &attrs(&id), secret, true, None).await?;
            Ok(())
        })
    }

    fn delete(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        with_collection(move |c| async move {
            for item in c.search_items(&attrs(&id)).await? {
                item.delete(None).await?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "keyring-test")]
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    #[test]
    fn no_session_bus_is_unsupported_quickly() {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
            return; // covered by the keyring container test
        }
        let start = Instant::now();
        assert!(matches!(SecretService.get("rustxterm-db-x"), Err(Error::Unsupported(_))));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[cfg(feature = "keyring-test")]
    fn snapshot(root: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(root) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                snapshot(&p, out);
            }
            out.push(p);
        }
    }

    /// Run by `lab/keyring/run.sh` inside a container with a D-Bus session and an unlocked
    /// gnome-keyring whose storage lives outside `$HOME`; any file under `$HOME` afterwards
    /// (e.g. oo7's file backend in `~/.local/share/keyrings`) means we did not speak D-Bus only.
    #[test]
    #[cfg(feature = "keyring-test")]
    fn secret_service_round_trip_writes_no_files() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let mut before = Vec::new();
        snapshot(&home, &mut before);

        let id = "rustxterm-db-labtest";
        assert_eq!(SecretService.get(id).unwrap(), None);
        SecretService.set(id, b"fictional-wrapping-key").unwrap();
        assert_eq!(SecretService.get(id).unwrap().as_deref(), Some(&b"fictional-wrapping-key"[..]));
        SecretService.set(id, b"replaced").unwrap();
        assert_eq!(SecretService.get(id).unwrap().as_deref(), Some(&b"replaced"[..]));
        SecretService.delete(id).unwrap();
        assert_eq!(SecretService.get(id).unwrap(), None);

        let mut after = Vec::new();
        snapshot(&home, &mut after);
        before.sort();
        after.sort();
        assert_eq!(before, after, "the keyring path wrote files under $HOME");
    }
}
