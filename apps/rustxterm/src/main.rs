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
use rxt_store::settings::{self, CliOverrides, LoadOutcome};
use tracing_subscriber::EnvFilter;

mod os;

/// Native terminal and remote session manager.
#[derive(Parser)]
#[command(name = "rustxterm", version, about)]
struct Cli {
    /// Run headless checks, print a report and exit (non-zero on failure).
    #[arg(long)]
    self_test: bool,
    /// Read `RXT_*` settings from this .env file (overrides config.toml, below the environment).
    #[arg(long, value_name = "PATH")]
    env_file: Option<PathBuf>,
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
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&loaded.settings.log_level)),
        )
        .init();
    match run(loaded, config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rustxterm: {e:#}");
            ExitCode::FAILURE
        }
    }
}

const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/rustxterm-256.png");

/// Window icon (X11 `_NET_WM_ICON`; Wayland uses the app id and the installed .desktop file).
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).unwrap_or_default()
}

fn run(loaded: LoadOutcome, config: PathBuf) -> anyhow::Result<()> {
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
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_app_id("rustxterm")
            .with_title("rustXterm")
            .with_icon(app_icon())
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size(rxt_ui::MIN_WINDOW),
        // Size, maximized, fullscreen and (on X11) position are restored by eframe; the app
        // refits a window restored onto a disconnected monitor.
        persist_window: true,
        #[cfg(windows)]
        wgpu_options: windows_wgpu(),
        ..Default::default()
    };
    let config_path = config;
    // winit dlopens libxkbcommon(-x11) and panics when it is missing (e.g. a minimal Arch/Xfce
    // install): report that as a startup error; any other panic propagates unchanged.
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_window(options, config_path, loaded, platform, vault, known_hosts)
    }));
    run.unwrap_or_else(|panic| match missing_library(panic.as_ref()) {
        Some(lib) => Err(anyhow::anyhow!("cannot open the window: {lib} is not installed")),
        None => std::panic::resume_unwind(panic),
    })
}

/// Windows renders through OpenGL when a hardware OpenGL adapter exists: DX12 presentation
/// flickers in bursts on Intel graphics while the window repaints. Without one (GPU-less VMs,
/// WARP) the usual choice applies. `WGPU_BACKEND` still overrides everything.
#[cfg(windows)]
fn windows_wgpu() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup};
    let mut config = WgpuConfiguration::default();
    if std::env::var_os("WGPU_BACKEND").is_some() {
        return config;
    }
    if let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        setup.native_adapter_selector = Some(Arc::new(|adapters, surface| {
            let usable: Vec<&eframe::wgpu::Adapter> =
                adapters.iter().filter(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
            let infos: Vec<_> = usable.iter().map(|a| a.get_info()).collect();
            let pick = pick_adapter(&infos.iter().map(|i| (i.backend, i.device_type)).collect::<Vec<_>>());
            pick.and_then(|i| usable.get(i)).map(|a| (*a).clone()).ok_or_else(|| "no usable wgpu adapter".to_owned())
        }));
    }
    config
}

/// Index of the adapter to render on: a hardware OpenGL one first, then by device type as wgpu
/// ranks them for high performance (discrete, integrated, virtual, other, CPU).
#[cfg_attr(not(windows), allow(dead_code))]
fn pick_adapter(adapters: &[(eframe::wgpu::Backend, eframe::wgpu::DeviceType)]) -> Option<usize> {
    use eframe::wgpu::{Backend, DeviceType};
    let rank = |&(backend, kind): &(Backend, DeviceType)| {
        let device = match kind {
            DeviceType::DiscreteGpu => 0,
            DeviceType::IntegratedGpu => 1,
            DeviceType::VirtualGpu => 2,
            DeviceType::Other => 3,
            DeviceType::Cpu => 4,
        };
        (!(backend == Backend::Gl && kind != DeviceType::Cpu), device)
    };
    (0..adapters.len()).min_by_key(|&i| rank(&adapters[i]))
}

/// The library named by xkbcommon-dl's "Library <name> could not be loaded." panic.
fn missing_library(panic: &(dyn std::any::Any + Send)) -> Option<&str> {
    let msg = panic.downcast_ref::<&str>().copied().or_else(|| panic.downcast_ref::<String>().map(String::as_str))?;
    msg.strip_prefix("Library ")?.strip_suffix(" could not be loaded.")
}

fn run_window(
    options: eframe::NativeOptions,
    config_path: PathBuf,
    loaded: LoadOutcome,
    platform: rxt_platform::Platform,
    vault: Arc<rxt_ui::Vault>,
    known_hosts: PathBuf,
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
                ..EngineConfig::default()
            };
            let engine = Engine::start(config, waker)?;
            let app =
                rxt_ui::App::new(cc.storage, platform, engine).with_settings(loaded, config_path).with_vault(vault);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("cannot open the window: {e}"))
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

    #[test]
    fn hardware_opengl_wins_then_device_type() {
        use eframe::wgpu::{Backend, DeviceType};
        let intel = [(Backend::Dx12, DeviceType::IntegratedGpu), (Backend::Gl, DeviceType::IntegratedGpu)];
        assert_eq!(super::pick_adapter(&intel), Some(1));
        // GPU-less VM: WARP on DX12 (CPU and output-owning), no hardware OpenGL.
        let vm = [(Backend::Dx12, DeviceType::Cpu), (Backend::Dx12, DeviceType::Other), (Backend::Gl, DeviceType::Cpu)];
        assert_eq!(super::pick_adapter(&vm), Some(1));
        assert_eq!(super::pick_adapter(&[]), None);
    }
}
