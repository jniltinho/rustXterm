//! rustXterm binary: opens the GUI, runs CLI subcommands / the `mcp` server (see `rxt_agent`
//! for the exit-code table), or runs headless checks with `--self-test`.
//!
//! Startup errors (no display, engine failure) are printed and exit non-zero; nothing panics.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use eframe::egui;
use rxt_engine::{Engine, EngineConfig, Records, Waker};
use rxt_platform::Paths;
use rxt_store::logfile;
use rxt_store::settings::{self, CliOverrides, LoadOutcome};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod gpu;
mod os;

/// Native terminal and remote session manager.
#[derive(Parser)]
#[command(name = "rustxterm", version, about)]
struct Cli {
    /// Run headless checks, print a report and exit (non-zero on failure).
    #[arg(long)]
    self_test: bool,
    /// Draw with software rendering for this launch (a graphics driver keeps crashing).
    #[arg(long)]
    safe_gpu: bool,
    /// Read `RXT_*` settings from this .env file (overrides config.toml, below the environment).
    #[arg(long, value_name = "PATH")]
    env_file: Option<PathBuf>,
    /// Also write a debug log (rustXterm's steps for every protocol, plus IronRDP) to this file,
    /// `rustxterm-debug.log` in the current directory by default; the `RXT_DEBUG_LOG=PATH` environment
    /// variable does the same. Passwords and keys are never logged.
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = DEBUG_LOG)]
    debug_log: Option<PathBuf>,
    /// Print command results as stable JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<rxt_agent::cli::Command>,
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    if os::gui_only() && (cli.self_test || cli.command.is_some()) {
        // No console to print to: CLI work belongs to rustxterm-cli.exe.
        return ExitCode::from(rxt_agent::exit_code(rxt_core::control::ErrorCode::Usage));
    }
    if cli.self_test {
        return self_test();
    }
    let config = os::SystemPaths.config_dir().join("config.toml");
    let env: Vec<(String, String)> = std::env::vars().collect();
    let loaded = settings::load(&config, cli.env_file.as_deref(), &env, &CliOverrides::default());
    if let Some(command) = cli.command {
        return run_agent(command, cli.json, &loaded);
    }
    let debug_log =
        cli.debug_log.or_else(|| std::env::var_os("RXT_DEBUG_LOG").filter(|v| !v.is_empty()).map(PathBuf::from));
    let config_dir = config.parent().map(PathBuf::from).unwrap_or_default();
    let diagnostics = init_logging(&loaded.settings.log_level, debug_log.as_deref(), config_dir);
    rxt_ui::DiagnosticsControl::apply(&diagnostics, &loaded.settings.diagnostics);
    match run(loaded, config, cli.safe_gpu, diagnostics) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rustxterm: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Default `--debug-log` file, relative to the current directory.
const DEBUG_LOG: &str = "rustxterm-debug.log";

/// Console log at `level` (or `RUST_LOG`), plus the diagnostic log file layer, always registered:
/// its filter is reloaded and its file reopened by Settings → Diagnostics without a restart
/// (`reload` cannot replace or add a filtered layer later). With `debug_log` (`--debug-log`) the
/// file is that one at full debug for this run; the default name falls back to `config_dir` when
/// the current directory is not writable (an app started from Finder or a Windows shortcut).
fn init_logging(level: &str, debug_log: Option<&std::path::Path>, config_dir: PathBuf) -> Diagnostics {
    let console = tracing_subscriber::fmt::layer()
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)));
    let sink =
        logfile::Sink::start("rustxterm-log").inspect_err(|e| eprintln!("rustxterm: diagnostic log writer: {e}")).ok();
    let (filter, handle) = tracing_subscriber::reload::Layer::new(EnvFilter::new("off"));
    let file_layer = sink.clone().map(|sink| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_thread_ids(true)
            .with_target(true)
            .with_writer(move || logfile::SinkWriter { sink: sink.clone(), key: LOG_KEY })
            .with_filter(filter)
    });
    tracing_subscriber::registry().with(console).with(file_layer).init();
    let reload: Reload = Box::new(move |f: &str| handle.reload(EnvFilter::new(f)).map_err(|e| e.to_string()));
    let mut diagnostics = Diagnostics { reload, sink, config_dir, forced: None };
    if let Some(path) = debug_log {
        diagnostics.forced = diagnostics.open_forced(path);
    }
    diagnostics
}

/// Sink key of the diagnostic log file.
const LOG_KEY: u64 = 0;

/// The diagnostic log behind Settings → Diagnostics (add-diagnostic-logging D1, D2, D5).
/// Swaps the diagnostic log filter (a `reload` handle).
type Reload = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

