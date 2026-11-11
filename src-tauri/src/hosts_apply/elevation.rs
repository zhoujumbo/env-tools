//! Privileged write to the system hosts file.
//!
//! Windows strategy copied from SwitchHosts v5:
//! 1. Write the new content to a temp file (no privilege required).
//! 2. Relaunch our own binary via ShellExecuteExW + `runas`.
//! 3. Elevated child overwrites the resolved Windows hosts path, then exits
//!    before Tauri / storage starts.
//!
//! Do NOT go through `cmd /c copy` — cmd expands %VAR% unconditionally.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub(crate) const ELEVATION_HELPER_ARG: &str = crate::paths::ELEVATION_HELPER_ARG;

#[derive(Debug, Error)]
pub enum HostsApplyError {
    #[error("cancelled")]
    Cancelled,
    #[error("no access: {message}")]
    #[allow(dead_code)]
    NoAccess { message: String },
    #[error("{message}")]
    Io { message: String },
}

impl HostsApplyError {
    #[allow(dead_code)]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, HostsApplyError::Cancelled)
    }
}

/// Write `content` to `target` using OS-native elevation.
pub fn write_with_elevation(target: &Path, content: &str) -> Result<(), HostsApplyError> {
    let tmp_path = stage_temp_file(content)?;
    let result = elevate_copy(&tmp_path, target);
    let _ = std::fs::remove_file(&tmp_path);
    result
}

fn stage_temp_file(content: &str) -> Result<PathBuf, HostsApplyError> {
    let mut path = std::env::temp_dir();
    let stamp = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() * 1_000_000);
    path.push(format!("swh_apply_{stamp}.hosts"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| HostsApplyError::Io {
            message: format!("staging temp file failed: {e}"),
        })?;
    file.write_all(content.as_bytes()).map_err(|e| HostsApplyError::Io {
        message: format!("staging temp file failed: {e}"),
    })?;
    Ok(path)
}

fn is_allowed_windows_elevation_target_for(dst: &Path, expected: &Path) -> bool {
    dst == expected
}

fn is_allowed_windows_elevation_target(dst: &Path) -> Result<bool, HostsApplyError> {
    Ok(is_allowed_windows_elevation_target_for(
        dst,
        &super::write::windows_system_hosts_path()?,
    ))
}

#[cfg(windows)]
fn elevate_copy(src: &Path, dst: &Path) -> Result<(), HostsApplyError> {
    use std::ffi::{OsStr, OsString};
    use std::iter;
    use std::mem;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_CANCELLED, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
        SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn to_wide(os: &OsStr) -> Vec<u16> {
        os.encode_wide().chain(iter::once(0)).collect()
    }

    if !is_allowed_windows_elevation_target(dst)? {
        return Err(HostsApplyError::Io {
            message: format!(
                "refusing to elevate write to non-hosts target: {}",
                dst.display()
            ),
        });
    }

    let exe = std::env::current_exe().map_err(|e| HostsApplyError::Io {
        message: format!("current_exe failed: {e}"),
    })?;

    let mut params = OsString::new();
    params.push(ELEVATION_HELPER_ARG);
    params.push(" \"");
    params.push(src.as_os_str());
    params.push("\"");

    let verb = to_wide(OsStr::new("runas"));
    let file = to_wide(exe.as_os_str());
    let params_w = to_wide(&params);

    let src_display = src.to_string_lossy().into_owned();
    let dst_display = dst.to_string_lossy().into_owned();

    let outcome = std::thread::spawn(move || unsafe {
        let hr = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let we_inited = hr >= 0;

        let mut info: SHELLEXECUTEINFOW = mem::zeroed();
        info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params_w.as_ptr();
        info.nShow = SW_HIDE;

        let success = ShellExecuteExW(&mut info);
        if success == 0 {
            let last_err = GetLastError();
            if we_inited {
                CoUninitialize();
            }
            if last_err == ERROR_CANCELLED {
                return Err(HostsApplyError::Cancelled);
            }
            return Err(HostsApplyError::Io {
                message: format!(
                    "ShellExecuteExW failed: GetLastError={last_err}; src={src_display}, dst={dst_display}"
                ),
            });
        }

        let process = info.hProcess;
        if process.is_null() {
            if we_inited {
                CoUninitialize();
            }
            return Err(HostsApplyError::Io {
                message: "ShellExecuteExW returned a null process handle".to_string(),
            });
        }

        let wait = WaitForSingleObject(process, INFINITE);
        if wait != WAIT_OBJECT_0 {
            CloseHandle(process);
            if we_inited {
                CoUninitialize();
            }
            return Err(HostsApplyError::Io {
                message: format!("WaitForSingleObject returned {wait}"),
            });
        }

        let mut exit_code: u32 = 0;
        let got_exit = GetExitCodeProcess(process, &mut exit_code as *mut u32);
        CloseHandle(process);
        if we_inited {
            CoUninitialize();
        }
        if got_exit == 0 {
            return Err(HostsApplyError::Io {
                message: "GetExitCodeProcess failed".to_string(),
            });
        }
        if exit_code == 0 {
            Ok(())
        } else {
            Err(HostsApplyError::Io {
                message: format!(
                    "elevated write failed: helper exit code {exit_code}; src={src_display}, dst={dst_display}"
                ),
            })
        }
    })
    .join();

    match outcome {
        Ok(result) => result,
        Err(_panic) => Err(HostsApplyError::Io {
            message: "elevation worker thread panicked".to_string(),
        }),
    }
}

