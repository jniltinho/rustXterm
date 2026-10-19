//! Diagnostic log levels (add-diagnostic-logging D1): the tracing filter of the diagnostic log file
//! for each level. Every filter keeps the NTLM/CredSSP crates at `info`: their debug output holds
//! plaintext CredSSP credentials (`sspi` `write_password_credentials`) or NTLMv2 responses
//! (`ironrdp_connector::credssp`).

use serde::{Deserialize, Serialize};

/// Level of the diagnostic log file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DiagLevel {
    /// No file.
    #[default]
    Off,
    Errors,
    Normal,
    FullDebug,
    /// Full debug plus `trace` for the chosen protocols.
    ProtocolTrace,
}

impl DiagLevel {
    pub const ALL: [Self; 5] = [Self::Off, Self::Errors, Self::Normal, Self::FullDebug, Self::ProtocolTrace];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Errors => "Errors",
            Self::Normal => "Normal",
            Self::FullDebug => "Full debug",
            Self::ProtocolTrace => "Protocol trace",
        }
    }
}

/// Diagnostics settings saved in `config.toml` (`[diagnostics]`). Stream capture is not saved: it
/// starts off every time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DiagSettings {
    pub level: DiagLevel,
    /// [`TRACE_PROTOCOLS`] names traced by [`DiagLevel::ProtocolTrace`].
    pub protocols: Vec<String>,
    /// Folder holding `rustxterm-logs/`; `None` is the folder rustXterm runs from.
    pub folder: Option<std::path::PathBuf>,
}

/// Directives every diagnostic filter ends with (secrets, see the module doc).
pub const SECRET_DIRECTIVES: &str = "sspi=info,picky=info,picky_krb=info,ironrdp_connector::credssp=info";

/// The graphics stack, whose per-frame events would bury everything else.
const QUIET_DIRECTIVES: &str = "wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,egui=info,eframe=info,egui_wgpu=info,\
                                winit=info,calloop=info,sctk=info";

/// Protocols offered for "Protocol trace": (name, label, tracing targets).
pub const TRACE_PROTOCOLS: [(&str, &str, &[&str]); 6] = [
    ("ssh", "SSH / SFTP", &["rxt_ssh", "rxt_sftp", "russh"]),
    ("rdp", "RDP", &["rxt_rdp", "ironrdp"]),
    ("vnc", "VNC", &["rxt_vnc", "vnc"]),
    ("ftp", "FTP / FTPS", &["rxt_ftp", "suppaftp"]),
    ("telnet", "Telnet / rlogin / rsh", &["rxt_telnet"]),
    ("serial", "Serial", &["rxt_serial"]),
];

/// Tracing filter of the diagnostic log for `level`; `None` for [`DiagLevel::Off`]. `protocols` are
/// [`TRACE_PROTOCOLS`] names, used by [`DiagLevel::ProtocolTrace`].
pub fn filter(level: DiagLevel, protocols: &[String]) -> Option<String> {
    let base = match level {
        DiagLevel::Off => return None,
        DiagLevel::Errors => "warn".to_owned(),
        DiagLevel::Normal => "info".to_owned(),
        DiagLevel::FullDebug => format!("debug,{QUIET_DIRECTIVES}"),
        DiagLevel::ProtocolTrace => {
            let mut f = format!("debug,{QUIET_DIRECTIVES}");
            for (_, _, targets) in TRACE_PROTOCOLS.iter().filter(|(n, _, _)| protocols.iter().any(|p| p == n)) {
                for t in *targets {
                    f.push_str(&format!(",{t}=trace"));
                }
            }
            f
        }
    };
    Some(format!("{base},{SECRET_DIRECTIVES}"))
}

/// The `--debug-log` filter (full debug).
pub fn debug_filter() -> String {
    filter(DiagLevel::FullDebug, &[]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_keeps_the_secret_directives() {
        let all: Vec<String> = TRACE_PROTOCOLS.iter().map(|(n, _, _)| (*n).to_owned()).collect();
        assert_eq!(filter(DiagLevel::Off, &all), None);
        for level in DiagLevel::ALL.into_iter().filter(|l| *l != DiagLevel::Off) {
            let f = filter(level, &all).unwrap();
            for d in SECRET_DIRECTIVES.split(',') {
                assert!(f.split(',').any(|x| x == d), "{level:?}: {d} missing from {f}");
            }
        }
    }

    #[test]
    fn protocol_trace_traces_only_the_chosen_protocols() {
        let f = filter(DiagLevel::ProtocolTrace, &["rdp".to_owned()]).unwrap();
        assert!(f.contains("rxt_rdp=trace") && f.contains("ironrdp=trace"));
        assert!(!f.contains("rxt_ssh=trace"));
        assert!(f.starts_with("debug,"));
    }
}
