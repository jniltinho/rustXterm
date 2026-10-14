## ADDED Requirements

### Requirement: Remote monitoring bar
SSH terminal tabs with monitoring active SHALL show a bar 26 ± 1 px high along the bottom of the tab area, below the terminal and above the window status bar, without overlapping the terminal grid. Left to right it SHALL hold chips with catalog icons: host, CPU percent with a sparkline, memory used/total, upload, download, uptime, then one chip per disk. CPU, memory and disk values SHALL use the theme's warning color at 80 % or more and its danger color at 95 % or more. Values with no reply for two intervals SHALL render muted. When the chips do not fit, left/right arrow buttons SHALL scroll them. A close (×) button SHALL always be visible at the right end and SHALL turn monitoring off for that tab. Every chip and button SHALL have an accessible name and a tooltip with details. The bar SHALL be complete in light, dark and system themes.

#### Scenario: Bar appears
- **WHEN** an SSH tab connects to a Linux server with remote monitoring on
- **THEN** within 5 s the bar shows the host, CPU, memory, throughput, uptime and root disk chips

#### Scenario: Close button
- **WHEN** the user clicks the bar's × button
- **THEN** the bar disappears from that tab only, the terminal grows by the bar's height, and the monitoring channel closes

#### Scenario: Overflow
- **WHEN** the window is too narrow for all chips
- **THEN** arrow buttons appear, scroll the chips one at a time, and the × stays visible

#### Scenario: Dark theme thresholds
- **WHEN** CPU usage is 97 % in the dark theme
- **THEN** the CPU value uses the dark theme's danger token

### Requirement: Remote monitoring settings
Settings › SSH SHALL contain a "Remote monitoring" group with "Show remote monitoring bar on SSH tabs" (default on) and "Refresh interval" (1–30 s, default 2). The SSH session editor's advanced options SHALL offer "Remote monitoring": Use global setting (default), On, Off. The effective state SHALL be decided when a tab connects: the session choice overrides the global one. The interval SHALL apply to running tabs immediately.

#### Scenario: Session opt-out
- **WHEN** the global setting is on and a session sets Remote monitoring to Off
- **THEN** that session's tabs connect without the bar or a monitoring channel

#### Scenario: Interval change
- **WHEN** the user changes the refresh interval from 2 s to 10 s
- **THEN** visible monitored tabs refresh every 10 s from then on without reconnecting

### Requirement: SFTP panel monitoring button
The "Remote monitoring" button at the bottom of the SFTP side panel SHALL toggle monitoring for the attached SSH tab. It SHALL appear pressed while monitoring is active, and it SHALL be disabled with a tooltip giving the reason when monitoring is unsupported or unavailable on that connection. The toggle SHALL not change any saved setting.

#### Scenario: Turn on from the panel
- **WHEN** the bar of the attached tab was closed and the user clicks "Remote monitoring"
- **THEN** the bar reappears with fresh values and the button shows as pressed

#### Scenario: Toggle is per tab
- **WHEN** two SSH tabs are open and the user turns monitoring off in one
- **THEN** the other tab keeps its bar
