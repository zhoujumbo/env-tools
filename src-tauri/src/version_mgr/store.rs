use crate::journal::{self, Db};
use crate::paths::AppPaths;
use crate::winutil;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Node,
    Python,
    Java,
    Golang,
}

impl RuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Python => "python",
            Self::Java => "java",
            Self::Golang => "golang",
        }
    }

    pub fn home_env(self) -> Option<&'static str> {
        match self {
            Self::Java => Some("JAVA_HOME"),
            Self::Golang => Some("GOROOT"),
            _ => None,
        }
    }

    pub fn which_name(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Python => "python",
            Self::Java => "java",
            Self::Golang => "go",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionMeta {
    pub version: String,
    pub npm: Option<String>,
    pub pip: Option<String>,
    pub pnpm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub kind: String,
    pub root: String,
    pub current: Option<String>,
    pub current_path: Option<String>,
    pub versions: Vec<String>,
    pub version_meta: Vec<VersionMeta>,
    pub path_ok: bool,
    pub path_entries: Vec<String>,
    /// `where python` / `where node` 的第一条。
    pub cmd_which: Option<String>,
    /// WindowsApps 里还有商店 python 别名（0 字节 stub）。
    pub store_alias: bool,
    /// cmd 通道 `pnpm -v` 成功（仅 Node 有意义）。
    #[serde(default)]
    pub pnpm_cmd_ok: bool,
    /// 已装 pnpm 但 cmd 找不到（不是内部或外部命令）。
    #[serde(default)]
    pub pnpm_needs_fix: bool,
    #[serde(default)]
    pub pnpm_which: Option<String>,
}

pub struct RuntimeStore {
    pub kind: RuntimeKind,
    pub root: PathBuf,
}

impl RuntimeStore {
    pub fn node(paths: &AppPaths) -> Self {
        Self {
            kind: RuntimeKind::Node,
            root: paths.node_runtime(),
        }
    }

    pub fn python(paths: &AppPaths) -> Self {
        Self {
            kind: RuntimeKind::Python,
            root: paths.python_runtime(),
        }
    }

    pub fn java(paths: &AppPaths) -> Self {
        Self {
            kind: RuntimeKind::Java,
            root: paths.java_runtime(),
        }
    }

    pub fn golang(paths: &AppPaths) -> Self {
        Self {
            kind: RuntimeKind::Golang,
            root: paths.golang_runtime(),
        }
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn current_dir(&self) -> PathBuf {
        self.root.join("current")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join(".cache")
    }

    pub fn version_dir(&self, version: &str) -> Result<PathBuf> {
        let ver = normalize_version(self.kind, version)?;
        let dir = self.versions_dir().join(&ver);
        if dir.file_name().and_then(|s| s.to_str()) != Some(ver.as_str()) {
            bail!("非法版本路径");
        }
        Ok(dir)
    }

    pub fn assert_dir_under_versions(&self, dir: &Path) -> Result<()> {
        let versions = self.versions_dir();
        if versions.exists() && crate::paths::is_inside(&versions, dir) {
            return Ok(());
        }
        if dir.starts_with(&versions)
            && dir.file_name().and_then(|s| s.to_str()).is_some_and(|n| normalize_version(self.kind, n).is_ok())
        {
            return Ok(());
        }
        bail!("拒绝操作 versions 目录外路径: {}", dir.display());
    }

    pub fn path_entries(&self) -> Vec<String> {
        let current = self.current_dir();
        match self.kind {
            RuntimeKind::Python => vec![
                current.display().to_string(),
                current.join("Scripts").display().to_string(),
            ],
            RuntimeKind::Java | RuntimeKind::Golang => {
                vec![current.join("bin").display().to_string()]
            }
            RuntimeKind::Node => vec![current.display().to_string()],
        }
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(self.versions_dir())?;
        fs::create_dir_all(self.cache_dir())?;
        Ok(())
    }

    /// 清掉中断下载/解压留下的临时文件。完整 zip/nupkg 缓存保留。
    pub fn cleanup_leftovers(&self) {
        sweep_cache_temps(&self.cache_dir());
        sweep_incomplete_versions(&self.versions_dir(), self.kind);
    }

    pub fn is_complete(&self, dir: &Path) -> bool {
        match self.kind {
            RuntimeKind::Node => dir.join("node.exe").is_file(),
            RuntimeKind::Python => dir.join("python.exe").is_file(),
            RuntimeKind::Java => dir.join("bin").join("java.exe").is_file(),
            RuntimeKind::Golang => dir.join("bin").join("go.exe").is_file(),
        }
    }

    pub fn list_installed(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(self.versions_dir()) else {
            return out;
        };
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                out.push(e.file_name().to_string_lossy().into_owned());
            }
        }
        out.sort_by(|a, b| cmp_version(b, a));
        out
    }

    pub fn current_version(&self) -> Option<String> {
        let current = self.current_dir();
        if !junction_or_dir_exists(&current) {
            return None;
        }
        let target = fs::canonicalize(&current).ok()?;
        let versions = fs::canonicalize(self.versions_dir()).ok()?;
        let rel = target.strip_prefix(&versions).ok()?;
        rel.components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .map(|s| s.to_string())
    }

    pub fn status(&self) -> RuntimeStatus {
        let entries = self.path_entries();
        let path_ok = user_path_has_all(&entries);
        let cmd_which = winutil::which(self.kind.which_name()).map(|p| p.display().to_string());
        let store_alias = self.kind == RuntimeKind::Python && winutil::python_store_alias_present();
        let versions = self.list_installed();
        let version_meta = versions
            .iter()
            .map(|v| {
                let dir = self.version_dir(v).unwrap_or_else(|_| self.versions_dir().join(v));
                VersionMeta {
                    version: v.clone(),
                    npm: if self.kind == RuntimeKind::Node {
                        bundled_npm_version(&dir)
                    } else {
                        None
                    },
                    pip: if self.kind == RuntimeKind::Python {
                        bundled_pip_version(&dir)
                    } else {
                        None
                    },
                    pnpm: if self.kind == RuntimeKind::Node {
                        bundled_pnpm_version(&dir)
                    } else {
                        None
                    },
                }
            })
            .collect();
        RuntimeStatus {
            kind: self.kind.as_str().into(),
            root: self.root.display().to_string(),
            current: self.current_version(),
            current_path: if junction_or_dir_exists(&self.current_dir()) {
                Some(self.current_dir().display().to_string())
            } else {
                None
            },
            versions,
            version_meta,
            path_ok,
            path_entries: entries,
            cmd_which,
            store_alias,
            pnpm_cmd_ok: false,
            pnpm_needs_fix: false,
            pnpm_which: None,
        }
    }

    pub fn set_current(&self, db: &Db, version: &str) -> Result<RuntimeStatus> {
        let ver = normalize_version(self.kind, version)?;
        let target = self.version_dir(&ver)?;
        if !target.is_dir() {
            bail!("未安装版本 {ver}");
        }
        self.assert_dir_under_versions(&target)?;
        let target_abs = fs::canonicalize(&target)?;
        let current = self.current_dir();
        let before = self.current_version();
        replace_junction(&current, &target_abs)?;
        if self.kind == RuntimeKind::Python {
            ensure_exe_alias(&target_abs, "python.exe", "python3.exe");
            ensure_exe_alias(&target_abs, "pythonw.exe", "python3w.exe");
        }
        self.ensure_user_path(db)?;
        self.ensure_home_env(db)?;
        if self.kind == RuntimeKind::Node {
            // pnpm 的 PATH 绑定当前版本目录，缓存必须失效，否则下次启动会跳过核对。
            crate::version_mgr::node::invalidate_pnpm_probe(db);
        }
        winutil::broadcast_environment();
        journal::log_action(
            db,
            self.kind.as_str(),
            "set_current",
            Some(ver.clone()),
            Some(&serde_json::json!({ "current": before })),
            Some(&serde_json::json!({ "current": ver })),
            Some(&serde_json::json!({ "type": "runtime_current", "kind": self.kind.as_str(), "previous": before })),
            true,
        )?;
        Ok(self.status())
    }

    pub fn uninstall(&self, db: &Db, version: &str) -> Result<RuntimeStatus> {
        let ver = normalize_version(self.kind, version)?;
        let dir = self.version_dir(&ver)?;
        if !dir.is_dir() {
            bail!("未安装版本 {ver}");
        }
        self.assert_dir_under_versions(&dir)?;
        let was_current = self.current_version().as_deref() == Some(ver.as_str());
        if was_current {
            let _ = fs::remove_dir(self.current_dir());
            let _ = junction::delete(self.current_dir());
        }
        fs::remove_dir_all(&dir).with_context(|| format!("删除 {}", dir.display()))?;
        if was_current {
            if let Some(next) = self.list_installed().into_iter().next() {
                let _ = self.set_current(db, &next);
            } else {
                self.remove_user_path(db)?;
                self.clear_home_env(db)?;
                winutil::broadcast_environment();
            }
        }
        journal::log_action(
            db,
            self.kind.as_str(),
            "uninstall",
            Some(ver),
            None,
            None,
            None,
            true,
        )?;
        Ok(self.status())
    }

    fn ensure_home_env(&self, db: &Db) -> Result<()> {
        let Some(key) = self.kind.home_env() else {
            return Ok(());
        };
        let current = self.current_dir();
        if !junction_or_dir_exists(&current) {
            return Ok(());
        }
        // 用 current junction 本身（不要 canonicalize 成 \\?\ + versions\x）。
        let abs = normalize_env_path(&current);
        let before = winutil::get_user_env(key)?;
        let already = before.as_deref() == Some(abs.as_str());
        if !already {
            winutil::set_user_env(key, &abs)?;
            db.add_touched_env_key(key)?;
            journal::log_action(
                db,
                self.kind.as_str(),
                "set_home_env",
                Some(format!("{key}={abs}")),
                Some(&serde_json::json!({ key: before })),
                Some(&serde_json::json!({ key: abs })),
                Some(&serde_json::json!({ "type": "user_env", "before": { key: before } })),
                true,
            )?;
        }
        std::env::set_var(key, &abs);
        Ok(())
    }

    fn clear_home_env(&self, db: &Db) -> Result<()> {
        let Some(key) = self.kind.home_env() else {
            return Ok(());
        };
        let before = winutil::get_user_env(key)?;
        let Some(cur) = before.as_ref() else {
            return Ok(());
        };
        let ours = normalize_env_path(&self.current_dir());
        let ours_target = fs::canonicalize(self.current_dir())
            .ok()
            .map(|p| strip_verbatim_prefix(&p.display().to_string()));
        let mine = path_eq(cur, &ours) || ours_target.as_ref().is_some_and(|t| path_eq(cur, t));
        if !mine {
            return Ok(());
        }
        winutil::delete_user_env(key)?;
        std::env::remove_var(key);
        db.add_touched_env_key(key)?;
        journal::log_action(
            db,
            self.kind.as_str(),
            "clear_home_env",
            Some(key.into()),
            Some(&serde_json::json!({ key: before })),
            None,
            Some(&serde_json::json!({ "type": "user_env", "before": { key: before } })),
            true,
        )?;
        Ok(())
    }

    pub fn ensure_user_path(&self, db: &Db) -> Result<()> {
        let before = winutil::get_user_env("Path")?;
        if self.kind == RuntimeKind::Python {
            let removed = winutil::disable_python_store_aliases();
            if !removed.is_empty() {
                journal::log_action(
                    db,
                    self.kind.as_str(),
                    "disable_store_alias",
                    Some(removed.join(";")),
                    None,
                    None,
                    None,
                    true,
                )?;
            }
        }
        // 倒序 prepend，让 path_entries 的第一项最终排在最前。
        for entry in self.path_entries().into_iter().rev() {
            winutil::prepend_user_path(&entry)?;
            winutil::prepend_process_path(&entry);
        }
        let after = winutil::get_user_env("Path")?;
        if after != before {
            db.add_touched_env_key("Path")?;
            journal::log_action(
                db,
                self.kind.as_str(),
                "path_prepend",
                Some(self.path_entries().join(";")),
                Some(&serde_json::json!({ "Path": before })),
                None,
                Some(&serde_json::json!({ "type": "user_env", "before": { "Path": before } })),
                true,
            )?;
            winutil::broadcast_environment();
        }
        let _ = self.ensure_home_env(db);
        Ok(())
    }

    fn remove_user_path(&self, db: &Db) -> Result<()> {
        let before = winutil::get_user_env("Path")?;
        for entry in self.path_entries() {
            winutil::remove_user_path_entry(&entry)?;
        }
        db.add_touched_env_key("Path")?;
        journal::log_action(
            db,
            self.kind.as_str(),
            "path_remove",
            Some(self.path_entries().join(";")),
            Some(&serde_json::json!({ "Path": before })),
            None,
            Some(&serde_json::json!({ "type": "user_env", "before": { "Path": before } })),
            true,
        )?;
        Ok(())
    }
}

