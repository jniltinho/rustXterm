## Purpose

At-a-glance metrics of the remote server (host, CPU, memory, network throughput, uptime, disk usage) in a thin bar on SSH tabs, collected over the tab's own SSH connection without installing or writing anything on the server.

## ADDED Requirements

### Requirement: Agentless metric collection
For a monitored SSH tab, the application SHALL collect metrics through at most one additional exec channel on the tab's SSH connection to the final target. The channel SHALL run a fixed POSIX `sh` command that contains no user data or secrets, installs nothing and writes no files on the server. The remote side SHALL only answer request lines sent by the application, so it does no sampling work between requests. Metric computation (percentages, rates, filtering) SHALL happen in the application.

#### Scenario: Nothing written on the server
- **WHEN** monitoring has run on a tab for one minute
- **THEN** no file under the user's home or `/tmp` was created or modified by it, and the server shows exactly one monitoring process for that tab

#### Scenario: Jump host untouched
- **WHEN** a monitored session connects through a jump host
- **THEN** the monitoring channel is opened only on the final target and no exec request reaches the jump host

### Requirement: Metrics shown
The bar SHALL show: the server host name; CPU usage in percent with a sparkline of the last 60 samples; memory used and total (used = MemTotal − MemAvailable, or MemTotal − (MemFree + Buffers + Cached) when MemAvailable is missing); upload and download throughput in bits per second of the default-route interface(s), or of all interfaces except loopback when there is no default route; uptime; and usage in percent per mounted filesystem. Filesystems SHALL be limited to the root mount and mounts whose source is a `/dev/` device other than loop devices, de-duplicated by source, at most 8. Disk usage SHALL refresh at most every 30 s. CPU and network values SHALL be computed from the difference between two consecutive samples, and a sample whose counters went backwards SHALL be skipped for those values.

#### Scenario: First samples
- **WHEN** monitoring starts on a Linux server
- **THEN** host, memory, uptime and disks appear with the first sample, and CPU and throughput appear with the second

#### Scenario: Container mounts filtered
- **WHEN** the server's `df` lists overlay, tmpfs, devtmpfs and loop mounts besides `/` and `/dev/sda2` on `/data`
- **THEN** only the `/` and `/data` disk chips are shown

#### Scenario: Counter reset
- **WHEN** an interface's byte counter is lower than in the previous sample
- **THEN** that sample shows no throughput value instead of a negative or huge rate

### Requirement: Supported servers and degradation
Full metrics SHALL require a Linux server with `/proc` (including BusyBox userlands). On other POSIX servers, the bar SHALL show only the host and disk chips, and the host chip's tooltip SHALL say that CPU, memory, network and uptime need a Linux server and name the reported OS. A metric whose source data is missing SHALL hide its chip instead of showing a wrong value. When the server does not answer the monitoring handshake within 10 s (no POSIX shell, forced command, SFTP-only account), monitoring SHALL be marked unsupported for that connection, its channel closed, and the reason shown in a tooltip.

#### Scenario: FreeBSD server
- **WHEN** a monitored tab connects to a server reporting `FreeBSD`
- **THEN** the bar shows the host and disk chips, and the host tooltip explains why the other chips are missing

#### Scenario: SFTP-only account
- **WHEN** the account's forced command is the SFTP server
- **THEN** monitoring is marked unsupported within 10 s and the terminal and SFTP panel keep working

### Requirement: Bounded monitoring cost
The sampling interval SHALL be configurable from 1 to 30 s (default 2 s), and values outside that range SHALL be clamped. The application SHALL send no sample request for a tab whose bar has not been displayed since the previous sample (hidden tab, minimized or occluded window), and SHALL resume with an immediate request when it is displayed again. Monitoring SHALL stop and close its channel on disconnect, reconnect, tab close and when the user turns it off. Lines longer than 4 KiB SHALL be dropped and samples larger than 64 KiB discarded. A sample not answered within 10 s SHALL count as a failure, and three consecutive failures SHALL stop monitoring for the tab with the reason shown. A refused channel open SHALL mark monitoring unavailable without retrying until the user turns it on again or the tab reconnects.

#### Scenario: Hidden tab pauses
- **WHEN** a monitored tab stays in the background for one minute
- **THEN** at most one sample request was sent for it during that minute

#### Scenario: No leftover processes
- **WHEN** a monitored tab disconnects or monitoring is turned off
- **THEN** within 5 s the server has no remaining monitoring process for that tab

#### Scenario: Server refuses the extra channel
- **WHEN** the server allows only one session channel per connection
- **THEN** the shell works, the bar reports monitoring as unavailable, and no further channel opens are attempted for that tab

### Requirement: SSH sessions only
The monitoring bar and collector SHALL exist only for SSH terminal tabs. Telnet, Serial, Rsh, XDMCP, RDP, VNC (including VNC over SSH), FTP/FTPS and standalone SFTP tabs SHALL never show the bar or open a monitoring channel.

#### Scenario: Telnet tab
- **WHEN** a Telnet tab is active and remote monitoring is enabled globally
- **THEN** no monitoring bar is shown and the SFTP panel's "Remote monitoring" button is not offered

### Requirement: Not exposed to agents
Metrics and the monitoring toggle SHALL NOT be available to agents in this version: the monitoring command ids SHALL be GUI-only in the agent permission matrix and rejected from the control channel, MCP and offline CLI, and metric events SHALL NOT be forwarded to agent clients.

#### Scenario: Agent tries to toggle
- **WHEN** an MCP client sends the monitoring toggle command for a tab with policy `allow`
- **THEN** the request is rejected as GUI-only and audited
