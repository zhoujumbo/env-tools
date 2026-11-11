use super::download;
use super::progress::ProgressEvent;
use super::store::{normalize_version, RuntimeStore};
use crate::journal::{self, Db};
use crate::paths::{AppPaths, Prefs};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const NODE_INDEX: &str = "https://nodejs.org/dist/index.json";
const NODE_DIST: &str = "https://nodejs.org/dist";
/// GUI 默认 TTL：nodejs.org 一周内最多发几次版，手动刷新可立刻更新。
const INDEX_TTL_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Deserialize)]
struct NodeIndexEntry {
    version: String,
    lts: serde_json::Value,
    files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteVersion {
    pub version: String,
    pub lts: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteList {
    pub versions: Vec<RemoteVersion>,
    pub from_cache: bool,
    pub stale: bool,
    pub fetched_at: Option<i64>,
    pub ttl_secs: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct IndexMeta {
    fetched_at: i64,
    etag: Option<String>,
}

pub fn list_remote(paths: &AppPaths, force: bool) -> Result<RemoteList> {
    let store = RuntimeStore::node(paths);
    store.ensure_dirs()?;
    let cache_path = store.cache_dir().join("dist-index.json");
    let meta_path = store.cache_dir().join("dist-index.meta.json");
    let cached = read_index_cache(&cache_path, &meta_path);
    let now = chrono::Utc::now().timestamp();

    if !force {
        if let Some((versions, meta)) = &cached {
            if now.saturating_sub(meta.fetched_at) < INDEX_TTL_SECS {
                return Ok(RemoteList {
                    versions: versions.clone(),
                    from_cache: true,
                    stale: false,
                    fetched_at: Some(meta.fetched_at),
                    ttl_secs: INDEX_TTL_SECS,
                });
            }
        }
    }

    let etag = cached.as_ref().and_then(|(_, m)| m.etag.clone());
    match fetch_index(paths, etag.as_deref()) {
        Ok(FetchOutcome::NotModified) => {
            if let Some((versions, mut meta)) = cached {
                meta.fetched_at = now;
                let _ = write_index_meta(&meta_path, &meta);
                return Ok(RemoteList {
                    versions,
                    from_cache: true,
                    stale: false,
                    fetched_at: Some(now),
                    ttl_secs: INDEX_TTL_SECS,
                });
            }
            bail!("远程 304 但本地没有缓存");
        }
        Ok(FetchOutcome::Fresh { versions, etag }) => {
            let _ = fs::write(
                &cache_path,
                serde_json::to_vec_pretty(&versions).unwrap_or_default(),
            );
            let _ = write_index_meta(
                &meta_path,
                &IndexMeta {
                    fetched_at: now,
                    etag,
                },
            );
            Ok(RemoteList {
                versions,
                from_cache: false,
                stale: false,
                fetched_at: Some(now),
                ttl_secs: INDEX_TTL_SECS,
            })
        }
        Err(e) => {
            if let Some((versions, meta)) = cached {
                return Ok(RemoteList {
                    versions,
                    from_cache: true,
                    stale: true,
                    fetched_at: Some(meta.fetched_at),
                    ttl_secs: INDEX_TTL_SECS,
                });
            }
            Err(e)
        }
    }
}

enum FetchOutcome {
    NotModified,
    Fresh {
        versions: Vec<RemoteVersion>,
        etag: Option<String>,
    },
}

fn fetch_index(paths: &AppPaths, etag: Option<&str>) -> Result<FetchOutcome> {
    let mut req = download::agent(paths)?.get(NODE_INDEX);
    if let Some(tag) = etag {
        req = req.set("If-None-Match", tag);
    }
    let resp = req.call().with_context(|| format!("GET {NODE_INDEX}"))?;
    if resp.status() == 304 {
        return Ok(FetchOutcome::NotModified);
    }
    if resp.status() >= 400 {
        bail!("HTTP {} from {NODE_INDEX}", resp.status());
    }
    let new_etag = resp.header("etag").map(|s| s.to_string());
    let raw: Vec<NodeIndexEntry> = resp
        .into_json()
        .with_context(|| format!("parse JSON {NODE_INDEX}"))?;
    Ok(FetchOutcome::Fresh {
        versions: parse_index(raw),
        etag: new_etag,
    })
}

fn parse_index(raw: Vec<NodeIndexEntry>) -> Vec<RemoteVersion> {
    let mut out = Vec::new();
    for e in raw {
        if !e.files.iter().any(|f| f == "win-x64-zip") {
            continue;
        }
        let lts = match &e.lts {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Bool(true) => Some("LTS".into()),
            _ => None,
        };
        out.push(RemoteVersion {
            version: e.version,
            lts,
        });
    }
    out
}

fn read_index_cache(cache: &Path, meta: &Path) -> Option<(Vec<RemoteVersion>, IndexMeta)> {
    let versions: Vec<RemoteVersion> = serde_json::from_slice(&fs::read(cache).ok()?).ok()?;
    let meta: IndexMeta = serde_json::from_slice(&fs::read(meta).ok()?).ok()?;
    Some((versions, meta))
}

fn write_index_meta(path: &Path, meta: &IndexMeta) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(meta)?)?;
    Ok(())
}

pub fn install(
    db: &Db,
    paths: &AppPaths,
    version: &str,
    mut emit: impl FnMut(ProgressEvent),
) -> Result<super::store::RuntimeStatus> {
    let store = RuntimeStore::node(paths);
    super::with_runtime_op(&store, || {
        store.ensure_dirs()?;
        let ver = normalize_version(store.kind, version)?;
        let dest = store.version_dir(&ver)?;
        store.assert_dir_under_versions(&dest)?;
        emit(ProgressEvent::node("install", "resolve", &ver, format!("准备安装 {ver}")));
        super::check_cancel()?;

        if dest.join("node.exe").is_file() {
            if store.current_version().is_none() {
                emit(ProgressEvent::node("install", "link", &ver, "已安装，设为 current"));
                store.set_current(db, &ver)?;
                apply_pkg_layout(db, paths, &ver)?;
            }
            emit(ProgressEvent::node("install", "done", &ver, "已存在，跳过下载").with_percent(100.0));
            return Ok(store.status());
        }

        let zip_name = format!("node-{ver}-win-x64.zip");
        let url = format!("{NODE_DIST}/{ver}/{zip_name}");
        let zip_path = store.cache_dir().join(&zip_name);
        let zip_ok = zip_path.is_file()
            && fs::metadata(&zip_path)
                .map(|m| m.len() > 1_000_000)
                .unwrap_or(false);

        if !zip_ok {
            emit(ProgressEvent::node("install", "download", &ver, format!("下载 {zip_name}")));
            download::download_file_progress(paths, &url, &zip_path, &mut |bytes, total| {
                emit(
                    ProgressEvent::node("install", "download", &ver, format!("下载 {zip_name}"))
                        .with_bytes(bytes, total),
                );
            })?;
            super::check_cancel()?;
        } else {
            emit(ProgressEvent::node("install", "download", &ver, "使用本地 zip 缓存").with_percent(100.0));
        }

        let staging = store.cache_dir().join(format!("{ver}-extract"));
        let staging_guard = download::TempPath::dir(staging.clone());
        emit(ProgressEvent::node("install", "extract", &ver, "解压"));
        download::extract_zip_progress(&zip_path, &staging, &mut |done, total| {
            let pct = if total > 0 {
                (done as f32 / total as f32) * 100.0
            } else {
                0.0
            };
            emit(
                ProgressEvent::node("install", "extract", &ver, format!("解压 {done}/{total}"))
                    .with_percent(pct),
            );
        })?;
        super::check_cancel()?;
        download::hoist_single_root(&staging)?;
        if !staging.join("node.exe").is_file() {
            let _ = fs::remove_file(&zip_path);
            bail!("zip 中没有 node.exe，版本 {ver} 可能不是 win-x64");
        }
        super::check_cancel()?;
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        fs::rename(&staging, &dest)?;
        staging_guard.persist();

        journal::log_action(
            db,
            "node",
            "install",
            Some(ver.clone()),
            None,
            Some(&serde_json::json!({ "dir": dest.display().to_string() })),
            None,
            true,
        )?;

        if store.current_version().is_none() {
            emit(ProgressEvent::node("install", "link", &ver, "设为 current"));
            store.set_current(db, &ver)?;
            apply_pkg_layout(db, paths, &ver)?;
        }
        emit(ProgressEvent::node("install", "done", &ver, "安装完成").with_percent(100.0));
        Ok(store.status())
    })
}

pub fn node_info(paths: &AppPaths, version: &str) -> serde_json::Value {
    let store = RuntimeStore::node(paths);
    let Ok(ver) = normalize_version(store.kind, version) else {
        return serde_json::json!({ "node": version, "npm": null, "ok": false });
    };
    let Ok(dir) = store.version_dir(&ver) else {
        return serde_json::json!({ "node": ver, "npm": null, "ok": false });
    };
    let ok = dir.join("node.exe").is_file();
    serde_json::json!({
        "node": ver,
        "npm": bundled_npm(&dir),
        "ok": ok
    })
}

fn bundled_npm(dir: &Path) -> Option<String> {
    let p = dir.join("node_modules").join("npm").join("package.json");
    let raw = fs::read_to_string(p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("version")?.as_str().map(|s| s.to_string())
}

pub fn npm_install_version(
    db: &Db,
    paths: &AppPaths,
    node_ver: &str,
    npm_ver: &str,
    mut emit: impl FnMut(ProgressEvent),
) -> Result<String> {
    let store = RuntimeStore::node(paths);
    super::with_runtime_op(&store, || {
        let ver = normalize_version(store.kind, node_ver)?;
        let dir = store.version_dir(&ver)?;
        let npm = dir.join("npm.cmd");
        if !npm.exists() {
            bail!("{} 没有 npm.cmd", dir.display());
        }
        let npm_ver = npm_ver.trim().trim_start_matches('v').trim_start_matches('V');
        if !super::store::is_tool_version(npm_ver) {
            bail!("非法 npm 版本号");
        }
        let pkg = format!("npm@{npm_ver}");
        emit(ProgressEvent::node(
            "npm",
            "npm",
            &ver,
            format!("安装 {pkg} 到 {ver}"),
        ));
        let r = interruptible_cmd(
            &npm.display().to_string(),
            &["install", "-g", &pkg],
            Some(&dir),
            Duration::from_secs(180),
        );
        journal::log_command(db, paths, "node", "npm_upgrade", &r, None, None, None)?;
        reject_cancel(&r)?;
        if r.ok {
            emit(ProgressEvent::node("npm", "done", &ver, format!("npm → {npm_ver}")).with_percent(100.0));
            Ok(r.stdout)
        } else {
            emit(ProgressEvent::node("npm", "error", &ver, r.stderr.clone()));
            bail!(r.stderr)
        }
    })
}

pub fn pnpm_install(
    db: &Db,
    paths: &AppPaths,
    node_ver: &str,
    mut emit: impl FnMut(ProgressEvent),
) -> Result<String> {
    let store = RuntimeStore::node(paths);
    super::with_runtime_op(&store, || {
        let ver = normalize_version(store.kind, node_ver)?;
        let dir = store.version_dir(&ver)?;
        let corepack = dir.join("corepack.cmd");
        let npm = dir.join("npm.cmd");
        emit(ProgressEvent::node("pnpm", "pnpm", &ver, format!("安装 pnpm 到 {ver}")));
        super::check_cancel()?;

        let out = if corepack.is_file() {
            emit(ProgressEvent::node("pnpm", "pnpm", &ver, "corepack enable"));
            let en = interruptible_cmd(
                &corepack.display().to_string(),
                &["enable"],
                Some(&dir),
                Duration::from_secs(60),
            );
            journal::log_command(db, paths, "node", "pnpm_corepack_enable", &en, None, None, None)?;
            reject_cancel(&en)?;
            if !en.ok {
                emit(ProgressEvent::node("pnpm", "error", &ver, en.stderr.clone()));
                bail!("corepack enable 失败: {}", en.stderr);
            }
            emit(ProgressEvent::node(
                "pnpm",
                "pnpm",
                &ver,
                "corepack prepare pnpm@latest --activate",
            ));
            let prep = interruptible_cmd(
                &corepack.display().to_string(),
                &["prepare", "pnpm@latest", "--activate"],
                Some(&dir),
                Duration::from_secs(180),
            );
            journal::log_command(db, paths, "node", "pnpm_prepare", &prep, None, None, None)?;
            reject_cancel(&prep)?;
            if !prep.ok {
                // 旧 corepack 可能没有 prepare；enable 已经写出 pnpm.cmd
                if !dir.join("pnpm.cmd").is_file() {
                    emit(ProgressEvent::node("pnpm", "error", &ver, prep.stderr.clone()));
                    bail!("corepack prepare pnpm 失败: {}", prep.stderr);
                }
            }
            format!("{}\n{}", en.stdout, prep.stdout)
        } else if npm.is_file() {
            emit(ProgressEvent::node("pnpm", "pnpm", &ver, "npm install -g pnpm"));
            let r = interruptible_cmd(
                &npm.display().to_string(),
                &["install", "-g", "pnpm"],
                Some(&dir),
                Duration::from_secs(180),
            );
            journal::log_command(db, paths, "node", "pnpm_install", &r, None, None, None)?;
            reject_cancel(&r)?;
            if !r.ok {
                emit(ProgressEvent::node("pnpm", "error", &ver, r.stderr.clone()));
                bail!(r.stderr);
            }
            r.stdout
        } else {
            bail!("{} 没有 corepack.cmd / npm.cmd", dir.display());
        };

        if dir.join("pnpm.cmd").is_file() {
            let _ = apply_pkg_layout(db, paths, &ver);
        }
        emit(ProgressEvent::node("pnpm", "done", &ver, "pnpm 已安装").with_percent(100.0));
        let _ = fix_pnpm_env(db, paths);
        Ok(out)
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PnpmEnv {
    pub installed: bool,
    pub cmd_ok: bool,
    pub needs_fix: bool,
    pub shim: Option<String>,
    pub which: Option<String>,
    pub version: Option<String>,
    pub detail: String,
}

fn pnpm_in_dir(dir: &Path) -> Option<PathBuf> {
    for name in ["pnpm.cmd", "pnpm.exe"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn canonical_pnpm_dirs(paths: &AppPaths) -> Vec<PathBuf> {
    let store = RuntimeStore::node(paths);
    let prefs = Prefs::load(paths);
    let (global, _, custom) = desired_pkg_dirs(&prefs);
    let mut dirs = Vec::new();
    if let Some(cur) = store.current_version() {
        if let Ok(dir) = store.version_dir(&cur) {
            dirs.push(dir);
        }
        dirs.push(store.current_dir());
    }
    dirs.push(global.clone());
    if !custom {
        let home = known_pnpm_home();
        if !paths_match(&home, &global) {
            dirs.push(home);
        }
    }
    dirs
}

fn path_is_canonical_pnpm(exe: &Path, paths: &AppPaths) -> bool {
    let Some(parent) = exe.parent() else {
        return false;
    };
    canonical_pnpm_dirs(paths)
        .iter()
        .any(|d| paths_match(parent, d))
}

/// 当前布局下的 pnpm CLI。旧 prefix / 其它 Node 版本里的 shim 不算已安装。
pub fn find_pnpm_shim(paths: &AppPaths) -> Option<PathBuf> {
    for dir in canonical_pnpm_dirs(paths) {
        if let Some(p) = pnpm_in_dir(&dir) {
            return Some(p);
        }
    }
    None
}

fn pnpm_version_of(exe: &Path) -> Option<String> {
    let cwd = exe.parent()?;
    let r = crate::winutil::run_cmd_timeout(
        &exe.display().to_string(),
        &["-v"],
        Some(cwd),
        Duration::from_secs(20),
    );
    if !r.ok {
        return None;
    }
    r.stdout
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty() && !s.starts_with("npm "))
        .map(|s| s.to_string())
}

pub fn probe_pnpm_env(paths: &AppPaths) -> PnpmEnv {
    let prefs = Prefs::load(paths);
    let shim = find_pnpm_shim(paths);
    let which = crate::winutil::which_on_registered_path("pnpm")
        .map(|p| p.display().to_string());
    let which_canonical = which
        .as_deref()
        .map(Path::new)
        .is_some_and(|p| path_is_canonical_pnpm(p, paths));
    let version = if which_canonical {
        which.as_deref().and_then(|w| pnpm_version_of(Path::new(w)))
    } else {
        shim.as_deref().and_then(pnpm_version_of)
    };
    let installed = shim.is_some();
    let stale_path = stale_user_pkg_path_entries(&prefs, None);
    let pnpm_home_ok = pnpm_home_matches(&prefs);
    let keep_on_path = pkg_path_keep(&prefs)
        .iter()
        .all(|k| user_path_has(&k.display().to_string()));
    let cmd_ok = installed && which_canonical && version.is_some();
    let needs_fix = !stale_path.is_empty()
        || !pnpm_home_ok
        || !keep_on_path
        || (which.is_some() && !which_canonical)
        || (installed && which.is_none());
    let detail = if !stale_path.is_empty() {
        format!(
            "用户 PATH 仍有旧全局目录：{}。当前目标 {}。",
            stale_path.join("; "),
            pkg_path_keep(&prefs)
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        )
    } else if which.is_some() && !which_canonical {
        format!(
            "新开 cmd 会命中旧 pnpm {}，不是当前布局。目标目录 {}。",
            which.as_deref().unwrap_or("-"),
            pkg_path_keep(&prefs)
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("; ")
        )
    } else if !pnpm_home_ok {
        format!(
            "PNPM_HOME={}，与当前布局 {} 不一致。",
            crate::winutil::get_user_env("PNPM_HOME")
                .ok()
                .flatten()
                .unwrap_or_else(|| "(未设置)".into()),
            desired_pnpm_home(&prefs).display()
        )
    } else if installed && which.is_none() {
        format!(
            "已找到 {}，但用户 PATH 里没有 pnpm。需要写入 PATH 并新开终端。",
            shim.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
        )
    } else if !installed {
        "当前布局下没有 pnpm（旧目录里的不算）。到 Node 页点「装 pnpm」。".into()
    } else if cmd_ok {
        format!(
            "pnpm {} · where={}",
            version.as_deref().unwrap_or("?"),
            which.as_deref().unwrap_or("-")
        )
    } else {
        "pnpm 环境不一致，请点「修复环境」或「装 pnpm」。".into()
    };
    PnpmEnv {
        installed,
        cmd_ok,
        needs_fix,
        shim: shim.map(|p| p.display().to_string()),
        which,
        version,
        detail,
    }
}

pub fn apply_pnpm_probe(mut st: super::store::RuntimeStatus, env: &PnpmEnv) -> super::store::RuntimeStatus {
    st.pnpm_cmd_ok = env.cmd_ok;
    st.pnpm_needs_fix = env.needs_fix;
    st.pnpm_which = env.which.clone();
    st
}

/// `pnpm -v` 会起一个 node 进程（冷启动 1–2s）。启动核对用缓存挡一层，
/// 五分钟内重复启动不再探测；写操作（切版本/装 pnpm/修复）会主动失效。
const PNPM_PROBE_TTL_SECS: i64 = 300;
const PNPM_PROBE_META_KEY: &str = "pnpm_probe_at";

fn pnpm_probe_fresh(db: &Db) -> bool {
    let Ok(Some(raw)) = db.meta_get(PNPM_PROBE_META_KEY) else {
        return false;
    };
    let Ok(ts) = raw.trim().parse::<i64>() else {
        return false;
    };
    ts > 0 && chrono::Utc::now().timestamp().saturating_sub(ts) < PNPM_PROBE_TTL_SECS
}

pub fn invalidate_pnpm_probe(db: &Db) {
    let _ = db.meta_set(PNPM_PROBE_META_KEY, "0");
}

fn mark_pnpm_probe_ok(db: &Db) {
    let _ = db.meta_set(
        PNPM_PROBE_META_KEY,
        &chrono::Utc::now().timestamp().to_string(),
    );
}

/// 启动时：PATH / PNPM_HOME 与当前布局不一致就修。不自动 corepack enable。
pub fn ensure_pnpm_on_startup(db: &Db, paths: &AppPaths) -> Result<()> {
    if pnpm_probe_fresh(db) {
        return Ok(());
    }
    let env = probe_pnpm_env(paths);
    if env.needs_fix {
        let env = fix_pnpm_env(db, paths)?;
        if env.needs_fix {
            return Ok(());
        }
    }
    mark_pnpm_probe_ok(db);
    Ok(())
}

pub fn fix_pnpm_env(db: &Db, paths: &AppPaths) -> Result<PnpmEnv> {
    let store = RuntimeStore::node(paths);
    if store.current_version().is_some() {
        store.ensure_user_path(db)?;
    }
    let _ = sync_npm_config(db, paths)?;
    crate::winutil::broadcast_environment();
    let env = probe_pnpm_env(paths);
    if !env.needs_fix {
        mark_pnpm_probe_ok(db);
    }
    Ok(env)
}

fn interruptible_cmd(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: Duration,
) -> crate::winutil::CmdResult {
    crate::winutil::run_cmd_interruptible(program, args, cwd, timeout, super::is_cancelled)
}

fn reject_cancel(r: &crate::winutil::CmdResult) -> Result<()> {
    if super::is_cancelled() || r.stderr.contains(super::CANCEL_MSG) {
        bail!("{}", super::CANCEL_MSG);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct NodePkgLayout {
    pub enabled: bool,
    pub root: String,
    pub custom: bool,
    pub global_dir: String,
    pub cache_dir: String,
    pub npm_prefix_default: String,
    pub npm_cache_default: String,
    pub pnpm_home_default: String,
    pub pnpm_store_default: String,
    pub pnpm_cache_default: String,
    pub global_exists: bool,
    pub cache_exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NpmUserConfig {
    pub available: bool,
    pub version: Option<String>,
    pub prefix: String,
    pub cache: String,
    pub registry: String,
    pub prefix_desired: String,
    pub cache_desired: String,
    pub prefix_ok: bool,
    pub cache_ok: bool,
    pub custom: bool,
    pub detail: String,
}

pub fn pkg_dirs(root: &str) -> (PathBuf, PathBuf) {
    let r = PathBuf::from(root.trim());
    (r.join("node_global"), r.join("node_cache"))
}

pub fn ensure_pkg_dirs(root: &str) -> Result<(PathBuf, PathBuf)> {
    let (global, cache) = pkg_dirs(root);
    fs::create_dir_all(&global).with_context(|| format!("创建 {}", global.display()))?;
    fs::create_dir_all(&cache).with_context(|| format!("创建 {}", cache.display()))?;
    Ok((global, cache))
}

fn known_npm_prefix() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("npm")
}

fn known_npm_cache() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("npm-cache")
}

fn known_pnpm_home() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("pnpm")
}

fn desired_pkg_dirs(prefs: &Prefs) -> (PathBuf, PathBuf, bool) {
    let root = prefs.node_pkg_root.trim();
    if prefs.node_pkg_override && !root.is_empty() {
        let (g, c) = pkg_dirs(root);
        (g, c, true)
    } else {
        (known_npm_prefix(), known_npm_cache(), false)
    }
}

fn ensure_desired_pkg_dirs(prefs: &Prefs) -> Result<(PathBuf, PathBuf, bool)> {
    let (global, cache, custom) = desired_pkg_dirs(prefs);
    fs::create_dir_all(&global).with_context(|| format!("创建 {}", global.display()))?;
    fs::create_dir_all(&cache).with_context(|| format!("创建 {}", cache.display()))?;
    Ok((global, cache, custom))
}

fn pkg_path_keep(prefs: &Prefs) -> Vec<PathBuf> {
    let (global, _, custom) = desired_pkg_dirs(prefs);
    let mut keep = vec![global.clone()];
    if !custom {
        let home = known_pnpm_home();
        if !paths_match(&home, &global) {
            keep.push(home);
        }
    }
    keep
}

fn desired_pnpm_home(prefs: &Prefs) -> PathBuf {
    let (global, _, custom) = desired_pkg_dirs(prefs);
    if custom {
        global
    } else {
        known_pnpm_home()
    }
}

fn keep_keys(prefs: &Prefs) -> Vec<String> {
    pkg_path_keep(prefs)
        .iter()
        .map(|p| p.display().to_string())
        .collect()
}

fn managed_default_keys() -> Vec<String> {
    vec![
        known_npm_prefix().display().to_string(),
        known_pnpm_home().display().to_string(),
    ]
}

fn extra_drop_keys(old: Option<&Prefs>, new: &Prefs) -> Vec<String> {
    let mut out = Vec::new();
    let Some(old) = old else {
        return out;
    };
    let new_keep = keep_keys(new);
    for p in pkg_path_keep(old) {
        let s = p.display().to_string();
        if new_keep.iter().any(|k| path_keys_equal(k, &s)) {
            continue;
        }
        out.push(s);
    }
    let old_root = old.node_pkg_root.trim();
    if !old_root.is_empty() {
        let (g, _) = pkg_dirs(old_root);
        let s = g.display().to_string();
        if !new_keep.iter().any(|k| path_keys_equal(k, &s)) {
            out.push(s);
        }
    }
    out
}

fn user_path_parts() -> Vec<String> {
    crate::winutil::get_user_env("Path")
        .ok()
        .flatten()
        .or_else(|| crate::winutil::get_user_env("PATH").ok().flatten())
        .unwrap_or_default()
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn user_path_has(entry: &str) -> bool {
    user_path_parts()
        .iter()
        .any(|p| path_keys_equal(p, entry))
}

fn stale_user_pkg_path_entries(prefs: &Prefs, old: Option<&Prefs>) -> Vec<String> {
    let parts = user_path_parts();
    let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
    let keep = keep_keys(prefs);
    let extra = extra_drop_keys(old, prefs);
    let defaults = managed_default_keys();
    refs.iter()
        .filter(|p| should_drop_pkg_path_entry(p, &keep, &extra, &defaults))
        .map(|s| (*s).to_string())
        .collect()
}

fn pnpm_home_matches(prefs: &Prefs) -> bool {
    let want = desired_pnpm_home(prefs);
    match crate::winutil::get_user_env("PNPM_HOME") {
        Ok(Some(v)) if !v.trim().is_empty() => paths_match(v.trim(), &want),
        _ => false,
    }
}

fn is_node_global_entry(entry: &str) -> bool {
    let t = entry.trim().trim_end_matches(['\\', '/']);
    Path::new(t)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("node_global"))
}

fn path_keys_equal(a: &str, b: &str) -> bool {
    paths_match(a, b)
}

fn should_drop_pkg_path_entry(
    entry: &str,
    keep: &[String],
    extra_drop: &[String],
    managed_defaults: &[String],
) -> bool {
    if keep.iter().any(|k| path_keys_equal(entry, k)) {
        return false;
    }
    if extra_drop.iter().any(|d| path_keys_equal(entry, d)) {
        return true;
    }
    if is_node_global_entry(entry) {
        return true;
    }
    managed_defaults.iter().any(|d| path_keys_equal(entry, d))
}

pub(crate) fn filter_pkg_path_entries(
    parts: &[&str],
    keep: &[String],
    extra_drop: &[String],
    managed_defaults: &[String],
) -> Vec<String> {
    parts
        .iter()
        .filter(|p| !should_drop_pkg_path_entry(p, keep, extra_drop, managed_defaults))
        .map(|s| (*s).to_string())
        .collect()
}

fn prepend_keep(mut parts: Vec<String>, keep: &[String]) -> Vec<String> {
    for k in keep.iter().rev() {
        parts.retain(|p| !path_keys_equal(p, k));
        parts.insert(0, k.clone());
    }
    parts
}

pub fn repair_pkg_path(db: &Db, paths: &AppPaths, old: Option<&Prefs>) -> Result<(bool, String)> {
    let prefs = Prefs::load(paths);
    apply_pkg_path_and_home(db, &prefs, old)
}

fn apply_pkg_path_and_home(
    db: &Db,
    prefs: &Prefs,
    old: Option<&Prefs>,
) -> Result<(bool, String)> {
    let keep = keep_keys(prefs);
    let extra = extra_drop_keys(old, prefs);
    let defaults = managed_default_keys();
    let before = crate::winutil::get_user_env("Path")?
        .or_else(|| crate::winutil::get_user_env("PATH").ok().flatten())
        .unwrap_or_default();
    let parts: Vec<&str> = before.split(';').filter(|s| !s.is_empty()).collect();
    let filtered = filter_pkg_path_entries(&parts, &keep, &extra, &defaults);
    let after_parts = prepend_keep(filtered, &keep);
    let after = after_parts.join(";");
    let mut notes = Vec::new();
    let mut changed = after != before;
    if changed {
        crate::winutil::set_user_path(&after)?;
        db.add_touched_env_key("Path")?;
        notes.push("已同步用户 PATH 到当前全局目录".into());
    }

    let proc = std::env::var("PATH").unwrap_or_default();
    let proc_parts: Vec<&str> = proc.split(';').filter(|s| !s.is_empty()).collect();
    let proc_filtered = filter_pkg_path_entries(&proc_parts, &keep, &extra, &defaults);
    let proc_after = prepend_keep(proc_filtered, &keep).join(";");
    if proc_after != proc {
        std::env::set_var("PATH", &proc_after);
    }

    let want_home = desired_pnpm_home(prefs).display().to_string();
    let before_home = crate::winutil::get_user_env("PNPM_HOME")?;
    if before_home.as_deref().map(str::trim) != Some(want_home.as_str()) {
        crate::winutil::set_user_env("PNPM_HOME", &want_home)?;
        db.add_touched_env_key("PNPM_HOME")?;
        changed = true;
        notes.push(format!("PNPM_HOME → {want_home}"));
    }
    std::env::set_var("PNPM_HOME", &want_home);

    if changed {
        crate::winutil::broadcast_environment();
    }
    Ok((changed, notes.join("；")))
}

fn find_pnpm_for_config(paths: &AppPaths) -> Option<PathBuf> {
    if let Some(p) = find_pnpm_shim(paths) {
        return Some(p);
    }
    let store = RuntimeStore::node(paths);
    for ver in store.list_installed() {
        if let Ok(dir) = store.version_dir(&ver) {
            if let Some(p) = pnpm_in_dir(&dir) {
                return Some(p);
            }
        }
    }
    None
}

fn apply_pnpm_layout_config(
    paths: &AppPaths,
    dir_hint: Option<&Path>,
    global: &Path,
    cache: &Path,
    custom: bool,
) -> Result<bool> {
    let Some(pnpm) = find_pnpm_for_config(paths).or_else(|| dir_hint.and_then(pnpm_in_dir)) else {
        return Ok(false);
    };
    let cwd = pnpm.parent().unwrap_or(global);
    if custom {
        pnpm_config_set(&pnpm, cwd, "global-bin-dir", &global.display().to_string())?;
        pnpm_config_set(
            &pnpm,
            cwd,
            "global-dir",
            &global.join("pnpm-global").display().to_string(),
        )?;
        pnpm_config_set(
            &pnpm,
            cwd,
            "cache-dir",
            &cache.join("pnpm").display().to_string(),
        )?;
        pnpm_config_set(
            &pnpm,
            cwd,
            "store-dir",
            &cache.join("pnpm-store").display().to_string(),
        )?;
    } else {
        let home = known_pnpm_home();
        pnpm_config_set(&pnpm, cwd, "global-bin-dir", &home.display().to_string())?;
        pnpm_config_set(
            &pnpm,
            cwd,
            "global-dir",
            &home.join("global").display().to_string(),
        )?;
        pnpm_config_set(
            &pnpm,
            cwd,
            "cache-dir",
            &home.join("cache").display().to_string(),
        )?;
        pnpm_config_set(
            &pnpm,
            cwd,
            "store-dir",
            &home.join("store").display().to_string(),
        )?;
    }
    Ok(true)
}

pub fn pkg_layout_status(paths: &AppPaths) -> NodePkgLayout {
    let prefs = Prefs::load(paths);
    let root = prefs.node_pkg_root.trim().to_string();
    let (global, cache, custom) = desired_pkg_dirs(&prefs);
    let pnpm = known_pnpm_home();
    NodePkgLayout {
        enabled: prefs.node_pkg_override,
        root,
        custom,
        global_dir: global.display().to_string(),
        cache_dir: cache.display().to_string(),
        npm_prefix_default: known_npm_prefix().display().to_string(),
        npm_cache_default: known_npm_cache().display().to_string(),
        pnpm_home_default: pnpm.display().to_string(),
        pnpm_store_default: pnpm.join("store").display().to_string(),
        pnpm_cache_default: pnpm.join("cache").display().to_string(),
        global_exists: global.is_dir(),
        cache_exists: cache.is_dir(),
    }
}

fn current_npm(paths: &AppPaths, version: Option<&str>) -> Result<Option<(String, PathBuf, PathBuf)>> {
    let store = RuntimeStore::node(paths);
    let ver = match version {
        Some(v) => normalize_version(store.kind, v)?,
        None => match store.current_version() {
            Some(v) => v,
            None => return Ok(None),
        },
    };
    let dir = store.version_dir(&ver)?;
    let npm = dir.join("npm.cmd");
    if !npm.is_file() {
        bail!("{} 没有 npm.cmd", dir.display());
    }
    Ok(Some((ver, dir, npm)))
}

fn unavailable_npm_config(prefs: &Prefs, detail: &str) -> NpmUserConfig {
    let (prefix, cache, custom) = desired_pkg_dirs(prefs);
    NpmUserConfig {
        available: false,
        version: None,
        prefix: String::new(),
        cache: String::new(),
        registry: String::new(),
        prefix_desired: prefix.display().to_string(),
        cache_desired: cache.display().to_string(),
        prefix_ok: false,
        cache_ok: false,
        custom,
        detail: detail.into(),
    }
}

pub fn npm_user_config(paths: &AppPaths) -> Result<NpmUserConfig> {
    let prefs = Prefs::load(paths);
    let (want_g, want_c, custom) = desired_pkg_dirs(&prefs);
    let Some((ver, dir, npm)) = current_npm(paths, None)? else {
        return Ok(unavailable_npm_config(
            &prefs,
            "未设置 current Node，无法读取 npm config",
        ));
    };
    let cfg = npm_config_map(&npm, &dir).unwrap_or_default();
    let prefix = cfg_str(&cfg, "prefix");
    let cache = cfg_str(&cfg, "cache");
    let registry = cfg_str(&cfg, "registry");
    let prefix_ok = paths_match(&prefix, &want_g);
    let cache_ok = paths_match(&cache, &want_c);
    Ok(NpmUserConfig {
        available: true,
        version: Some(ver),
        prefix,
        cache,
        registry,
        prefix_desired: want_g.display().to_string(),
        cache_desired: want_c.display().to_string(),
        prefix_ok,
        cache_ok,
        custom,
        detail: String::new(),
    })
}

/// 启动 / 切版本 / 改设置：核对 prefix+cache，不一致则写成面板目标值（自定义或系统默认）。
pub fn apply_pkg_layout(db: &Db, paths: &AppPaths, version: &str) -> Result<()> {
    let _ = sync_npm_config_for(db, paths, Some(version), None)?;
    Ok(())
}

pub fn sync_npm_config(db: &Db, paths: &AppPaths) -> Result<NpmUserConfig> {
    sync_npm_config_for(db, paths, None, None)
}

pub fn sync_npm_config_after_prefs(
    db: &Db,
    paths: &AppPaths,
    old: &Prefs,
) -> Result<NpmUserConfig> {
    sync_npm_config_for(db, paths, None, Some(old))
}

fn sync_npm_config_for(
    db: &Db,
    paths: &AppPaths,
    version: Option<&str>,
    old_prefs: Option<&Prefs>,
) -> Result<NpmUserConfig> {
    let prefs = Prefs::load(paths);
    let (global, cache, custom) = ensure_desired_pkg_dirs(&prefs)?;
    let want_g = global.display().to_string();
    let want_c = cache.display().to_string();
    let before_path = crate::winutil::get_user_env("Path")
        .ok()
        .flatten()
        .or_else(|| crate::winutil::get_user_env("PATH").ok().flatten());
    let (path_changed, path_notes) = apply_pkg_path_and_home(db, &prefs, old_prefs)?;

    let npm_pair = match current_npm(paths, version) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("skip npm config: {e}");
            None
        }
    };
    let Some((ver, dir, npm)) = npm_pair else {
        let mut cfg = unavailable_npm_config(
            &prefs,
            "未设置 current Node，已同步 PATH / PNPM_HOME，跳过 npm prefix",
        );
        if !path_notes.is_empty() {
            cfg.detail = path_notes;
        }
        return Ok(cfg);
    };

    let cfg = npm_config_map(&npm, &dir).unwrap_or_default();
    let before_prefix = cfg_str(&cfg, "prefix");
    let before_cache = cfg_str(&cfg, "cache");
    let registry = cfg_str(&cfg, "registry");

    let mut changed = Vec::new();
    if !paths_match(&before_prefix, &global) {
        npm_config_set(&npm, &dir, "prefix", &want_g)?;
        changed.push(format!("prefix {before_prefix} → {want_g}"));
    }
    if !paths_match(&before_cache, &cache) {
        npm_config_set(&npm, &dir, "cache", &want_c)?;
        changed.push(format!("cache {before_cache} → {want_c}"));
    }
    if !path_notes.is_empty() {
        changed.push(path_notes.clone());
    }

    match apply_pnpm_layout_config(paths, Some(&dir), &global, &cache, custom) {
        Ok(true) => changed.push("已同步 pnpm global/store 到当前目录".into()),
        Ok(false) => {
            if changed.iter().any(|s| s.starts_with("prefix ")) {
                changed.push("当前布局没有 pnpm CLI，未改 pnpm config".into());
            }
        }
        Err(e) => {
            log::warn!("pnpm config sync: {e}");
            changed.push(format!("pnpm config 同步失败: {e}"));
        }
    }

    let old_has_pkgs = !before_prefix.is_empty()
        && !paths_match(&before_prefix, &global)
        && (Path::new(&before_prefix).join("pnpm.cmd").is_file()
            || Path::new(&before_prefix).join("node_modules").is_dir());
    let new_empty = !global.join("pnpm.cmd").is_file()
        && !global.join("node_modules").is_dir()
        && !global.join("pnpm-global").is_dir();
    if old_has_pkgs && new_empty {
        changed.push("新全局目录为空，旧目录里的全局包不会自动搬过来".into());
    }

    if path_changed || !changed.is_empty() {
        journal::log_action(
            db,
            "node",
            "pkg_layout",
            Some(changed.join("; ")),
            Some(&serde_json::json!({
                "prefix": before_prefix,
                "cache": before_cache,
                "Path": before_path
            })),
            Some(&serde_json::json!({
                "prefix": want_g,
                "cache": want_c,
                "custom": custom,
                "version": ver
            })),
            Some(&serde_json::json!({ "type": "user_env", "before": { "Path": before_path } })),
            true,
        )?;
        log::info!("npm config synced: {}", changed.join("; "));
    }

    let prefix = if changed.iter().any(|s| s.starts_with("prefix ")) {
        want_g.clone()
    } else {
        before_prefix
    };
    let cache_now = if changed.iter().any(|s| s.starts_with("cache ")) {
        want_c.clone()
    } else {
        before_cache
    };
    Ok(NpmUserConfig {
        available: true,
        version: Some(ver),
        prefix: prefix.clone(),
        cache: cache_now.clone(),
        registry,
        prefix_desired: want_g.clone(),
        cache_desired: want_c.clone(),
        prefix_ok: paths_match(&prefix, &want_g),
        cache_ok: paths_match(&cache_now, &want_c),
        custom,
        detail: if changed.is_empty() {
            String::new()
        } else {
            format!("已同步: {}", changed.join("；"))
        },
    })
}

pub fn set_npm_registry(db: &Db, paths: &AppPaths, url: &str) -> Result<NpmUserConfig> {
    let url = normalize_registry(url)?;
    let Some((ver, dir, npm)) = current_npm(paths, None)? else {
        bail!("未设置 current Node，无法写入 registry");
    };
    let before = npm_config_get(&npm, &dir, "registry").unwrap_or_default();
    npm_config_set(&npm, &dir, "registry", &url)?;
    journal::log_action(
        db,
        "node",
        "npm_registry",
        Some(format!("registry {before} → {url}")),
        Some(&serde_json::json!({ "registry": before, "version": ver })),
        Some(&serde_json::json!({ "registry": url, "version": ver })),
        None,
        true,
    )?;
    let mut cfg = npm_user_config(paths)?;
    cfg.detail = format!("registry → {url}");
    Ok(cfg)
}

pub fn npm_cache_clean(db: &Db, paths: &AppPaths) -> Result<String> {
    let Some((ver, dir, npm)) = current_npm(paths, None)? else {
        bail!("未设置 current Node，无法清理 npm cache");
    };
    let r = crate::winutil::run_cmd_timeout(
        &npm.display().to_string(),
        &["cache", "clean", "--force"],
        Some(&dir),
        Duration::from_secs(90),
    );
    journal::log_command(db, paths, "node", "npm_cache_clean", &r, None, None, None)?;
    if r.ok {
        let out = r.stdout.trim();
        if out.is_empty() {
            Ok(format!("已执行 npm cache clean --force（{ver}）"))
        } else {
            Ok(out.to_string())
        }
    } else {
        bail!("npm cache clean --force 失败: {}", r.stderr)
    }
}

fn normalize_registry(url: &str) -> Result<String> {
    let s = url.trim();
    if s.is_empty() {
        bail!("镜像地址不能为空");
    }
    let lower = s.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        bail!("镜像地址必须以 http:// 或 https:// 开头");
    }
    if s.contains(char::is_whitespace) {
        bail!("镜像地址不能包含空白");
    }
    Ok(s.to_string())
}

fn paths_match(actual: impl AsRef<Path>, desired: impl AsRef<Path>) -> bool {
    let a = actual.as_ref();
    let b = desired.as_ref();
    if a.as_os_str().is_empty() {
        return false;
    }
    let na = norm_path_key(a);
    let nb = norm_path_key(b);
    if na == nb {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn norm_path_key(p: &Path) -> String {
    p.to_string_lossy()
        .trim()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn npm_config_map(npm: &Path, cwd: &Path) -> Result<serde_json::Value> {
    let r = crate::winutil::run_cmd_timeout(
        &npm.display().to_string(),
        &["config", "list", "--json"],
        Some(cwd),
        Duration::from_secs(20),
    );
    if !r.ok {
        bail!("npm config list 失败: {}", r.stderr)
    }
    let raw = r.stdout.trim();
    let json = raw.find('{').map(|i| &raw[i..]).unwrap_or(raw);
    serde_json::from_str(json).with_context(|| "parse npm config list --json")
}

fn cfg_str(cfg: &serde_json::Value, key: &str) -> String {
    cfg.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "undefined")
        .unwrap_or_default()
}

fn npm_config_get(npm: &Path, cwd: &Path, key: &str) -> Result<String> {
    let r = crate::winutil::run_cmd_timeout(
        &npm.display().to_string(),
        &["config", "get", key],
        Some(cwd),
        Duration::from_secs(20),
    );
    if !r.ok {
        bail!("npm config get {key} 失败: {}", r.stderr)
    }
    npm_config_value(&r.stdout).ok_or_else(|| anyhow::anyhow!("npm config get {key} 无输出"))
}

fn npm_config_value(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| *s != "undefined")
        .filter(|s| !s.starts_with("npm "))
        .last()
        .map(|s| s.to_string())
}

fn npm_config_set(npm: &Path, cwd: &Path, key: &str, value: &str) -> Result<()> {
    let r = crate::winutil::run_cmd_timeout(
        &npm.display().to_string(),
        &["config", "set", key, value, "--location=user"],
        Some(cwd),
        Duration::from_secs(30),
    );
    if r.ok {
        Ok(())
    } else {
        bail!("npm config set {key} 失败: {}", r.stderr)
    }
}

fn pnpm_config_set(pnpm: &Path, cwd: &Path, key: &str, value: &str) -> Result<()> {
    let r = crate::winutil::run_cmd_timeout(
        &pnpm.display().to_string(),
        &["config", "set", key, value, "--location=global"],
        Some(cwd),
        Duration::from_secs(30),
    );
    if r.ok {
        return Ok(());
    }
    let fallback = crate::winutil::run_cmd_timeout(
        &pnpm.display().to_string(),
        &["config", "set", key, value],
        Some(cwd),
        Duration::from_secs(30),
    );
    if fallback.ok {
        Ok(())
    } else {
        bail!("pnpm config set {key} 失败: {}", r.stderr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_stale_node_global_keeps_unrelated() {
        let parts = [
            r"D:\ProgramData\Node\node_global",
            r"C:\programs\Node\node_global",
            r"C:\Users\me\.cargo\bin",
            r"C:\Users\me\AppData\Roaming\npm",
        ];
        let keep = vec![r"D:\ProgramData\Node\node_global".to_string()];
        let extra = vec![r"C:\programs\Node\node_global".to_string()];
        let defaults = vec![
            r"C:\Users\me\AppData\Roaming\npm".to_string(),
            r"C:\Users\me\AppData\Local\pnpm".to_string(),
        ];
        let out = filter_pkg_path_entries(&parts, &keep, &extra, &defaults);
        assert_eq!(
            out,
            vec![
                r"D:\ProgramData\Node\node_global".to_string(),
                r"C:\Users\me\.cargo\bin".to_string(),
            ]
        );
    }

    #[test]
    fn default_layout_keeps_npm_prefix_and_pnpm_home() {
        let parts = [
            r"D:\ProgramData\Node\node_global",
            r"C:\Users\me\AppData\Roaming\npm",
            r"C:\Users\me\AppData\Local\pnpm",
            r"C:\Windows\system32",
        ];
        let keep = vec![
            r"C:\Users\me\AppData\Roaming\npm".to_string(),
            r"C:\Users\me\AppData\Local\pnpm".to_string(),
        ];
        let extra = vec![r"D:\ProgramData\Node\node_global".to_string()];
        let defaults = keep.clone();
        let out = filter_pkg_path_entries(&parts, &keep, &extra, &defaults);
        assert_eq!(
            out,
            vec![
                r"C:\Users\me\AppData\Roaming\npm".to_string(),
                r"C:\Users\me\AppData\Local\pnpm".to_string(),
                r"C:\Windows\system32".to_string(),
            ]
        );
    }

    #[test]
    fn node_global_suffix_is_detected() {
        assert!(is_node_global_entry(r"C:\programs\Node\node_global"));
        assert!(is_node_global_entry(r"D:\ProgramData\Node\node_global\"));
        assert!(!is_node_global_entry(r"C:\Users\me\AppData\Roaming\npm"));
        assert!(!is_node_global_entry(r"D:\node_global_backup\bin"));
    }
}
