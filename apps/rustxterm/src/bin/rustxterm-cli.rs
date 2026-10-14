//! `rustxterm-cli.exe`: the Windows console executable (CLI subcommands, `mcp`, `--self-test`),
//! built from the same code as the GUI-subsystem `rustxterm.exe` (add-windows-port design W5).
//! On other targets `rustxterm` itself is the console program, so this binary only points there.

#[cfg(windows)]
#[path = "../main.rs"]
mod app;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    app::main()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("rustxterm-cli is the Windows console executable; use `rustxterm`");
    std::process::ExitCode::FAILURE
}
