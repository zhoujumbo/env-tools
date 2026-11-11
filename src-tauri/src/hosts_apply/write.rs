//! Write aggregated hosts content to the Windows system hosts file.
//! Elevation path is a direct port of SwitchHosts v5 Windows behavior.

use super::elevation::{write_with_elevation, HostsApplyError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyOutcome {
    pub previous_content: String,
    pub new_content: String,
    pub unchanged: bool,
}

pub fn windows_system_hosts_path() -> Result<PathBuf, HostsApplyError> {
    let windir = std::env::var("SystemRoot")
        .or_else(|_| std::env::var("WINDIR"))
        .unwrap_or_else(|_| r"C:\Windows".into());
    Ok(windows_hosts_path_from_windows_dir(Path::new(&windir)))
}

pub fn windows_hosts_path_from_windows_dir(windows_dir: &Path) -> PathBuf {
    windows_dir
        .join("System32")
        .join("drivers")
        .join("etc")
        .join("hosts")
}

pub fn system_hosts_path() -> Result<PathBuf, HostsApplyError> {
    windows_system_hosts_path()
}

fn normalize_line_endings(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn to_crlf(s: &str) -> String {
    let lf = normalize_line_endings(s);
    lf.replace('\n', "\r\n")
}

fn is_permission_denied(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
        || e.raw_os_error() == Some(5)
        || e.raw_os_error() == Some(32)
}

pub fn read_system_hosts() -> Result<String, HostsApplyError> {
    let target = windows_system_hosts_path()?;
    let raw = fs::read_to_string(&target).map_err(|e| HostsApplyError::Io {
        message: format!("read {}: {e}", target.display()),
    })?;
    Ok(normalize_line_endings(&raw))
}

pub fn apply_to_system_hosts(content: &str) -> Result<ApplyOutcome, HostsApplyError> {
    let target = windows_system_hosts_path()?;
    let previous_raw = fs::read_to_string(&target).unwrap_or_default();
    let previous_lf = normalize_line_endings(&previous_raw);
    let final_content_lf = normalize_line_endings(content);
    if previous_lf == final_content_lf {
        return Ok(ApplyOutcome {
            previous_content: previous_lf,
            new_content: final_content_lf,
            unchanged: true,
        });
    }
    let disk_content = to_crlf(&final_content_lf);
    match fs::write(&target, disk_content.as_bytes()) {
        Ok(()) => Ok(ApplyOutcome {
            previous_content: previous_lf,
            new_content: final_content_lf,
            unchanged: false,
        }),
        Err(e) if is_permission_denied(&e) => {
            write_with_elevation(&target, &disk_content)?;
            Ok(ApplyOutcome {
                previous_content: previous_lf,
                new_content: final_content_lf,
                unchanged: false,
            })
        }
        Err(e) => Err(HostsApplyError::Io {
            message: format!("write {}: {e}", target.display()),
        }),
    }
}

pub fn can_read_hosts() -> (bool, String) {
    match windows_system_hosts_path() {
        Ok(p) => match fs::read_to_string(&p) {
            Ok(_) => (true, p.display().to_string()),
            Err(e) => (false, format!("{}: {e}", p.display())),
        },
        Err(e) => (false, e.to_string()),
    }
}

pub fn can_write_hosts() -> (bool, String) {
    match windows_system_hosts_path() {
        Ok(p) => {
            let writable = fs::OpenOptions::new().write(true).append(true).open(&p).is_ok();
            (writable, p.display().to_string())
        }
        Err(e) => (false, e.to_string()),
    }
}
