//! Remote monitoring bar at the bottom of SSH terminal tabs (design M6 of
//! `add-remote-monitoring`): chips for host, CPU with a sparkline, memory, upload, download,
//! uptime and disks, scroll arrows when they overflow, and a close button.

use std::collections::VecDeque;

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, WidgetInfo, WidgetType, pos2, vec2};
use rxt_core::{MetricsSample, MonitorState, TabId, Tokens};

use crate::app::Action;
use crate::icons::Icons;
use crate::theme::color;
use crate::widgets::describe;

/// Bar height (logical px).
pub(crate) const BAR_H: f32 = 26.0;
/// CPU values kept for the sparkline.
const HISTORY: usize = 60;
const SPARK: egui::Vec2 = vec2(48.0, 14.0);
const ICON: f32 = 16.0;
const BUTTON: f32 = 24.0;

/// Monitoring state of one tab as the UI knows it.
#[derive(Debug, Default)]
pub(crate) struct MonitorView {
    /// Last state reported by the engine; `None` until the first report.
    pub state: Option<MonitorState>,
    pub sample: Option<MetricsSample>,
    /// Last [`HISTORY`] CPU percentages, oldest first.
    pub cpu: VecDeque<f32>,
    /// `egui` time of the last sample.
    pub at: Option<f64>,
    /// First chip shown (overflow scrolling).
    pub scroll: usize,
}

impl MonitorView {
    /// The bar is shown: monitoring is on, or stopped with a reason to show.
    pub(crate) fn shown(&self) -> bool {
        self.state.as_ref().is_some_and(|s| *s != MonitorState::Off)
    }

    /// New state; starting again resets the values and the history.
    pub(crate) fn set_state(&mut self, state: MonitorState) {
        if state == MonitorState::On && self.state != Some(MonitorState::On) {
            *self = Self::default();
        }
        self.state = Some(state);
    }

    pub(crate) fn push(&mut self, sample: MetricsSample, now: f64) {
        if let Some(cpu) = sample.cpu {
            if self.cpu.len() == HISTORY {
                self.cpu.pop_front();
            }
            self.cpu.push_back(cpu);
        }
        self.sample = Some(sample);
        self.at = Some(now);
    }

