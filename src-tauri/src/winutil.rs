use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use winreg::enums::*;
use winreg::RegKey;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const USER_ENV: &str = r"Environment";
const SYS_ENV: &str =
    r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
const CMD_PROCESSOR: &str = r"Software\Microsoft\Command Processor";

pub fn user_env_key() -> Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(USER_ENV, KEY_READ | KEY_WRITE)
        .or_else(|_| hkcu.create_subkey(USER_ENV).map(|(k, _)| k))
        .context("open HKCU\\Environment")
}

pub fn user_env_key_ro() -> Result<RegKey> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(USER_ENV)
        .context("open HKCU\\Environment readonly")
}

pub fn system_env_key_ro() -> Result<RegKey> {
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(SYS_ENV)
        .context("open system Environment readonly")
}

pub fn read_env_map(system: bool) -> Result<HashMap<String, String>> {
    let key = if system {
        system_env_key_ro()?
    } else {
        user_env_key_ro().or_else(|_| user_env_key())?
    };
    let mut map = HashMap::new();
    for (name, _) in key.enum_values().filter_map(|x| x.ok()) {
        if let Ok(v) = key.get_value::<String, _>(&name) {
            map.insert(name, v);
        }
    }
    Ok(map)
}

pub fn set_user_env(name: &str, value: &str) -> Result<()> {
    let key = user_env_key()?;
    if name.eq_ignore_ascii_case("PATH") {
        key.set_value(name, &OsString::from(value))?;
    } else {
        key.set_value(name, &value.to_string())?;
    }
    Ok(())
}