struct Diagnostics {
    reload: Reload,
    sink: Option<logfile::Sink>,
    config_dir: PathBuf,
    /// `--debug-log` file: wins over the Settings level for this run.
    forced: Option<PathBuf>,
}

impl Diagnostics {
    /// Opens the `--debug-log` file at full debug; the default name falls back to the config folder.
    fn open_forced(&self, path: &std::path::Path) -> Option<PathBuf> {
        let sink = self.sink.as_ref()?;
        let fallback = (path == std::path::Path::new(DEBUG_LOG)).then(|| self.config_dir.join(DEBUG_LOG));
        for p in std::iter::once(path.to_path_buf()).chain(fallback) {
            let p = std::path::absolute(&p).unwrap_or(p);
            sink.open(LOG_KEY, p.clone());
            sink.flush();
            match sink.take_error() {
                None => {
                    let _ = (self.reload)(&rxt_core::diag::debug_filter());
                    eprintln!("rustxterm: writing the debug log to {}", p.display());
                    tracing::info!(version = env!("CARGO_PKG_VERSION"), os = std::env::consts::OS, "debug log started");
                    return Some(p);
                }
                Some(e) => eprintln!("rustxterm: debug log: {e}"),
            }
        }
        None
    }
}

impl rxt_ui::DiagnosticsControl for Diagnostics {
    fn apply(&self, s: &rxt_core::DiagSettings) -> rxt_ui::DiagState {
        let mut state = rxt_ui::DiagState::default();
        match logfile::logs_dir(s.folder.as_deref(), &self.config_dir) {
            Ok((dir, note)) => {
                state.dir = Some(dir);
                state.note = note;
            }
            Err(e) => state.error = Some(format!("no usable folder for diagnostics: {e}")),
        }
        let Some(sink) = &self.sink else {
            state.error = Some("the diagnostic log writer did not start".into());
            return state;
        };
        if let Some(path) = &self.forced {
            state.file = Some(path.clone());
            state.forced = true;
            return state;
        }
        sink.close(LOG_KEY);
        let filter = rxt_core::diag::filter(s.level, &s.protocols);
        let (Some(filter), Some(dir)) = (filter, state.dir.clone()) else {
            let _ = (self.reload)("off");
            return state;
        };
        let file = dir.join(logfile::DIAGNOSTIC_LOG);
        // Queued, not awaited: a failure to open shows up through `poll_error`.
        sink.open(LOG_KEY, file.clone());
        if let Err(e) = (self.reload)(&filter) {
            state.error = Some(format!("cannot change the log level: {e}"));
        }
        tracing::info!(
            level = s.level.label(),
            version = env!("CARGO_PKG_VERSION"),
            os = std::env::consts::OS,
            "diagnostic log started"
        );
        state.file = Some(file);
        state
    }

    fn poll_error(&self) -> Option<String> {
        let e = self.sink.as_ref()?.take_error()?;
        // Nothing more can be written: stop formatting log lines for a closed file.
        let _ = (self.reload)("off");
        Some(e)
    }

    fn flush(&self) {
        if let Some(sink) = &self.sink {
            sink.flush();
        }
    }
}

const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/rustxterm-256.png");

/// Window icon (X11 `_NET_WM_ICON`; Wayland uses the app id and the installed .desktop file).
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).unwrap_or_default()
}

