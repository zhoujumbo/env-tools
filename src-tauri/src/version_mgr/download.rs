use crate::paths::{AppPaths, Prefs};
use anyhow::{bail, Context, Result};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct TempPath {
    path: PathBuf,
    is_dir: bool,
    persist: bool,
}

impl TempPath {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            is_dir: false,
            persist: false,
        }
    }

    pub fn dir(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            is_dir: true,
            persist: false,
        }
    }

    pub fn persist(mut self) {
        self.persist = true;
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if self.persist {
            return;
        }
        if self.is_dir {
            let _ = fs::remove_dir_all(&self.path);
        } else {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn partial_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".into());
    dest.with_file_name(format!("{name}.partial"))
}

pub fn agent(paths: &AppPaths) -> Result<ureq::Agent> {
    let mut b = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(600))
        .user_agent("envdesk/0.1");
    if let Some(raw) = resolve_proxy(&Prefs::load(paths))? {
        let proxy = ureq::Proxy::new(&raw).with_context(|| format!("无效代理: {raw}"))?;
        b = b.proxy(proxy);
    }
    Ok(b.build())
}

/// `on_progress(bytes, content_length)`
pub fn download_file_progress(
    paths: &AppPaths,
    url: &str,
    dest: &Path,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = partial_path(dest);
    if tmp.exists() {
        fs::remove_file(&tmp).with_context(|| format!("remove leftover {}", tmp.display()))?;
    }
    let _guard = TempPath::file(tmp.clone());
    let resp = agent(paths)?
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    if resp.status() >= 400 {
        bail!("HTTP {} from {url}", resp.status());
    }
    let total = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok());
    let mut reader = resp.into_reader();
    let mut file = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let mut buf = [0u8; 64 * 1024];
    let mut copied = 0u64;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    on_progress(0, total);
    loop {
        if super::is_cancelled() {
            bail!("{}", super::CANCEL_MSG);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        copied += n as u64;
        if last_emit.elapsed() >= Duration::from_millis(80) {
            on_progress(copied, total);
            last_emit = Instant::now();
        }
    }
    on_progress(copied, total);
    file.flush()?;
    drop(file);
    if dest.exists() {
        fs::remove_file(dest)?;
    }
    fs::rename(&tmp, dest)?;
    _guard.persist();
    Ok(())
}

/// `on_progress(done, total_entries)`
pub fn extract_zip_progress(
    zip_path: &Path,
    dest: &Path,
    on_progress: &mut dyn FnMut(usize, usize),
) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::create_dir_all(dest)?;
    let _guard = TempPath::dir(dest.to_path_buf());
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let total = archive.len();
    on_progress(0, total);
    for i in 0..total {
        if super::is_cancelled() {
            bail!("{}", super::CANCEL_MSG);
        }
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut outfile = File::create(&out)?;
            io::copy(&mut entry, &mut outfile)?;
        }
        if i == total - 1 || i % 8 == 0 {
            on_progress(i + 1, total);
        }
    }
    on_progress(total, total);
    _guard.persist();
    Ok(())
}

/// If dest contains a single directory, hoist its contents up.
pub fn hoist_single_root(dest: &Path) -> Result<()> {
    let mut dirs = Vec::new();
    let mut files = 0usize;
    for e in fs::read_dir(dest)?.flatten() {
        if e.file_type()?.is_dir() {
            dirs.push(e.path());
        } else {
            files += 1;
        }
    }
    if files == 0 && dirs.len() == 1 {
        let only = dirs.remove(0);
        let staging = sibling_staging(dest, "hoist");
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        let _guard = TempPath::dir(staging.clone());
        fs::rename(&only, &staging)?;
        fs::remove_dir_all(dest)?;
        fs::rename(&staging, dest)?;
        _guard.persist();
    }
    Ok(())
}

pub fn hoist_subdir(dest: &Path, name: &str) -> Result<()> {
    let sub = dest.join(name);
    if !sub.is_dir() {
        return Ok(());
    }
    let staging = sibling_staging(dest, "hoist");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    let _guard = TempPath::dir(staging.clone());
    fs::rename(&sub, &staging)?;
    fs::remove_dir_all(dest)?;
    fs::rename(&staging, dest)?;
    _guard.persist();
    Ok(())
}

fn sibling_staging(dest: &Path, tag: &str) -> std::path::PathBuf {
    let name = dest
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tmp".into());
    dest.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{name}-{tag}"))
}

fn resolve_proxy(prefs: &Prefs) -> Result<Option<String>> {
    match prefs.proxy_mode.as_str() {
        "none" => Ok(None),
        "manual" => Ok(Some(manual_proxy_url(prefs)?)),
        _ => Ok(system_proxy()),
    }
}

fn manual_proxy_url(prefs: &Prefs) -> Result<String> {
    let (host, port) = split_host_port(&prefs.proxy_host, prefs.proxy_port)?;
    Ok(format_proxy_url("http", &host, port, &prefs.proxy_user, &prefs.proxy_pass))
}

fn split_host_port(raw_host: &str, port: u16) -> Result<(String, u16)> {
    let stripped = strip_proxy_scheme(raw_host.trim());
    let host = stripped.split('/').next().unwrap_or(stripped.as_str()).trim();
    if let Some((h, p)) = host.rsplit_once(':') {
        if !h.is_empty() && !h.ends_with(']') {
            if let Ok(parsed) = p.parse::<u16>() {
                if parsed != 0 {
                    return Ok((h.to_string(), parsed));
                }
            }
        }
    }
    if host.is_empty() {
        bail!("手动代理需要填写地址");
    }
    if port == 0 {
        bail!("手动代理需要填写端口");
    }
    Ok((host.to_string(), port))
}

fn format_proxy_url(scheme: &str, host: &str, port: u16, user: &str, pass: &str) -> String {
    let user = user.trim();
    let pass = pass.trim();
    if user.is_empty() {
        format!("{scheme}://{host}:{port}")
    } else if pass.is_empty() {
        format!("{scheme}://{}@{host}:{port}", percent_encode(user))
    } else {
        format!(
            "{scheme}://{}:{}@{host}:{port}",
            percent_encode(user),
            percent_encode(pass)
        )
    }
}

fn system_proxy() -> Option<String> {
    windows_ie_proxy().or_else(env_proxy)
}

fn windows_ie_proxy() -> Option<String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    let enable: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    if enable == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    parse_ie_proxy_server(&server)
}

fn parse_ie_proxy_server(server: &str) -> Option<String> {
    let s = server.trim();
    if s.is_empty() {
        return None;
    }
    if !s.contains('=') {
        return Some(to_proxy_url(s));
    }
    let mut https = None;
    let mut http = None;
    let mut socks = None;
    for part in s.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (scheme, rest) = part.split_once('=').unwrap_or(("http", part));
        let rest = rest.trim();
        match scheme.trim().to_ascii_lowercase().as_str() {
            "https" => https = Some(to_proxy_url(rest)),
            "http" => http = Some(to_proxy_url(rest)),
            "socks" | "socks5" => socks = Some(format!("socks5://{}", strip_proxy_scheme(rest))),
            _ => {}
        }
    }
    https.or(http).or(socks)
}

fn env_proxy() -> Option<String> {
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(to_proxy_url(v));
            }
        }
    }
    None
}

fn to_proxy_url(s: &str) -> String {
    let s = s.trim();
    if s.contains("://") {
        s.to_string()
    } else {
        format!("http://{s}")
    }
}

fn strip_proxy_scheme(s: &str) -> String {
    for prefix in ["http://", "https://", "socks5://", "socks://", "socks5h://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    s.to_string()
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