pub fn delete_user_env(name: &str) -> Result<()> {
    let key = user_env_key()?;
    match key.delete_value(name) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub fn get_user_env(name: &str) -> Result<Option<String>> {
    let key = match user_env_key_ro() {
        Ok(k) => k,
        Err(_) => return Ok(None),
    };
    match key.get_value::<String, _>(name) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn get_system_env(name: &str) -> Result<Option<String>> {
    let key = match system_env_key_ro() {
        Ok(k) => k,
        Err(_) => return Ok(None),
    };
    match key.get_value::<String, _>(name) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn system_env_key_rw() -> Result<RegKey> {
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(SYS_ENV, KEY_READ | KEY_WRITE)
        .context("open system Environment read/write（需要管理员）")
}

pub fn set_system_env(name: &str, value: &str) -> Result<()> {
    let key = system_env_key_rw()?;
    if name.eq_ignore_ascii_case("PATH") {
        key.set_value(name, &OsString::from(value))?;
    } else {
        key.set_value(name, &value.to_string())?;
    }
    Ok(())
}

pub fn delete_system_env(name: &str) -> Result<()> {
    let key = system_env_key_rw()?;
    match key.delete_value(name) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub fn strip_verbatim_prefix(s: &str) -> String {
    s.strip_prefix(r"\\?\")
        .unwrap_or(s)
        .trim_end_matches(['/', '\\'])
        .to_string()
}

pub fn path_eq(a: &str, b: &str) -> bool {
    strip_verbatim_prefix(a).eq_ignore_ascii_case(&strip_verbatim_prefix(b))
}

/// 从 PATH 字符串去掉若干条目（大小写不敏感，兼容 `\\?\`）。
pub fn path_without_entries(path: &str, remove: &[String]) -> String {
    path.split(';')
        .filter(|p| {
            !p.is_empty() && !remove.iter().any(|r| path_eq(p, r))
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysEnvTask {
    pub delete_machine_vars: Vec<String>,
    pub remove_machine_path: Vec<String>,
}

/// UAC 子进程入口：应用临时目录里的 SysEnvTask JSON。
pub fn maybe_run_as_sysenv_helper() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(crate::paths::ELEVATION_SYSENV_ARG) {
        return false;
    }
    if args.len() != 3 {
        eprintln!("[sysenv-helper] invalid arguments");
        std::process::exit(1);
    }
    let src = PathBuf::from(&args[2]);
    if !is_regular_temp_file(&src) {
        eprintln!(
            "[sysenv-helper] refusing src outside temp: {}",
            src.display()
        );
        std::process::exit(1);
    }
    let raw = match fs::read_to_string(&src) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[sysenv-helper] read task failed: {e}");
            std::process::exit(1);
        }
    };
    let task: SysEnvTask = match serde_json::from_str(&raw) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[sysenv-helper] bad json: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = apply_sysenv_task(&task) {
        eprintln!("[sysenv-helper] apply failed: {e}");
        std::process::exit(1);
    }
    broadcast_environment();
    std::process::exit(0);
}

fn is_regular_temp_file(src: &Path) -> bool {
    let Ok(tmp) = fs::canonicalize(std::env::temp_dir()) else {
        return false;
    };
    let Ok(meta) = fs::symlink_metadata(src) else {
        return false;
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return false;
    }
    let Ok(src) = fs::canonicalize(src) else {
        return false;
    };
    src.starts_with(&tmp)
}

fn apply_sysenv_task(task: &SysEnvTask) -> Result<()> {
    for name in &task.delete_machine_vars {
        let ok = name.eq_ignore_ascii_case("JAVA_HOME")
            || name.eq_ignore_ascii_case("GOROOT")
            || name.eq_ignore_ascii_case("JDK_HOME");
        if !ok {
            bail!("refusing to delete system var {name}");
        }
        delete_system_env(name)?;
    }
    if !task.remove_machine_path.is_empty() {
        let current = get_system_env("Path")?.unwrap_or_default();
        let next = path_without_entries(&current, &task.remove_machine_path);
        if next != current {
            set_system_env("Path", &next)?;
        }
    }
    Ok(())
}

/// 弹 UAC，以管理员身份执行 SysEnvTask。
pub fn elevate_sysenv_task(task: &SysEnvTask) -> Result<()> {
    #[cfg(windows)]
    {
        elevate_sysenv_task_windows(task)
    }
    #[cfg(not(windows))]
    {
        let _ = task;
        bail!("sysenv elevation is Windows-only");
    }
}

#[cfg(windows)]
fn elevate_sysenv_task_windows(task: &SysEnvTask) -> Result<()> {
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

    let mut path = std::env::temp_dir();
    let stamp = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() * 1_000_000);
    path.push(format!("det_sysenv_{stamp}.json"));
    fs::write(&path, serde_json::to_vec_pretty(task)?)?;
    let _cleanup = scopeguard_remove(path.clone());

    let exe = std::env::current_exe().context("current_exe")?;
    let mut params = OsString::new();
    params.push(crate::paths::ELEVATION_SYSENV_ARG);
    params.push(" \"");
    params.push(path.as_os_str());
    params.push("\"");

    let verb = to_wide(OsStr::new("runas"));
    let file = to_wide(exe.as_os_str());
    let params_w = to_wide(&params);
    let path_display = path.display().to_string();

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
                return Err(anyhow::anyhow!("cancelled"));
            }
            return Err(anyhow::anyhow!(
                "ShellExecuteExW failed: GetLastError={last_err}; task={path_display}"
            ));
        }
        let process = info.hProcess;
        if process.is_null() {
            if we_inited {
                CoUninitialize();
            }
            return Err(anyhow::anyhow!("null process handle"));
        }
        let wait = WaitForSingleObject(process, INFINITE);
        if wait != WAIT_OBJECT_0 {
            CloseHandle(process);
            if we_inited {
                CoUninitialize();
            }
            return Err(anyhow::anyhow!("WaitForSingleObject returned {wait}"));
        }
        let mut exit_code: u32 = 0;
        let got = GetExitCodeProcess(process, &mut exit_code as *mut u32);
        CloseHandle(process);
        if we_inited {
            CoUninitialize();
        }
        if got == 0 {
            return Err(anyhow::anyhow!("GetExitCodeProcess failed"));
        }
        if exit_code == 0 {
            Ok(())
        } else {
            Err(anyhow::anyhow!("helper exit code {exit_code}"))
        }
    })
    .join()
    .map_err(|_| anyhow::anyhow!("elevation worker panicked"))?;
    outcome
}

fn scopeguard_remove(path: PathBuf) -> impl Drop {
    struct Guard(PathBuf);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    Guard(path)
}

pub fn broadcast_environment() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        };
        let env: Vec<u16> = "Environment\0".encode_utf16().collect();
        let mut result: usize = 0;
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST as HWND,
            WM_SETTINGCHANGE,
            0,
            env.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            3000,
            &mut result,
        );
    }
}