pub fn normalize_version(kind: RuntimeKind, raw: &str) -> Result<String> {
    let t = raw.trim().trim_start_matches(['v', 'V']);
    if crate::paths::has_path_escape(t) || t.contains("..") {
        bail!("非法版本号");
    }
    let first = t.chars().next().ok_or_else(|| anyhow::anyhow!("版本号为空"))?;
    if !first.is_ascii_digit()
        || !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
    {
        bail!("非法版本号");
    }
    Ok(match kind {
        RuntimeKind::Node => format!("v{t}"),
        RuntimeKind::Python | RuntimeKind::Java | RuntimeKind::Golang => t.to_string(),
    })
}

pub fn is_tool_version(raw: &str) -> bool {
    let t = raw.trim().trim_start_matches(['v', 'V']);
    !t.is_empty()
        && !crate::paths::has_path_escape(t)
        && t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
}

fn junction_or_dir_exists(path: &Path) -> bool {
    path.exists() || junction::exists(path).unwrap_or(false)
}

/// Windows `canonicalize` 会加 `\\?\`，很多 Java/Go 工具不认。
fn strip_verbatim_prefix(s: &str) -> String {
    s.strip_prefix(r"\\?\")
        .unwrap_or(s)
        .trim_end_matches(['/', '\\'])
        .to_string()
}

fn normalize_env_path(path: &Path) -> String {
    strip_verbatim_prefix(&path.display().to_string())
}

fn path_eq(a: &str, b: &str) -> bool {
    strip_verbatim_prefix(a).eq_ignore_ascii_case(&strip_verbatim_prefix(b))
}

fn replace_junction(link: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    if junction_or_dir_exists(link) {
        let _ = junction::delete(link);
        let _ = fs::remove_dir(link);
    }
    junction::create(target, link).with_context(|| {
        format!(
            "创建 junction {} -> {}",
            link.display(),
            target.display()
        )
    })?;
    Ok(())
}

pub(crate) fn ensure_exe_alias(dir: &Path, src_name: &str, alias_name: &str) {
    let src = dir.join(src_name);
    let dst = dir.join(alias_name);
    if !src.is_file() || dst.exists() {
        return;
    }
    if fs::hard_link(&src, &dst).is_err() {
        let _ = fs::copy(&src, &dst);
    }
}

fn bundled_npm_version(dir: &Path) -> Option<String> {
    let p = dir.join("node_modules").join("npm").join("package.json");
    let raw = fs::read_to_string(p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("version")?.as_str().map(|s| s.to_string())
}

fn bundled_pnpm_version(dir: &Path) -> Option<String> {
    if dir.join("pnpm.cmd").is_file() || dir.join("pnpm.exe").is_file() {
        let pkg = dir.join("node_modules").join("pnpm").join("package.json");
        if let Ok(raw) = fs::read_to_string(pkg) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(ver) = v.get("version").and_then(|x| x.as_str()) {
                    return Some(ver.to_string());
                }
            }
        }
        return Some(String::new());
    }
    None
}

