//! Remote monitoring data shared by the engine (collector) and the UI (bar).

use serde::{Deserialize, Serialize};

/// One local filesystem of the server (`df -P` row kept by the collector).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disk {
    /// Device, e.g. `/dev/sda1` (or `overlay` for a container root).
    pub source: String,
    pub mount: String,
    pub used_kib: u64,
    /// Used plus available (what `df` bases its percentage on).
    pub size_kib: u64,
    /// Used percent, rounded up like `df`.
    pub percent: u8,
}

/// Latest metrics of a monitored SSH tab; `None` hides the chip.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricsSample {
    /// Server host name (`uname -n`).
    pub host: String,
    /// Server OS (`uname -s`), e.g. `Linux`, `FreeBSD`.
    pub os: String,
    /// CPU busy percent since the previous sample.
    pub cpu: Option<f32>,
    /// User + nice + system part of [`Self::cpu`].
    pub cpu_user_system: Option<f32>,
    /// Memory used and total, bytes.
    pub mem: Option<(u64, u64)>,
    /// Upload and download rates, bits per second.
    pub net: Option<(u64, u64)>,
    /// Interfaces counted in [`Self::net`].
    pub net_ifaces: Vec<String>,
    pub uptime_secs: Option<u64>,
    pub disks: Option<Vec<Disk>>,
    /// Disks left out by the chip cap.
    pub disks_more: usize,
}

/// Monitoring state of a tab (`Event::Monitor`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonitorState {
    /// Not monitored (setting off, closed with ×, or not an SSH terminal).
    Off,
    /// Collecting; samples arrive through the engine's latest-sample slot.
    On,
    /// The server cannot run the collector (no POSIX shell, forced command, SFTP-only account).
    Unsupported(String),
    /// The server refused the extra channel; not retried until toggled or reconnected.
    Unavailable(String),
    /// Stopped after repeated failures or a closed channel.
    Stopped(String),
}

impl MonitorState {
    /// Why monitoring is not running, for tooltips.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Unsupported(r) | Self::Unavailable(r) | Self::Stopped(r) => Some(r),
            Self::Off | Self::On => None,
        }
    }
}
