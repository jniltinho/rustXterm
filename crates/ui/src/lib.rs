//! egui shell: layout, tabs, sidebar, dialogs, terminal and file widgets (design D2, D7).
//!
//! Entry point: [`App::new`] with an injected [`rxt_platform::Platform`] and an engine
//! [`Backend`] (normally [`rxt_engine::EngineHandle`]); [`App`] implements [`eframe::App`].

pub mod terminal;

mod agent;
mod app;
mod banner;
mod desktop;
mod dialogs;
mod file_tab;
mod home;
mod icons;
mod menu;
mod monitor_bar;
mod session_editor;
mod settings;
mod sftp_panel;
mod sidebar;
// Shared helpers (design R7, R9); some are adopted by other screens later.
#[allow(dead_code, reason = "shared helpers adopted screen by screen")]
mod states;
mod store_ui;
mod tabs;
mod theme;
mod tunnels;
mod vault;
#[allow(dead_code, reason = "shared helpers adopted screen by screen")]
mod widgets;

#[cfg(test)]
mod lab_e2e;
#[cfg(test)]
mod lab_gates;
#[cfg(test)]
mod shell_tests;
#[cfg(test)]
mod tests;

pub use app::{App, Backend, EXIT, GpuStart, HELP, MIN_WINDOW};
pub use icons::OVERRIDE_ENV as ICON_OVERRIDE_ENV;
pub use vault::Vault;