fn run(loaded: LoadOutcome, config: PathBuf, safe_gpu: bool, diagnostics: Diagnostics) -> anyhow::Result<()> {
    let platform = os::platform();
    let known_hosts = platform.paths.ssh_dir().join("known_hosts");
    let data_dir = loaded.settings.data_dir.clone().unwrap_or_else(|| platform.paths.data_dir());
    // Owner-only access before the store opens (Windows ACL; Unix keeps per-file modes).
    platform.paths.make_private(&data_dir)?;
    let vault = Arc::new(rxt_ui::Vault::new(data_dir, platform.keyring.clone()));
    // Dock/taskbar icon on Wayland comes from a .desktop entry named after the app id.
    #[cfg(target_os = "linux")]
    if let Err(e) = rxt_platform_linux::install_desktop_entry(APP_ICON_PNG) {
        tracing::warn!("desktop entry: {e}");
    }
    // Crash-safe graphics start (see `gpu`): a marker left by a start that died picks a safer backend.
    let os = gpu::Os::current();
    let config_dir = config.parent().map(PathBuf::from).unwrap_or_default();
    let preference = gpu::Backend::parse(&loaded.settings.gpu_backend);
    let plan = gpu::plan(preference, gpu::Marker::died(&config_dir), safe_gpu, os);
    if let Some(notice) = plan.notice() {
        tracing::warn!("{notice}");
    }
    let marker = gpu::Marker::start(&config_dir, plan.backend)
        .inspect_err(|e| tracing::warn!("graphics start marker: {e}"))
        .ok();
    let gpu_start = rxt_ui::GpuStart {
        save: plan.to_save().map(|b| b.name().to_owned()),
        notice: plan.notice(),
        on_started: Box::new(move || {
            if let Some(mut m) = marker {
                m.clear();
            }
        }),
    };
    // macOS falls back from Metal (wgpu) to the glow (OpenGL) renderer when the backend plan is
    // Gl/Cpu; every other platform renders through wgpu. glow is compiled in only on macOS.
    // `WGPU_BACKEND` keeps its documented override: when set it forces the wgpu (Metal) path.
    #[cfg(target_os = "macos")]
    let renderer =
        if std::env::var_os("WGPU_BACKEND").is_none() && matches!(plan.backend, gpu::Backend::Gl | gpu::Backend::Cpu) {
            eframe::Renderer::Glow
        } else {
            eframe::Renderer::Wgpu
        };
    #[cfg(not(target_os = "macos"))]
    let renderer = eframe::Renderer::Wgpu;
    let options = eframe::NativeOptions {
        renderer,
        viewport: egui::ViewportBuilder::default()
            .with_app_id("rustxterm")
            .with_title("rustXterm")
            .with_icon(app_icon())
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size(rxt_ui::MIN_WINDOW),
        // Size, maximized, fullscreen and (on X11) position are restored by eframe; the app
        // refits a window restored onto a disconnected monitor.
        persist_window: true,
        wgpu_options: gpu::configuration(plan.backend, os).unwrap_or_default(),
        ..Default::default()
    };
    let config_path = config;
    // winit dlopens libxkbcommon(-x11) and panics when it is missing (e.g. a minimal Arch/Xfce
    // install): report that as a startup error; any other panic propagates unchanged.
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_window(options, config_path, loaded, platform, vault, known_hosts, gpu_start, diagnostics)
    }));
    run.unwrap_or_else(|panic| match missing_library(panic.as_ref()) {
        Some(lib) => Err(anyhow::anyhow!("cannot open the window: {lib} is not installed")),
        None => std::panic::resume_unwind(panic),
    })
}

/// The library named by xkbcommon-dl's "Library <name> could not be loaded." panic.
fn missing_library(panic: &(dyn std::any::Any + Send)) -> Option<&str> {
    let msg = panic.downcast_ref::<&str>().copied().or_else(|| panic.downcast_ref::<String>().map(String::as_str))?;
    msg.strip_prefix("Library ")?.strip_suffix(" could not be loaded.")
}

#[allow(clippy::too_many_arguments)]
fn run_window(
    options: eframe::NativeOptions,
    config_path: PathBuf,
    loaded: LoadOutcome,
    platform: rxt_platform::Platform,
    vault: Arc<rxt_ui::Vault>,
    known_hosts: PathBuf,
    gpu_start: rxt_ui::GpuStart,
    diagnostics: Diagnostics,
) -> anyhow::Result<()> {
    eframe::run_native(
        "rustxterm",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let waker: Waker = Arc::new(move || ctx.request_repaint());
            let records: Arc<dyn Records> = vault.clone();
            let control = Some(platform.control.clone());
            let config = EngineConfig {
                known_hosts,
                records: Some(records),
                control,
                serial_ports: Some(platform.serial.clone()),
                local_pty: platform.local_pty.clone(),
                wsl: platform.wsl.clone(),
                ..EngineConfig::default()
            };
            let engine = Engine::start(config, waker)?;
            let app =
                rxt_ui::App::new(cc.storage, platform, engine).with_settings(loaded, config_path).with_vault(vault);
            let app = app.with_gpu_start(gpu_start).with_diagnostics(Box::new(diagnostics));
            Ok(Box::new(app))
        }),
    )
    // The marker stays: the next start tries a safer backend.
    .map_err(|e| anyhow::anyhow!("cannot open the window: {e} (the next start tries a safer graphics backend)"))
}

/// CLI subcommands and `mcp`; never opens a window.
fn run_agent(command: rxt_agent::cli::Command, json: bool, loaded: &LoadOutcome) -> ExitCode {
    let paths = os::SystemPaths;
    let ctx = rxt_agent::Ctx {
        data_dir: loaded.settings.data_dir.clone().unwrap_or_else(|| paths.data_dir()),
        keyring: os::keyring(),
        control: os::control(),
    };
    if let Err(e) = paths.make_private(&ctx.data_dir) {
        eprintln!("rustxterm: {e}");
        return ExitCode::FAILURE;
    }
    match command {
        rxt_agent::cli::Command::Mcp => match rxt_agent::mcp::run(ctx) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rustxterm: {e}");
                ExitCode::from(rxt_agent::exit_code(e.code))
            }
        },
        other => ExitCode::from(rxt_agent::cli::run(other, json, &ctx)),
    }
}