pub fn cmd_autorun_get() -> Result<Option<String>> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = match hkcu.open_subkey(CMD_PROCESSOR) {
        Ok(k) => k,
        Err(_) => return Ok(None),
    };
    match key.get_value::<String, _>("AutoRun") {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn cmd_autorun_set(value: Option<&str>) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(CMD_PROCESSOR)?;
    match value {
        None | Some("") => {
            let _ = key.delete_value("AutoRun");
        }
        Some(v) => key.set_value("AutoRun", &v.to_string())?,
    }
    Ok(())
}

pub fn prepend_user_path(entry: &str) -> Result<String> {
    let current = get_user_env("Path")?
        .or_else(|| get_user_env("PATH").ok().flatten())
        .unwrap_or_default();
    let mut parts: Vec<&str> = current.split(';').filter(|s| !s.is_empty()).collect();
    parts.retain(|p| !p.eq_ignore_ascii_case(entry));
    let new_val = if parts.is_empty() {
        entry.to_string()
    } else {
        format!("{};{}", entry, parts.join(";"))
    };
    if new_val != current {
        set_user_env("Path", &new_val)?;
    }
    Ok(new_val)
}

/// 改进程内 PATH，让随后的 `where` / `cmd /C python` 立刻看到新顺序。
pub fn prepend_process_path(entry: &str) {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<&str> = current.split(';').filter(|s| !s.is_empty()).collect();
    parts.retain(|p| !p.eq_ignore_ascii_case(entry));
    let new_val = if parts.is_empty() {
        entry.to_string()
    } else {
        format!("{};{}", entry, parts.join(";"))
    };
    std::env::set_var("PATH", new_val);
}

pub fn remove_user_path_entry(entry: &str) -> Result<String> {
    let current = get_user_env("Path")?
        .or_else(|| get_user_env("PATH").ok().flatten())
        .unwrap_or_default();
    let parts: Vec<&str> = current
        .split(';')
        .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case(entry))
        .collect();
    let new_val = parts.join(";");
    set_user_env("Path", &new_val)?;
    Ok(new_val)
}

#[allow(dead_code)]
pub fn remove_process_path_entry(entry: &str) {
    let current = std::env::var("PATH").unwrap_or_default();
    let parts: Vec<&str> = current
        .split(';')
        .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case(entry))
        .collect();
    std::env::set_var("PATH", parts.join(";"));
}

pub fn set_user_path(value: &str) -> Result<()> {
    set_user_env("Path", value)
}

/// 新开 cmd 看到的 PATH：用户 Path 在前，系统 Path 在后（与 Explorer 拉起的进程一致）。
pub fn registered_path() -> String {
    let user = get_user_env("Path")
        .ok()
        .flatten()
        .or_else(|| get_user_env("PATH").ok().flatten())
        .unwrap_or_default();
    let machine = get_system_env("Path")
        .ok()
        .flatten()
        .or_else(|| get_system_env("PATH").ok().flatten())
        .unwrap_or_default();
    [user.as_str(), machine.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(";")
}

pub fn expand_env_in_path(s: &str) -> String {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<u16> = std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let needed = windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                0,
            );
            if needed == 0 {
                return s.to_string();
            }
            let mut buf = vec![0u16; needed as usize];
            let n = windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW(
                wide.as_ptr(),
                buf.as_mut_ptr(),
                needed,
            );
            if n == 0 {
                return s.to_string();
            }
            return std::ffi::OsString::from_wide(&buf[..n as usize - 1])
                .to_string_lossy()
                .into_owned();
        }
    }
    #[cfg(not(windows))]
    {
        s.to_string()
    }
}

