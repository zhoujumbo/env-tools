use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub fn is_inside(root: &Path, child: &Path) -> bool {
    match (fs::canonicalize(root), fs::canonicalize(child)) {
        (Ok(root), Ok(child)) => child.starts_with(root),
        _ => false,
    }
}

pub fn has_path_escape(name: &str) -> bool {
    let p = Path::new(name);
    name.is_empty()
        || name.contains('\0')
        || p.components().count() != 1
        || matches!(
            p.components().next(),
            Some(Component::ParentDir | Component::CurDir | Component::RootDir | Component::Prefix(_))
        )
}

pub const APP_DIR_NAME: &str = "envts";
pub const ENVT_PATH_VAR: &str = "ENVT_PATH";
pub const MARKER_BEGIN: &str = "# BEGIN desktop-env-tool fnm";
pub const MARKER_END: &str = "# END desktop-env-tool fnm";
pub const ELEVATION_HELPER_ARG: &str = "--det-elevated-apply-hosts";
pub const ELEVATION_SYSENV_ARG: &str = "--det-elevated-sys-env";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl AppPaths {
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("envdesk.db")
    }
    pub fn hosts_dir(&self) -> PathBuf {
        self.data_dir.join("hosts")
    }
    pub fn hosts_entries_dir(&self) -> PathBuf {
        self.hosts_dir().join("entries")
    }
    pub fn hosts_manifest(&self) -> PathBuf {
        self.hosts_dir().join("manifest.json")
    }
    pub fn hosts_trash(&self) -> PathBuf {
        self.hosts_dir().join("trashcan.json")
    }
    pub fn baseline_dir(&self) -> PathBuf {
        self.config_dir.join("baseline")
    }
    pub fn shell_dir(&self) -> PathBuf {
        self.config_dir.join("shell")
    }
    pub fn fnm_init_cmd(&self) -> PathBuf {
        self.shell_dir().join("fnm_init.cmd")
    }
    pub fn first_run_flag(&self) -> PathBuf {
        self.config_dir.join(".first_run_done")
    }
    pub fn prefs_path(&self) -> PathBuf {
        self.config_dir.join("prefs.json")
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }
    pub fn runtimes_dir(&self) -> PathBuf {
        storage_root().join("runtimes")
    }
    pub fn node_runtime(&self) -> PathBuf {
        self.runtimes_dir().join("node")
    }
    pub fn python_runtime(&self) -> PathBuf {
        self.runtimes_dir().join("python")
    }
    pub fn java_runtime(&self) -> PathBuf {
        self.runtimes_dir().join("java")
    }
    pub fn golang_runtime(&self) -> PathBuf {
        self.runtimes_dir().join("golang")
    }
}

/// `ENVT_PATH` from the process environment, then HKCU user env (set in 系统属性 before launch).
pub fn envt_path_value() -> Option<PathBuf> {
    let from_proc = std::env::var(ENVT_PATH_VAR)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let from_user = crate::winutil::get_user_env(ENVT_PATH_VAR)
        .ok()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    from_proc.or(from_user).map(PathBuf::from)
}

pub fn storage_root() -> PathBuf {
    envt_path_value().unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_DIR_NAME)
    })
}

pub fn from_envt_path() -> bool {
    envt_path_value().is_some()
}

pub fn paths_json_path() -> PathBuf {
    storage_root().join("paths.json")
}