    /// Why the SFTP panel button is disabled.
    pub(crate) fn disabled_reason(&self) -> Option<&str> {
        match &self.state {
            Some(MonitorState::Unsupported(r) | MonitorState::Unavailable(r)) => Some(r),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Level {
    Normal,
    Warning,
    Danger,
}

fn level(percent: f32) -> Level {
    if percent >= 95.0 {
        Level::Danger
    } else if percent >= 80.0 {
        Level::Warning
    } else {
        Level::Normal
    }
}

struct Chip {
    icon: &'static str,
    text: String,
    /// Accessible name.
    name: String,
    tip: String,
    level: Level,
    spark: bool,
}

impl Chip {
    fn new(icon: &'static str, what: &str, text: String, tip: String) -> Self {
        Self { icon, name: format!("{what}: {text}"), text, tip, level: Level::Normal, spark: false }
    }
}

/// Bits per second, e.g. `0.02 Mb/s`.
pub(crate) fn rate(bps: u64) -> String {
    let b = bps as f64;
    if b >= 1e9 {
        format!("{:.2} Gb/s", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.2} Mb/s", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.1} kb/s", b / 1e3)
    } else {
        format!("{bps} b/s")
    }
}

/// Memory size in GiB (MiB below 1 GiB).
pub(crate) fn mem(bytes: u64) -> String {
    let gib = bytes as f64 / f64::from(1u32 << 30);
    if gib >= 1.0 { format!("{gib:.2} GiB") } else { format!("{:.0} MiB", bytes as f64 / f64::from(1u32 << 20)) }
}

/// `12 min`, `5 h 12 min` or `3 d 4 h`.
pub(crate) fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    match (d, h) {
        (0, 0) => format!("{m} min"),
        (0, _) => format!("{h} h {m} min"),
        _ => format!("{d} d {h} h"),
    }
}

fn chips(view: &MonitorView, fallback_host: &str) -> Vec<Chip> {
    let s = view.sample.as_ref();
    let host = s.map(|s| s.host.as_str()).filter(|h| !h.is_empty()).unwrap_or(fallback_host);
    let os = s.map_or("", |s| s.os.as_str());
    let mut host_tip = format!("Host: {host}");
    if !os.is_empty() {
        host_tip.push_str(&format!("\nOS: {os}"));
    }
    let linux = os.is_empty() || os == "Linux";
    if !linux {
        host_tip.push_str(&format!(
            "\nCPU, memory, network and uptime need a Linux server (/proc); this server reports {os}"
        ));
    }
    let mut out = vec![Chip::new("server-tower", "Host", host.to_owned(), host_tip)];
    if let Some(reason) = view.state.as_ref().and_then(MonitorState::reason) {
        let text = match view.state {
            Some(MonitorState::Stopped(_)) => "Monitoring stopped",
            Some(MonitorState::Unsupported(_)) => "Monitoring not supported",
            _ => "Monitoring unavailable",
        };
        let mut c = Chip::new("monitor-pulse", "Remote monitoring", text.into(), reason.to_owned());
        c.level = Level::Warning;
        out.push(c);
        return out;
    }
    if linux {
        let cpu = s.and_then(|s| s.cpu);
        let text = cpu.map_or_else(|| "…".to_owned(), |c| format!("{c:.0} %"));
        let tip = match s.and_then(|s| s.cpu_user_system) {
            Some(us) => format!("CPU usage {text} (user+system {us:.0} %)"),
            None => "CPU usage (needs two samples)".to_owned(),
        };
        let mut c = Chip::new("chip", "CPU", text, tip);
        c.level = cpu.map_or(Level::Normal, level);
        c.spark = true;
        out.push(c);
    }
    let Some(s) = s else { return out };
    if let Some((used, total)) = s.mem {
        let tip = format!("Memory: {used} of {total} bytes used");
        let mut c = Chip::new("ram", "Memory", format!("{} / {}", mem(used), mem(total)), tip);
        c.level = if total == 0 { Level::Normal } else { level(used as f32 / total as f32 * 100.0) };
        out.push(c);
    }
    if let Some((tx, rx)) = s.net {
        let ifaces = s.net_ifaces.join(", ");
        out.push(Chip::new("upload", "Upload", rate(tx), format!("Upload on {ifaces}")));
        out.push(Chip::new("download", "Download", rate(rx), format!("Download on {ifaces}")));
    }
    if let Some(up) = s.uptime_secs {
        out.push(Chip::new("monitor-clock", "Uptime", uptime(up), format!("Up for {}", uptime(up))));
    }
    let disks = s.disks.as_deref().unwrap_or_default();
    for (i, d) in disks.iter().enumerate() {
        let mut tip =
            format!("{} on {}: {} of {} used", d.source, d.mount, mem(d.used_kib * 1024), mem(d.size_kib * 1024));
        if i + 1 == disks.len() && s.disks_more > 0 {
            tip.push_str(&format!("\n+{} more", s.disks_more));
        }
        let mut c = Chip::new("hard-disk", "Disk", format!("{}: {} %", d.mount, d.percent), tip);
        c.level = level(f32::from(d.percent));
        out.push(c);
    }
    out
}

/// Paint the bar into `ui` (a 28 px bottom panel of the tab). `interval_secs` decides when
/// values count as stale (no reply for two intervals).
#[allow(clippy::too_many_arguments)]
pub(crate) fn bar(
    ui: &mut egui::Ui,
    view: &mut MonitorView,
    tab: TabId,
    fallback_host: &str,
    interval_secs: u32,
    icons: &Icons,
    tokens: &Tokens,
    out: &mut Vec<Action>,
) {
    let rect = ui.max_rect();
    let now = ui.input(|i| i.time);
    let stale_after = 2.0 * f64::from(interval_secs.max(1));
    let stale = view.state == Some(MonitorState::On) && view.at.is_some_and(|t| now - t > stale_after);
    if view.state == Some(MonitorState::On) {
        // Repaint to show staleness when no sample arrives.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(stale_after + 0.1));
    }
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, color(tokens.panel));
    p.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, color(tokens.border)));
    let font = egui::TextStyle::Small.resolve(ui.style());
    let chips = chips(view, fallback_host);
    let galleys: Vec<_> =
        chips.iter().map(|c| p.layout_no_wrap(c.text.clone(), font.clone(), Color32::PLACEHOLDER)).collect();
    let widths: Vec<f32> = chips
        .iter()
        .zip(&galleys)
        .map(|(c, g)| 6.0 + ICON + 4.0 + g.size().x + if c.spark { 4.0 + SPARK.x } else { 0.0 } + 6.0)
        .collect();
    let gap = 3.0;
    let close = Rect::from_center_size(pos2(rect.right() - BUTTON / 2.0 - 1.0, rect.center().y), vec2(BUTTON, BUTTON));
    let full: f32 = widths.iter().map(|w| w + gap).sum();
    let mut right = close.left() - 2.0;
    let overflow = full > right - rect.left() - 2.0;
    if overflow {
        right -= 2.0 * BUTTON;
    }
    view.scroll = view.scroll.min(chips.len().saturating_sub(1));
    if !overflow {
        view.scroll = 0;
    }
    let area = Rect::from_min_max(pos2(rect.left() + 2.0, rect.top() + 1.0), pos2(right, rect.bottom()));
    let clip = p.with_clip_rect(area);
    let mut x = area.left();
    let mut last_fully_shown = view.scroll;
    for (i, ((chip, galley), w)) in chips.iter().zip(&galleys).zip(&widths).enumerate().skip(view.scroll) {
        if x >= area.right() {
            break;
        }
        let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(*w, rect.height() - 3.0));
        if r.right() <= area.right() {
            last_fully_shown = i;
        }
        clip.rect_stroke(r, 2.0, Stroke::new(1.0, color(tokens.border)), StrokeKind::Inside);
        let icon_rect = Rect::from_min_size(pos2(r.left() + 6.0, r.center().y - ICON / 2.0), vec2(ICON, ICON));
        if let Some(img) = icons.image(chip.icon, ICON)
            && icon_rect.right() <= area.right()
        {
            img.paint_at(ui, icon_rect);
        }
        let text_color = match chip.level {
            _ if stale => tokens.text_muted,
            Level::Danger => tokens.danger,
            Level::Warning => tokens.warning,
            Level::Normal => tokens.text,
        };
        let tx = icon_rect.right() + 4.0;
        clip.galley(pos2(tx, r.center().y - galley.size().y / 2.0), galley.clone(), color(text_color));
        if chip.spark {
            let s = Rect::from_min_size(pos2(tx + galley.size().x + 4.0, r.center().y - SPARK.y / 2.0), SPARK);
            sparkline(&clip, s, &view.cpu, tokens, stale);
        }
        let resp = ui.interact(r.intersect(area), ui.id().with(("monitor-chip", tab, i)), Sense::hover());
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &chip.name));
        resp.on_hover_text(&chip.tip);
        x += w + gap;
    }
    if overflow {
        let back = Rect::from_min_size(pos2(right, close.top()), vec2(BUTTON, BUTTON));
        let fwd = back.translate(vec2(BUTTON, 0.0));
        let can_fwd = last_fully_shown + 1 < chips.len();
        if button(ui, back, icons, "back", "Scroll monitoring left", view.scroll > 0).clicked() {
            view.scroll -= 1;
        }
        if button(ui, fwd, icons, "forward", "Scroll monitoring right", can_fwd).clicked() {
            view.scroll += 1;
        }
    }
    if button(ui, close, icons, "close", "Close remote monitoring", true).clicked() {
        out.push(Action::SetMonitoring(tab, false));
    }
}