type Check = fn() -> anyhow::Result<()>;

/// Headless `--self-test` checks; adding one is one entry here.
const SELF_TESTS: &[(&str, Check)] = &[
    ("config defaults", check_config_defaults),
    ("terminal echo", check_terminal_echo),
    ("theme tokens", check_theme_tokens),
    ("engine start/shutdown", check_engine),
    ("encrypted database", check_database),
    ("mcp handshake", check_mcp),
];

fn self_test() -> ExitCode {
    println!("rustxterm {} self-test", env!("CARGO_PKG_VERSION"));
    let mut failed = 0;
    for (name, check) in SELF_TESTS {
        match check() {
            Ok(()) => println!("  ok    {name}"),
            Err(e) => {
                failed += 1;
                println!("  FAIL  {name}: {e:#}");
            }
        }
    }
    println!("{} passed, {failed} failed", SELF_TESTS.len() - failed);
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn check_config_defaults() -> anyhow::Result<()> {
    use rxt_platform::Paths;
    let paths = os::SystemPaths;
    anyhow::ensure!(paths.config_dir().ends_with("rustxterm"), "config dir {}", paths.config_dir().display());
    anyhow::ensure!(paths.data_dir().ends_with("rustxterm"), "data dir {}", paths.data_dir().display());
    let engine = EngineConfig::default();
    anyhow::ensure!(engine.output_buffer_bytes > 0 && engine.command_queue > 0, "engine defaults");
    anyhow::ensure!(rxt_core::ThemeMode::default() == rxt_core::ThemeMode::System, "theme default");
    Ok(())
}

fn check_terminal_echo() -> anyhow::Result<()> {
    let mut term = rxt_term::Terminal::new(80, 24, 100);
    term.feed(b"rustxterm \x1b[1mself-test\x1b[0m\r\nok");
    let text = term.visible_text();
    anyhow::ensure!(text == "rustxterm self-test\nok\n", "unexpected screen {text:?}");
    Ok(())
}

fn check_theme_tokens() -> anyhow::Result<()> {
    use rxt_core::{TerminalScheme, Tokens};
    for dark in [false, true] {
        let t = Tokens::for_dark(dark);
        anyhow::ensure!(t.dark == dark && t.text != t.surface, "tokens (dark={dark})");
        anyhow::ensure!(TerminalScheme::for_dark(dark) != TerminalScheme::for_dark(!dark), "terminal scheme");
    }
    Ok(())
}

fn check_engine() -> anyhow::Result<()> {
    let engine = Engine::start(EngineConfig::default(), Arc::new(|| {}))?;
    engine.shutdown(std::time::Duration::from_secs(2));
    Ok(())
}

fn check_database() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rustxterm-self-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let result = database_round_trip(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn database_round_trip(dir: &std::path::Path) -> anyhow::Result<()> {
    let keyring: Arc<dyn rxt_platform::Keyring> = Arc::new(rxt_platform::fake::Fake::default());
    let master = "self-test master password";
    rxt_ui::Vault::fast(dir.to_owned(), keyring.clone()).create(master, false)?;
    let vault = rxt_ui::Vault::fast(dir.to_owned(), keyring);
    vault.unlock(master)?;
    let session = rxt_core::Session::new(rxt_core::Protocol::Ssh, "self-test.example", None, None);
    let id = vault.with(|db| db.save_session(&session))?;
    let back = vault.with(|db| db.session(id))?;
    anyhow::ensure!(back.host == session.host, "session read back as {:?}", back.host);
    Ok(())
}

fn check_mcp() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("rustxterm-self-test-mcp-{}", std::process::id()));
    let ctx = rxt_agent::Ctx {
        data_dir: dir.join("data"),
        keyring: Arc::new(rxt_platform::fake::Fake::default()),
        control: Arc::new(rxt_platform::fake::Fake::default()),
    };
    let tools = rxt_agent::mcp::self_test(ctx).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(tools.iter().any(|t| t == "list_sessions"), "tools/list returned {tools:?}");
    anyhow::ensure!(!tools.iter().any(|t| t.contains("add")), "mutation tool listed: {tools:?}");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_library_names_only_the_dlopen_panic() {
        let p: Box<dyn std::any::Any + Send> = Box::new("Library libxkbcommon-x11.so could not be loaded.");
        assert_eq!(super::missing_library(p.as_ref()), Some("libxkbcommon-x11.so"));
        let other: Box<dyn std::any::Any + Send> = Box::new(String::from("index out of bounds"));
        assert_eq!(super::missing_library(other.as_ref()), None);
    }
}