pub fn default_paths() -> AppPaths {
    let root = storage_root();
    AppPaths {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        log_dir: root.join("logs"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsView {
    pub root_dir: PathBuf,
    pub from_envt_path: bool,
    pub envt_path: Option<String>,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl PathsView {
    pub fn from_app(p: &AppPaths) -> Self {
        Self {
            root_dir: storage_root(),
            from_envt_path: from_envt_path(),
            envt_path: envt_path_value().map(|p| p.display().to_string()),
            config_dir: p.config_dir.clone(),
            data_dir: p.data_dir.clone(),
            log_dir: p.log_dir.clone(),
        }
    }
}

pub fn load_or_init() -> anyhow::Result<AppPaths> {
    let json_path = paths_json_path();
    let paths = if json_path.exists() {
        let raw = fs::read_to_string(&json_path)?;
        serde_json::from_str::<AppPaths>(&raw)?
    } else {
        let p = default_paths();
        if let Some(parent) = json_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&json_path, serde_json::to_string_pretty(&p)?)?;
        p
    };
    ensure_dirs(&paths)?;
    Ok(paths)
}

pub fn ensure_dirs(paths: &AppPaths) -> anyhow::Result<()> {
    for d in [
        &paths.config_dir,
        &paths.data_dir,
        &paths.log_dir,
        &paths.hosts_dir(),
        &paths.hosts_entries_dir(),
        &paths.baseline_dir(),
        &paths.shell_dir(),
        &paths.backups_dir(),
        &paths.runtimes_dir(),
    ] {
        fs::create_dir_all(d)?;
    }
    Ok(())
}

pub fn dir_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write_probe");
    let ok = fs::write(&probe, b"ok").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

pub fn save_paths(new_paths: &AppPaths, migrate: bool, old: &AppPaths) -> anyhow::Result<()> {
    ensure_dirs(new_paths)?;
    if migrate {
        copy_dir_contents(&old.config_dir, &new_paths.config_dir)?;
        copy_dir_contents(&old.data_dir, &new_paths.data_dir)?;
        copy_dir_contents(&old.log_dir, &new_paths.log_dir)?;
    }
    let json_path = paths_json_path();
    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(json_path, serde_json::to_string_pretty(new_paths)?)?;
    Ok(())
}

fn copy_dir_contents(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if !src.exists() || src == dst {
        return Ok(());
    }
    fs::create_dir_all(dst)?;
    for entry in walkdir::WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let rel = match entry.path().strip_prefix(src) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            if !target.exists() {
                fs::copy(entry.path(), &target)?;
            }
        }
    }
    Ok(())
}

fn default_theme() -> String {
    "dark".into()
}

fn default_false() -> bool {
    false
}

fn default_true() -> bool {
    true
}

fn default_empty() -> String {
    String::new()
}

fn default_proxy_mode() -> String {
    "system".into()
}

fn default_zero_u16() -> u16 {
    0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prefs {
    pub flush_dns_after_hosts: bool,
    pub backup_keep: u32,
    /// `dark` | `light` | `system`
    #[serde(default = "default_theme")]
    pub theme: String,
    /// 切换 Node 时是否改写 npm/pnpm 的 prefix 与 cache
    #[serde(default = "default_false")]
    pub node_pkg_override: bool,
    /// 自定义根目录；空则保持系统默认。开启且非空时使用 `{root}/node_global` 与 `{root}/node_cache`
    #[serde(default = "default_empty")]
    pub node_pkg_root: String,
    /// `none` | `system` | `manual`，默认跟随系统代理
    #[serde(default = "default_proxy_mode")]
    pub proxy_mode: String,
    #[serde(default = "default_empty")]
    pub proxy_host: String,
    #[serde(default = "default_zero_u16")]
    pub proxy_port: u16,
    #[serde(default = "default_empty")]
    pub proxy_user: String,
    #[serde(default = "default_empty")]
    pub proxy_pass: String,
    /// 最小化时收进托盘并释放 WebView2；关掉则保持普通最小化
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            flush_dns_after_hosts: true,
            backup_keep: 10,
            theme: default_theme(),
            node_pkg_override: false,
            node_pkg_root: String::new(),
            proxy_mode: default_proxy_mode(),
            proxy_host: String::new(),
            proxy_port: 0,
            proxy_user: String::new(),
            proxy_pass: String::new(),
            minimize_to_tray: true,
        }
    }
}

impl Prefs {
    pub fn load(paths: &AppPaths) -> Self {
        let p = paths.prefs_path();
        if let Ok(raw) = fs::read_to_string(p) {
            if let Ok(v) = serde_json::from_str(&raw) {
                return v;
            }
        }
        Self::default()
    }

    pub fn save(&self, paths: &AppPaths) -> anyhow::Result<()> {
        fs::write(paths.prefs_path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
