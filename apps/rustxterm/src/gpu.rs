//! Crash-safe graphics start (PhotoCraft-style). A driver that crashes while the window's wgpu
//! device is created kills the process where no Rust error handling runs, so every launch would
//! die the same way. Before the window exists the app writes a marker (`gpu-starting` in the
//! config folder: the backend being tried) and holds an OS lock on it; it removes the marker once
//! the first frames painted. A later launch that finds a marker nobody holds knows that start
//! died, and tries the next safer backend:
//!
//! - Windows: OpenGL → DirectX 12 → Vulkan → software (WARP)
//! - Linux: Vulkan → OpenGL → software (llvmpipe/lavapipe)
//! - macOS: Metal (wgpu) → OpenGL (eframe's glow renderer) → Apple's software GL. wgpu only
//!   exposes Metal on macOS, so the non-Metal path switches renderer (not just wgpu backend).
//!
//! A hardware fallback that worked is saved as the `gpu_backend` setting (Settings › Display,
//! where it can be reset); the software path is never saved silently. `WGPU_BACKEND` overrides
//! everything and `--safe-gpu` forces the software path for one launch.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup};
use eframe::wgpu::{self, Backends, DeviceType};

/// Marker file name in the config folder.
pub const MARKER: &str = "gpu-starting";

/// Rendering backend; stored lowercase in the `gpu_backend` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Platform default: hardware OpenGL first on Windows, wgpu's choice elsewhere.
    Auto,
    Gl,
    Dx12,
    Vulkan,
    /// Software rendering (WARP on Windows, llvmpipe/lavapipe elsewhere).
    Cpu,
}

impl Backend {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "gl" | "opengl" => Self::Gl,
            "dx12" => Self::Dx12,
            "vulkan" => Self::Vulkan,
            "cpu" | "software" => Self::Cpu,
            _ => Self::Auto,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gl => "gl",
            Self::Dx12 => "dx12",
            Self::Vulkan => "vulkan",
            Self::Cpu => "cpu",
        }
    }
}

/// Host OS: the ladder and the wgpu/renderer choice differ per platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Macos,
}

impl Os {
    /// This build's OS.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

/// Backends from the preferred one to the safest, per platform.
///
/// On macOS wgpu only exposes Metal, so the ladder is Metal (`Auto`) → the glow (OpenGL) renderer
/// (`Gl`) → Apple's software GL (`Cpu`); `main.rs` turns `Gl`/`Cpu` into `Renderer::Glow`.
pub fn ladder(os: Os) -> &'static [Backend] {
    match os {
        Os::Windows => &[Backend::Gl, Backend::Dx12, Backend::Vulkan, Backend::Cpu],
        Os::Linux => &[Backend::Vulkan, Backend::Gl, Backend::Cpu],
        Os::Macos => &[Backend::Auto, Backend::Gl, Backend::Cpu],
    }
}

/// The backend `Auto` stands for on the ladder (where a crash moves on from).
fn effective(b: Backend, os: Os) -> Backend {
    if b == Backend::Auto { ladder(os)[0] } else { b }
}

/// The next safer backend after a start on `tried` died.
pub fn next(tried: Backend, os: Os) -> Backend {
    let l = ladder(os);
    let tried = effective(tried, os);
    l.iter().position(|b| *b == tried).and_then(|i| l.get(i + 1)).copied().unwrap_or(Backend::Cpu)
}

/// This launch's backend and why.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub backend: Backend,
    /// The backend whose start died (a fallback launch).
    pub crashed: Option<Backend>,
}

impl Plan {
    /// Setting to store once this launch painted: a working hardware fallback only.
    pub fn to_save(&self) -> Option<Backend> {
        self.crashed.filter(|_| self.backend != Backend::Cpu).map(|_| self.backend)
    }

    /// Status line for a fallback launch.
    pub fn notice(&self) -> Option<String> {
        let crashed = self.crashed?;
        Some(format!(
            "The graphics driver stopped the last start ({}); now using {}. Change it in Settings › Display.",
            crashed.name(),
            self.backend.name()
        ))
    }
}

/// Decide from the setting, a marker left by a start that died, and `--safe-gpu`.
pub fn plan(preference: Backend, died: Option<Backend>, safe: bool, os: Os) -> Plan {
    if safe {
        return Plan { backend: Backend::Cpu, crashed: None };
    }
    match died {
        Some(tried) => Plan { backend: next(tried, os), crashed: Some(tried) },
        None => Plan { backend: preference, crashed: None },
    }
}

