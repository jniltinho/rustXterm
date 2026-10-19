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
    init_logging(&loaded.settings.log_level, debug_log.as_deref(), config.parent());
    match run(loaded, config, cli.safe_gpu) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rustxterm: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Default `--debug-log` file, relative to the current directory.
const DEBUG_LOG: &str = "rustxterm-debug.log";

/// Debug everywhere except the graphics stack, whose per-frame events would bury everything else, and
/// the NTLM/CredSSP code, kept at `info`: `sspi` debug spans record return values, plaintext RDP
/// credentials included (`write_password_credentials`), and `ironrdp_connector::credssp` logs the NTLM
/// messages, whose NTLMv2 response allows offline password guessing.
const DEBUG_FILTER: &str = "debug,sspi=info,picky=info,picky_krb=info,ironrdp_connector::credssp=info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,egui=info,eframe=info,egui_wgpu=info,winit=info,calloop=info,sctk=info";

/// Console log at `level` (or `RUST_LOG`); with `debug_log`, also a full debug log appended to that
/// file (owner-only on Unix). The default name falls back to `fallback_dir` (the config folder) when
/// the current directory is not writable, as for an app started from Finder (`/`) or a Windows shortcut
/// (`Program Files`). A file that cannot be opened is reported and the app starts anyway.
fn init_logging(level: &str, debug_log: Option<&std::path::Path>, fallback_dir: Option<&std::path::Path>) {
    let console = tracing_subscriber::fmt::layer()
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)));
    let file = debug_log.and_then(|path| {
        let fallback = fallback_dir.filter(|_| path == std::path::Path::new(DEBUG_LOG)).map(|d| d.join(DEBUG_LOG));
        let open = |p: &std::path::Path| open_debug_log(p).map(|f| (p.to_path_buf(), f));
        match open(path).or_else(|e| fallback.as_deref().map_or(Err(e), open)) {
            Ok(opened) => Some(opened),
            Err(e) => {
                eprintln!("rustxterm: debug log {}: {e}", path.display());
                None
            }
        }
    });
    let (path, file_layer) = match file {
        Some((path, f)) => {
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_thread_ids(true)
                .with_target(true)
                .with_writer(std::sync::Mutex::new(f))
                .with_filter(EnvFilter::new(DEBUG_FILTER));
            (Some(path), Some(layer))
        }
        None => (None, None),
    };
    tracing_subscriber::registry().with(console).with(file_layer).init();
    if let Some(path) = path {
        let shown = std::path::absolute(&path).unwrap_or(path);
        eprintln!("rustxterm: writing the debug log to {}", shown.display());
        tracing::info!(
            version = env!("CARGO_PKG_VERSION"),
            os = std::env::consts::OS,
            "debug log started: {}",
            shown.display()
        );
    }
}

fn open_debug_log(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        // A new file is created owner-only; `create_new` never follows a symlink.
        match std::fs::OpenOptions::new().append(true).create_new(true).mode(0o600).open(path) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            r => return r,
        }
        // An existing one must be a plain file (no symlink or hard link planted in a shared folder),
        // the same one that gets opened, and is then made owner-only.
        let refuse = |why: &str| std::io::Error::new(std::io::ErrorKind::PermissionDenied, why.to_owned());
        let before = std::fs::symlink_metadata(path)?;
        if !before.file_type().is_file() || before.nlink() != 1 {
            return Err(refuse("not a regular file"));
        }
        let file = std::fs::OpenOptions::new().append(true).open(path)?;
        let after = file.metadata()?;
        if (after.dev(), after.ino()) != (before.dev(), before.ino()) {
            return Err(refuse("the file changed while opening it"));
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }
    #[cfg(not(unix))]
    std::fs::OpenOptions::new().create(true).append(true).open(path)
}

const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/rustxterm-256.png");

/// Window icon (X11 `_NET_WM_ICON`; Wayland uses the app id and the installed .desktop file).
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).unwrap_or_default()
}

fn run(loaded: LoadOutcome, config: PathBuf, safe_gpu: bool) -> anyhow::Result<()> {
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
        run_window(options, config_path, loaded, platform, vault, known_hosts, gpu_start)
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

fn run_window(
    options: eframe::NativeOptions,
    config_path: PathBuf,
    loaded: LoadOutcome,
    platform: rxt_platform::Platform,
    vault: Arc<rxt_ui::Vault>,
    known_hosts: PathBuf,
    gpu_start: rxt_ui::GpuStart,
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
                ..EngineConfig::default()
            };
            let engine = Engine::start(config, waker)?;
            let app =
                rxt_ui::App::new(cc.storage, platform, engine).with_settings(loaded, config_path).with_vault(vault);
            let app = app.with_gpu_start(gpu_start);
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

#[cfg(all(test, unix))]
mod debug_log_tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn debug_log_is_private_and_refuses_links() {
        let dir = std::env::temp_dir().join(format!("rxt-debug-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("rustxterm-debug.log");
        drop(super::open_debug_log(&log).unwrap());
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(super::open_debug_log(&log).unwrap());
        assert_eq!(std::fs::metadata(&log).unwrap().permissions().mode() & 0o777, 0o600);

        let target = dir.join("target");
        std::fs::write(&target, b"").unwrap();
        let link = dir.join("link.log");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(super::open_debug_log(&link).is_err());
        let hard = dir.join("hard.log");
        std::fs::hard_link(&target, &hard).unwrap();
        assert!(super::open_debug_log(&hard).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
