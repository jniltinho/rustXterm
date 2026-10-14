//! Developer tasks: `layers`, `guard`, `lab`, `dist`, `windows-receipt`, `prints`, `scenarios`.

mod dist;
mod guard;
mod lab;
mod layers;
mod matrix;
mod msi_normalize;
mod prints;
mod receipt;

const USAGE: &str = "usage: cargo xtask <layers | guard [--ci] [--artifact PATH]... | dist-check DIR [--reproduce DIR] | windows-receipt --check DIR | msi-normalize MSI UUID EPOCH | lab ... | prints | scenarios>";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("layers") => layers::run(),
        Some("dist-check") => dist::run(rest),
        Some("guard") => guard::run(rest),
        Some("lab") => lab::run(rest),
        Some("prints") => prints::run(),
        Some("scenarios") => matrix::run(),
        Some("windows-receipt") => receipt::run(rest),
        Some("msi-normalize") => msi_normalize::run(rest),
        _ => Err(anyhow::anyhow!(USAGE)),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