/// The marker of this start, locked until dropped or [`Marker::clear`]ed.
pub struct Marker {
    path: PathBuf,
    file: Option<File>,
}

impl Marker {
    /// Backend of a start that died: a marker no running process holds.
    pub fn died(dir: &Path) -> Option<Backend> {
        let path = dir.join(MARKER);
        let file = OpenOptions::new().read(true).write(true).open(&path).ok()?;
        // Held: another instance is starting right now; not a crash.
        file.try_lock().ok()?;
        let mut text = String::new();
        file.take(64).read_to_string(&mut text).ok()?;
        Some(Backend::parse(&text))
    }

    /// Write and lock the marker for `backend`. Errors only lose the crash protection.
    pub fn start(dir: &Path, backend: Backend) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(MARKER);
        let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(&path)?;
        file.lock()?;
        file.write_all(backend.name().as_bytes())?;
        file.sync_all()?;
        Ok(Self { path, file: Some(file) })
    }

    /// The start painted: remove the marker.
    pub fn clear(&mut self) {
        if self.file.take().is_some() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// wgpu setup for `backend`; `None` keeps eframe's default (`WGPU_BACKEND` set, or `Auto` off
/// Windows).
pub fn configuration(backend: Backend, os: Os) -> Option<WgpuConfiguration> {
    // macOS renders through Metal (`Auto`) or the glow renderer (`Gl`/`Cpu`, chosen in `main.rs`);
    // wgpu is never reconfigured there, so keep eframe's defaults.
    if os == Os::Macos {
        return None;
    }
    let windows = os == Os::Windows;
    if std::env::var_os("WGPU_BACKEND").is_some() || (backend == Backend::Auto && !windows) {
        return None;
    }
    let backends = match backend {
        Backend::Auto => Backends::all(),
        Backend::Gl => Backends::GL,
        Backend::Dx12 => Backends::DX12,
        Backend::Vulkan => Backends::VULKAN,
        Backend::Cpu if windows => Backends::DX12,
        Backend::Cpu => Backends::GL | Backends::VULKAN,
    };
    let mut config = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        setup.instance_descriptor.backends = backends;
        let software = backend == Backend::Cpu;
        setup.native_adapter_selector = Some(Arc::new(move |adapters, surface| {
            let usable: Vec<&wgpu::Adapter> =
                adapters.iter().filter(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
            let infos: Vec<_> = usable.iter().map(|a| a.get_info()).collect();
            let list: Vec<_> = infos.iter().map(|i| (i.backend, i.device_type, i.name.as_str())).collect();
            pick_adapter(&list, software)
                .and_then(|i| usable.get(i))
                .map(|a| (*a).clone())
                .ok_or_else(|| "no usable wgpu adapter".to_owned())
        }));
    }
    Some(config)
}

/// Index of the adapter to render on. Normal: a hardware OpenGL one first (DX12 presentation
/// flickers on Intel graphics), then by device type as wgpu ranks them for high performance.
/// Software: a CPU adapter (WARP's output-owning entry first), else the first usable one (on
/// NVIDIA's EGL llvmpipe is hidden, so the window still opens).
pub fn pick_adapter(adapters: &[(wgpu::Backend, DeviceType, &str)], software: bool) -> Option<usize> {
    let warp = |name: &str| name.contains("Microsoft Basic Render Driver");
    let rank = |&(backend, kind, name): &(wgpu::Backend, DeviceType, &str)| {
        let device = match kind {
            DeviceType::DiscreteGpu => 0,
            DeviceType::IntegratedGpu => 1,
            DeviceType::VirtualGpu => 2,
            DeviceType::Other => 3,
            DeviceType::Cpu => 4,
        };
        if software {
            let cpu = kind == DeviceType::Cpu || warp(name);
            // WARP is listed twice on GPU-less machines; the non-CPU entry owns the output.
            return (!cpu, !(warp(name) && kind != DeviceType::Cpu), device);
        }
        (false, !(backend == wgpu::Backend::Gl && kind != DeviceType::Cpu), device)
    };
    (0..adapters.len()).min_by_key(|&i| rank(&adapters[i]))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use wgpu::Backend as W;

    #[test]
    fn ladders_and_next() {
        assert_eq!(next(Backend::Auto, Os::Windows), Backend::Dx12, "auto starts at GL on Windows");
        assert_eq!(next(Backend::Gl, Os::Windows), Backend::Dx12);
        assert_eq!(next(Backend::Dx12, Os::Windows), Backend::Vulkan);
        assert_eq!(next(Backend::Vulkan, Os::Windows), Backend::Cpu);
        assert_eq!(next(Backend::Cpu, Os::Windows), Backend::Cpu);
        assert_eq!(next(Backend::Auto, Os::Linux), Backend::Gl);
        assert_eq!(next(Backend::Gl, Os::Linux), Backend::Cpu);
        assert_eq!(next(Backend::Dx12, Os::Linux), Backend::Cpu, "not on the Linux ladder");
        // macOS: Metal (Auto) → glow (Gl) → software GL (Cpu).
        assert_eq!(next(Backend::Auto, Os::Macos), Backend::Gl, "Metal falls back to the glow renderer");
        assert_eq!(next(Backend::Gl, Os::Macos), Backend::Cpu);
        assert_eq!(next(Backend::Cpu, Os::Macos), Backend::Cpu);
        assert_eq!(next(Backend::Vulkan, Os::Macos), Backend::Cpu, "not on the macOS ladder");
    }

    #[test]
    fn plan_and_what_is_saved() {
        let p = plan(Backend::Auto, None, false, Os::Windows);
        assert_eq!((p.backend, p.to_save(), p.notice()), (Backend::Auto, None, None));
        let p = plan(Backend::Auto, Some(Backend::Gl), false, Os::Windows);
        assert_eq!((p.backend, p.to_save()), (Backend::Dx12, Some(Backend::Dx12)));
        assert!(p.notice().unwrap().contains("(gl); now using dx12"));
        let p = plan(Backend::Dx12, Some(Backend::Vulkan), false, Os::Windows);
        assert_eq!((p.backend, p.to_save()), (Backend::Cpu, None), "software is never saved silently");
        assert_eq!(
            plan(Backend::Gl, Some(Backend::Gl), true, Os::Windows),
            Plan { backend: Backend::Cpu, crashed: None }
        );
        // macOS: a Metal start that died promotes glow, and that working fallback is saved.
        let p = plan(Backend::Auto, Some(Backend::Auto), false, Os::Macos);
        assert_eq!((p.backend, p.to_save()), (Backend::Gl, Some(Backend::Gl)));
        assert_eq!(Backend::parse(" OpenGL\n"), Backend::Gl);
        assert_eq!(Backend::parse("weird"), Backend::Auto);
    }

    #[test]
    fn marker_held_is_not_a_crash_and_left_marker_is() {
        let dir = std::env::temp_dir().join(format!("rxt-gpu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Marker::died(&dir), None);
        let mut m = Marker::start(&dir, Backend::Vulkan).unwrap();
        assert_eq!(Marker::died(&dir), None, "held by a starting instance");
        // A start that died leaves the file but no lock.
        drop(m.file.take());
        assert_eq!(Marker::died(&dir), Some(Backend::Vulkan));
        let mut ok = Marker::start(&dir, Backend::Gl).unwrap();
        ok.clear();
        assert_eq!(Marker::died(&dir), None);
        assert!(!dir.join(MARKER).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adapter_choice() {
        let intel = [(W::Dx12, DeviceType::IntegratedGpu, "Intel"), (W::Gl, DeviceType::IntegratedGpu, "Intel")];
        assert_eq!(pick_adapter(&intel, false), Some(1), "hardware OpenGL first");
        let vm = [
            (W::Dx12, DeviceType::Cpu, "Microsoft Basic Render Driver"),
            (W::Dx12, DeviceType::Other, "Microsoft Basic Render Driver"),
            (W::Gl, DeviceType::Cpu, "llvmpipe"),
        ];
        assert_eq!(pick_adapter(&vm, false), Some(1));
        assert_eq!(pick_adapter(&vm, true), Some(1), "output-owning WARP");
        let linux = [(W::Vulkan, DeviceType::DiscreteGpu, "RTX"), (W::Gl, DeviceType::Cpu, "llvmpipe")];
        assert_eq!(pick_adapter(&linux, true), Some(1));
        let nvidia_egl = [(W::Vulkan, DeviceType::DiscreteGpu, "RTX")];
        assert_eq!(pick_adapter(&nvidia_egl, true), Some(0), "no software adapter: still opens");
        assert_eq!(pick_adapter(&[], false), None);
    }
}