fn bundled_pip_version(dir: &Path) -> Option<String> {
    let site = dir.join("Lib").join("site-packages");
    let rd = fs::read_dir(site).ok()?;
    for e in rd.flatten() {
        let name = e.file_name();
        let s = name.to_string_lossy();
        if s.starts_with("pip-") && s.ends_with(".dist-info") && e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let ver = s
                .trim_start_matches("pip-")
                .trim_end_matches(".dist-info")
                .to_string();
            if !ver.is_empty() {
                return Some(ver);
            }
        }
    }
    None
}

fn sweep_cache_temps(cache: &Path) {
    let Ok(rd) = fs::read_dir(cache) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let s = name.to_string_lossy();
        let path = e.path();
        let file = e.file_type().map(|t| t.is_file()).unwrap_or(false);
        let dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if file && s.ends_with(".partial") {
            let _ = fs::remove_file(&path);
            continue;
        }
        if dir && (s.ends_with("-extract") || (s.starts_with('.') && s.ends_with("-hoist"))) {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn sweep_incomplete_versions(versions: &Path, kind: RuntimeKind) {
    let Ok(rd) = fs::read_dir(versions) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let complete = match kind {
            RuntimeKind::Node => path.join("node.exe").is_file(),
            RuntimeKind::Python => path.join("python.exe").is_file(),
            RuntimeKind::Java => path.join("bin").join("java.exe").is_file(),
            RuntimeKind::Golang => path.join("bin").join("go.exe").is_file(),
        };
        if !complete {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn user_path_has_all(entries: &[String]) -> bool {
    let current = winutil::get_user_env("Path")
        .ok()
        .flatten()
        .unwrap_or_default();
    let parts: Vec<&str> = current.split(';').filter(|s| !s.is_empty()).collect();
    entries
        .iter()
        .all(|e| parts.iter().any(|p| p.eq_ignore_ascii_case(e)))
}

fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let pa = parse_ver(a);
    let pb = parse_ver(b);
    pa.cmp(&pb)
}

fn parse_ver(s: &str) -> Vec<u32> {
    s.trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse().ok())
        .collect()
}