#[cfg(not(windows))]
fn elevate_copy(_src: &Path, _dst: &Path) -> Result<(), HostsApplyError> {
    Err(HostsApplyError::Io {
        message: "hosts elevation is Windows-only".into(),
    })
}

#[derive(Debug)]
enum ElevationHelperInvocation {
    NotHelper,
    InvalidArgs,
    Copy { src: PathBuf },
}

fn parse_elevation_helper_args<I, S>(args: I) -> ElevationHelperInvocation
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    if args.get(1).map(String::as_str) != Some(ELEVATION_HELPER_ARG) {
        return ElevationHelperInvocation::NotHelper;
    }
    if args.len() != 3 {
        return ElevationHelperInvocation::InvalidArgs;
    }
    ElevationHelperInvocation::Copy {
        src: PathBuf::from(&args[2]),
    }
}

fn is_regular_temp_file(src: &Path) -> bool {
    let Ok(tmp) = std::fs::canonicalize(std::env::temp_dir()) else {
        return false;
    };
    let Ok(meta) = std::fs::symlink_metadata(src) else {
        return false;
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return false;
    }
    let Ok(src) = std::fs::canonicalize(src) else {
        return false;
    };
    src.starts_with(&tmp)
}

fn overwrite_file_contents(src: &Path, dst: &Path) -> io::Result<u64> {
    let mut input = std::fs::File::open(src)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)?;
    std::io::copy(&mut input, &mut output)
}

/// Early argv check. If this process was spawned as the UAC helper, write
/// hosts and exit before Tauri starts.
pub fn maybe_run_as_elevation_helper() -> bool {
    match parse_elevation_helper_args(std::env::args()) {
        ElevationHelperInvocation::NotHelper => false,
        ElevationHelperInvocation::InvalidArgs => {
            eprintln!("[elevation-helper] invalid arguments");
            std::process::exit(1);
        }
        ElevationHelperInvocation::Copy { src } => {
            if !is_regular_temp_file(&src) {
                eprintln!(
                    "[elevation-helper] refusing src outside temp or non-regular: {}",
                    src.display()
                );
                std::process::exit(1);
            }
            let dst = match super::write::system_hosts_path() {
                Ok(path) => path,
                Err(e) => {
                    eprintln!("[elevation-helper] failed to resolve hosts path: {e}");
                    std::process::exit(1);
                }
            };
            let exit_code = match overwrite_file_contents(&src, &dst) {
                Ok(_) => 0,
                Err(e) => {
                    eprintln!(
                        "[elevation-helper] write {} -> {} failed: {e}",
                        src.display(),
                        dst.display()
                    );
                    1
                }
            };
            std::process::exit(exit_code);
        }
    }
}