fn sparkline(p: &egui::Painter, r: Rect, cpu: &VecDeque<f32>, tokens: &Tokens, stale: bool) {
    p.rect_filled(r, 0.0, color(tokens.surface));
    p.rect_stroke(r, 0.0, Stroke::new(1.0, color(tokens.border_subtle)), StrokeKind::Inside);
    let step = r.width() / (HISTORY - 1) as f32;
    let start = HISTORY - cpu.len();
    let points: Vec<_> = cpu
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(r.left() + (start + i) as f32 * step, r.bottom() - 1.0 - v / 100.0 * (r.height() - 2.0)))
        .collect();
    let line = if stale { tokens.text_muted } else { tokens.accent };
    if points.len() > 1 {
        p.line(points, Stroke::new(1.0, color(line)));
    }
}

/// Flat icon button at `r`; accessible name and tooltip from `label`.
fn button(ui: &mut egui::Ui, r: Rect, icons: &Icons, icon: &str, label: &str, enabled: bool) -> egui::Response {
    let b = match icons.image(icon, ICON) {
        Some(img) => egui::Button::image(img),
        None => egui::Button::new(label).small(),
    };
    let resp = ui.add_enabled_ui(enabled, |ui| ui.put(r, b.frame_when_inactive(false))).inner;
    describe(resp, label, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(rate(20_000), "20.0 kb/s");
        assert_eq!(rate(2_500_000), "2.50 Mb/s");
        assert_eq!(rate(999), "999 b/s");
        assert_eq!(mem(512 << 20), "512 MiB");
        assert_eq!(mem(3 << 30), "3.00 GiB");
        assert_eq!(uptime(12 * 60), "12 min");
        assert_eq!(uptime(5 * 3600 + 12 * 60), "5 h 12 min");
        assert_eq!(uptime(3 * 86_400 + 4 * 3600 + 59), "3 d 4 h");
    }

    #[test]
    fn thresholds_and_history() {
        assert!(level(79.9) == Level::Normal && level(80.0) == Level::Warning && level(95.0) == Level::Danger);
        let mut v = MonitorView::default();
        v.set_state(MonitorState::On);
        for i in 0..70 {
            v.push(MetricsSample { cpu: Some(i as f32), ..MetricsSample::default() }, 0.0);
        }
        assert_eq!(v.cpu.len(), HISTORY);
        assert_eq!(v.cpu.front(), Some(&10.0));
        v.set_state(MonitorState::Off);
        assert!(!v.shown());
        v.set_state(MonitorState::On);
        assert!(v.cpu.is_empty() && v.shown());
    }
}