/// 按注册表 PATH 解析命令，不看本进程被污染过的 PATH。
pub fn which_on_registered_path(name: &str) -> Option<PathBuf> {
    let pathext = get_user_env("PATHEXT")
        .ok()
        .flatten()
        .or_else(|| get_system_env("PATHEXT").ok().flatten())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.MSC".into());
    let exts: Vec<String> = pathext
        .split(';')
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();
    let has_ext = Path::new(name)
        .extension()
        .is_some_and(|e| !e.is_empty());
    for dir in registered_path().split(';').filter(|s| !s.is_empty()) {
        let dir = expand_env_in_path(dir);
        let base = PathBuf::from(&dir).join(name);
        if base.is_file() {
            return Some(base);
        }
        if has_ext {
            continue;
        }
        for ext in &exts {
            let p = PathBuf::from(&dir).join(format!("{name}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdResult {
    pub program: String,
    pub args: Vec<String>,
    pub cmdline: String,
    pub cwd: Option<String>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
}

/// 单条流最多保留的输出字节数。npm / pip 偶尔会吐几十 MB，超出部分直接丢弃，
/// 只留下开头，避免把整个输出读进内存。
const MAX_STREAM_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

fn capture_stream(mut reader: impl std::io::Read, max: usize) -> Captured {
    let mut out = Vec::with_capacity(8 * 1024);
    let mut truncated = false;
    let mut buf = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out.len() < max {
                    let take = (max - out.len()).min(n);
                    out.extend_from_slice(&buf[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    Captured {
        bytes: out,
        truncated,
    }
}

/// 必须在进程运行期间就持续读取管道：Windows 管道缓冲约 64KB，
/// 写满后子进程会阻塞在 write 上永远不退出（表现为莫名其妙地卡到 timeout）。
fn spawn_capture<R>(reader: Option<R>, max: usize) -> std::thread::JoinHandle<Captured>
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || match reader {
        Some(r) => capture_stream(r, max),
        None => Captured::default(),
    })
}

enum StopReason {
    Natural(std::process::ExitStatus),
    Cancelled,
    Timeout,
    WaitError(String),
}

fn spawn_failed(program: &str, args: &[&str], cmdline: String, cwd: Option<&Path>, e: &dyn std::fmt::Display) -> CmdResult {
    CmdResult {
        program: program.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cmdline,
        cwd: cwd.map(|p| p.display().to_string()),
        exit_code: -1,
        stdout: String::new(),
        stderr: format!("spawn failed: {e}"),
        ok: false,
    }
}

pub fn run_cmd(program: &str, args: &[&str], cwd: Option<&Path>) -> CmdResult {
    run_cmd_wait(program, args, cwd, None, &|| false)
}

pub fn run_cmd_timeout(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: Duration,
) -> CmdResult {
    run_cmd_wait(program, args, cwd, timeout, &|| false)
}

/// 轮询等待；`should_stop` 为真时 taskkill /T 杀掉进程树。
pub fn run_cmd_interruptible(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: Duration,
    should_stop: impl Fn() -> bool,
) -> CmdResult {
    run_cmd_wait(program, args, cwd, timeout, &should_stop)
}

fn run_cmd_wait(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: impl Into<Option<Duration>>,
    should_stop: &dyn Fn() -> bool,
) -> CmdResult {
    let timeout = timeout.into();
    let cmdline = format!("{} {}", program, args.join(" "));
    let mut cmd = Command::new(program);
    cmd.args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return spawn_failed(program, args, cmdline, cwd, &e),
    };

    let out_h = spawn_capture(child.stdout.take(), MAX_STREAM_BYTES);
    let err_h = spawn_capture(child.stderr.take(), MAX_STREAM_BYTES);
    let pid = child.id();
    let start = std::time::Instant::now();

    let reason = loop {
        match child.try_wait() {
            Ok(Some(status)) => break StopReason::Natural(status),
            Ok(None) => {
                if should_stop() {
                    kill_process_tree(pid);
                    let _ = child.kill();
                    break StopReason::Cancelled;
                }
                if timeout.is_some_and(|t| start.elapsed() > t) {
                    kill_process_tree(pid);
                    let _ = child.kill();
                    break StopReason::Timeout;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                break StopReason::WaitError(e.to_string());
            }
        }
    };

    // 进程已结束/被杀，读线程会读到 EOF；join 拿回已经收到的内容。
    let out = out_h.join().unwrap_or_default();
    let err = err_h.join().unwrap_or_default();
    let mut stdout = decode_output(&out.bytes);
    let mut stderr = decode_output(&err.bytes);
    if out.truncated {
        stdout.push_str("\n…(stdout 已截断)");
    }
    if err.truncated {
        stderr.push_str("\n…(stderr 已截断)");
    }

    let (exit_code, ok) = match reason {
        StopReason::Natural(status) => (
            status.code().unwrap_or(-1),
            status.success(),
        ),
        StopReason::Cancelled => {
            let tag = crate::version_mgr::CANCEL_MSG;
            if !stderr.contains(tag) {
                stderr = if stderr.is_empty() {
                    tag.to_string()
                } else {
                    format!("{stderr}\n{tag}")
                };
            }
            (-1, false)
        }
        StopReason::Timeout => {
            if !stderr.contains("timeout") {
                stderr = if stderr.is_empty() {
                    "timeout".into()
                } else {
                    format!("{stderr}\ntimeout")
                };
            }
            (-1, false)
        }
        StopReason::WaitError(e) => {
            stderr = if stderr.is_empty() {
                e
            } else {
                format!("{stderr}\n{e}")
            };
            (-1, false)
        }
    };

    CmdResult {
        program: program.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cmdline,
        cwd: cwd.map(|p| p.display().to_string()),
        exit_code,
        stdout,
        stderr,
        ok,
    }
}

fn kill_process_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn decode_output(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.trim_end().to_string();
    }
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

pub fn which(name: &str) -> Option<PathBuf> {
    which_all(name).into_iter().next()
}

pub fn which_all(name: &str) -> Vec<PathBuf> {
    let r = run_cmd("where", &[name], None);
    if !r.ok {
        return Vec::new();
    }
    r.stdout
        .lines()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

const PYTHON_STORE_ALIAS_EXES: &[&str] = &[
    "python.exe",
    "python3.exe",
    "pythonw.exe",
    "python3w.exe",
];

pub fn windowsapps_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|p| p.join("Microsoft").join("WindowsApps"))
}

fn is_store_stub(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.is_dir() {
        return false;
    }
    if meta.len() == 0 {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

pub fn python_store_alias_paths() -> Vec<PathBuf> {
    let Some(dir) = windowsapps_dir() else {
        return Vec::new();
    };
    PYTHON_STORE_ALIAS_EXES
        .iter()
        .map(|name| dir.join(name))
        .filter(|p| is_store_stub(p))
        .collect()
}

pub fn python_store_alias_present() -> bool {
    !python_store_alias_paths().is_empty()
}

/// 关掉微软商店占的 python.exe / python3.exe 别名（0 字节 APPEXECLINK）。
/// 等价于设置 → 应用执行别名里把这两项关掉。
pub fn disable_python_store_aliases() -> Vec<String> {
    let mut removed = Vec::new();
    for p in python_store_alias_paths() {
        if let Ok(meta) = fs::symlink_metadata(&p) {
            let mut perms = meta.permissions();
            if perms.readonly() {
                perms.set_readonly(false);
                let _ = fs::set_permissions(&p, perms);
            }
        }
        match fs::remove_file(&p) {
            Ok(()) => {
                log::info!("removed python store alias {}", p.display());
                removed.push(p.display().to_string());
            }
            Err(e) => log::warn!("remove python store alias {}: {e}", p.display()),
        }
    }
    removed
}

pub fn path_looks_like_windowsapps(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("WindowsApps"))
    })
}

#[allow(dead_code)]
pub fn find_winget() -> Option<PathBuf> {
    which("winget").or_else(|| {
        let p = dirs::data_local_dir()?
            .join("Microsoft")
            .join("WindowsApps")
            .join("winget.exe");
        if p.exists() {
            Some(p)
        } else {
            None
        }
    })
}

#[allow(dead_code)]
pub fn find_scoop() -> Option<PathBuf> {
    which("scoop")
}

pub fn run_cmd_exe(script: &str) -> CmdResult {
    run_cmd("cmd.exe", &["/D", "/C", script], None)
}

pub fn run_powershell(script: &str) -> CmdResult {
    run_cmd(
        "powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
        None,
    )
}

pub fn ps5_profile_path() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("WindowsPowerShell")
        .join("Microsoft.PowerShell_profile.ps1")
}

pub fn ps7_profile_path() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PowerShell")
        .join("Microsoft.PowerShell_profile.ps1")
}

pub fn file_contains_marker(path: &Path, begin: &str) -> bool {
    fs::read_to_string(path)
        .map(|s| s.contains(begin))
        .unwrap_or(false)
}

pub fn upsert_marked_block(path: &Path, begin: &str, end: &str, body: &str) -> Result<String> {
    let original = if path.exists() {
        fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };
    let new_block = format!("{begin}\n{body}\n{end}\n");
    let updated = if original.contains(begin) && original.contains(end) {
        replace_marked(&original, begin, end, &new_block)
    } else {
        let mut s = original.clone();
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&new_block);
        s
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, &updated)?;
    Ok(original)
}

