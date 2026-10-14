//! Remote file commands and the transfer queue over the tab's file connection (lazily opened
//! SFTP channel, or the FTP connection of a file-only tab).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rxt_core::fileops::{join, parent};
use rxt_core::{
    Command, Direction, Error, Event, FileOps, Overwrite, PromptAnswer, PromptKind, Result, TabId, TransferId,
};
use tokio_util::sync::CancellationToken;

use crate::Shared;

const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// The tab's SFTP channel, opened on first use over its existing connection.
pub(crate) async fn file_ops(sh: &Shared, tab: TabId) -> Result<Arc<dyn FileOps>> {
    let conn = sh
        .tabs
        .lock()
        .get(&tab)
        .and_then(|t| t.conn.clone())
        .ok_or_else(|| Error::Connection("tab is not connected".into()))?;
    let link = conn.link.clone();
    conn.sftp
        .get_or_try_init(|| async move {
            match link {
                Some(link) => link.file_ops().await,
                None => Err(Error::Connection("tab has no file connection".into())),
            }
        })
        .await
        .cloned()
}

/// List/Mkdir/Rename/Delete/Chmod. Success emits a fresh `Listing` of the affected directory.
pub(crate) async fn file_command(sh: Arc<Shared>, tab: TabId, cmd: Command) {
    let result = async {
        let fs = file_ops(&sh, tab).await?;
        let dir = match &cmd {
            Command::List { path, .. } if path.is_empty() || path == "~" => fs.home().await?,
            Command::List { path, .. } => path.clone(),
            Command::Mkdir { path, .. } => {
                fs.mkdir(path).await?;
                parent(path)
            }
            Command::Rename { from, to, .. } => {
                fs.rename(from, to).await?;
                parent(to)
            }
            Command::Delete { path, .. } => {
                fs.delete(path).await?;
                parent(path)
            }
            Command::Chmod { path, mode, .. } => {
                fs.chmod(path, *mode).await?;
                parent(path)
            }
            _ => return Ok(None),
        };
        let entries = fs.list(&dir).await?;
        Ok::<_, Error>(Some((dir, entries)))
    }
    .await;
    match result {
        Ok(Some((path, entries))) => sh.emit(Event::Listing { tab, path, entries }).await,
        Ok(None) => {}
        Err(e) => sh.emit(Event::Notice { tab: Some(tab), message: e.to_string() }).await,
    }
}

pub(crate) struct Job {
    pub id: TransferId,
    pub tab: TabId,
    pub direction: Direction,
    pub remote: String,
    pub local: PathBuf,
}

/// One queued transfer. `TransferStarted` is an upsert: sent when queued (no total) and again
/// once the target is resolved (final paths, total). `TransferFinished` always ends it.
pub(crate) async fn transfer(sh: Arc<Shared>, job: Job, cancel: CancellationToken) {
    let id = job.id;
    sh.emit(started(&job, None)).await;
    let result = tokio::select! {
        r = run_transfer(&sh, job, &cancel) => r,
        () = cancel.cancelled() => Err(Error::Cancelled),
    };
    sh.transfers.lock().remove(&id);
    sh.emit(Event::TransferFinished { id, result }).await;
}

fn started(job: &Job, total: Option<u64>) -> Event {
    Event::TransferStarted {
        id: job.id,
        tab: job.tab,
        direction: job.direction,
        remote: job.remote.clone(),
        local: job.local.display().to_string(),
        total,
    }
}

async fn run_transfer(sh: &Shared, mut job: Job, cancel: &CancellationToken) -> Result<u64> {
    let _slot = sh.transfer_slots.acquire().await.map_err(|_| Error::Cancelled)?;
    let fs = file_ops(sh, job.tab).await?;
    // Resolve the overwrite choice before any byte is written.
    loop {
        let exists = match job.direction {
            Direction::Upload => fs.stat(&job.remote).await?.is_some(),
            Direction::Download => tokio::fs::try_exists(&job.local).await?,
        };
        if !exists {
            break;
        }
        let path = match job.direction {
            Direction::Upload => job.remote.clone(),
            Direction::Download => job.local.display().to_string(),
        };
        match sh.ask(Some(job.tab), PromptKind::Overwrite { path }, cancel).await.0 {
            PromptAnswer::Overwrite(Overwrite::Overwrite) => break,
            PromptAnswer::Overwrite(Overwrite::Skip) => return Ok(0),
            PromptAnswer::Overwrite(Overwrite::Rename(name)) => match job.direction {
                Direction::Upload => job.remote = join(&parent(&job.remote), &name),
                Direction::Download => job.local.set_file_name(&name),
            },
            _ => return Err(Error::Cancelled),
        }
    }
    let total = match job.direction {
        Direction::Upload => tokio::fs::metadata(&job.local).await.ok().map(|m| m.len()),
        Direction::Download => fs.stat(&job.remote).await.ok().flatten().map(|e| e.size),
    };
    sh.emit(started(&job, total)).await;

    let done = Arc::new(AtomicU64::new(0));
    let counter = done.clone();
    let progress = move |n: u64| counter.store(n, Ordering::Relaxed);
    let work = async {
        match job.direction {
            Direction::Download => fs.get(&job.remote, &job.local, &progress).await,
            Direction::Upload => fs.put(&job.local, &job.remote, &progress).await,
        }
    };
    tokio::pin!(work);
    let mut tick = tokio::time::interval(PROGRESS_EVERY);
    loop {
        tokio::select! {
            r = &mut work => return r,
            _ = tick.tick() => sh.emit(Event::TransferProgress { id: job.id, done: done.load(Ordering::Relaxed) }).await,
        }
    }
}