pub fn strip_marked_block(path: &Path, begin: &str, end: &str) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    let original = fs::read_to_string(path)?;
    if !original.contains(begin) {
        return Ok(original);
    }
    let updated = replace_marked(&original, begin, end, "");
    let trimmed = updated.trim_end().to_string();
    if trimmed.is_empty() {
        fs::remove_file(path)?;
    } else {
        fs::write(path, format!("{trimmed}\n"))?;
    }
    Ok(original)
}

fn replace_marked(src: &str, begin: &str, end: &str, replacement: &str) -> String {
    let Some(start) = src.find(begin) else {
        return format!("{src}{replacement}");
    };
    let after = &src[start..];
    let Some(end_rel) = after.find(end) else {
        return src.to_string();
    };
    let end_abs = start + end_rel + end.len();
    let mut rest = &src[end_abs..];
    if rest.starts_with('\r') {
        rest = &rest[1..];
    }
    if rest.starts_with('\n') {
        rest = &rest[1..];
    }
    format!("{}{}{}", &src[..start], replacement, rest)
}

pub fn execution_policy() -> String {
    run_powershell("Get-ExecutionPolicy -Scope CurrentUser").stdout
}

pub fn now_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string()
}

pub fn write_log_file(dir: &Path, prefix: &str, content: &str) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let p = dir.join(format!("{prefix}-{}.txt", now_stamp()));
    fs::write(&p, content)?;
    Ok(p)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppMemory {
    pub working_set: u64,
    pub private: u64,
    pub processes: u32,
    pub rust_working_set: u64,
    pub webview_working_set: u64,
}

pub fn app_memory() -> AppMemory {
    #[cfg(windows)]
    {
        memory_process_tree()
    }
    #[cfg(not(windows))]
    {
        AppMemory {
            working_set: 0,
            private: 0,
            processes: 1,
            rust_working_set: 0,
            webview_working_set: 0,
        }
    }
}

#[cfg(windows)]
fn memory_process_tree() -> AppMemory {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let self_pid = unsafe { GetCurrentProcessId() };
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap.is_null() || snap == INVALID_HANDLE_VALUE {
        return sum_pids(&[(self_pid, "envdesk.exe".to_string())]);
    }

    let mut pe: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut pairs: Vec<(u32, u32, String)> = Vec::new();
    unsafe {
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                let name_len = pe
                    .szExeFile
                    .iter()
                    .position(|&ch| ch == 0)
                    .unwrap_or(pe.szExeFile.len());
                pairs.push((
                    pe.th32ProcessID,
                    pe.th32ParentProcessID,
                    String::from_utf16_lossy(&pe.szExeFile[..name_len]),
                ));
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }

    let self_name = pairs
        .iter()
        .find(|(pid, _, _)| *pid == self_pid)
        .map(|(_, _, name)| name.clone())
        .unwrap_or_else(|| "envdesk.exe".to_string());
    let mut pids = vec![(self_pid, self_name)];
    let mut changed = true;
    while changed {
        changed = false;
        for (pid, parent, name) in &pairs {
            if pids.iter().any(|(known, _)| known == parent)
                && !pids.iter().any(|(known, _)| known == pid)
                && *pid != 0
            {
                pids.push((*pid, name.clone()));
                changed = true;
            }
        }
    }
    sum_pids(&pids)
}

#[cfg(windows)]
fn sum_pids(pids: &[(u32, String)]) -> AppMemory {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    let mut working_set = 0u64;
    let mut private = 0u64;
    let mut processes = 0u32;
    let mut rust_working_set = 0u64;
    let mut webview_working_set = 0u64;
    let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ;
    for (pid, name) in pids {
        unsafe {
            let h = OpenProcess(access, 0, *pid);
            if h.is_null() {
                continue;
            }
            let mut pmc: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            let ok = GetProcessMemoryInfo(h, &mut pmc as *mut _ as *mut _, pmc.cb);
            let _ = CloseHandle(h);
            if ok == 0 {
                continue;
            }
            working_set += pmc.WorkingSetSize as u64;
            private += pmc.PrivateUsage as u64;
            processes += 1;
            let lname = name.to_ascii_lowercase();
            if lname.contains("msedgewebview2") {
                webview_working_set += pmc.WorkingSetSize as u64;
            } else if lname.contains("envdesk") {
                rust_working_set += pmc.WorkingSetSize as u64;
            }
        }
    }
    AppMemory {
        working_set,
        private,
        processes,
        rust_working_set,
        webview_working_set,
    }
}

