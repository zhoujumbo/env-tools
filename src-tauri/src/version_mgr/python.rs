use super::download;
use super::progress::ProgressEvent;
use super::store::{normalize_version, RuntimeStore};
use crate::journal::{self, Db};
use crate::paths::AppPaths;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

const NUGET_INDEX: &str = "https://api.nuget.org/v3-flatcontainer/python/index.json";
const NUGET_PKG: &str = "https://api.nuget.org/v3-flatcontainer/python";
/// 与 Node 一致：一周内走缓存，刷新走 ETag。
const INDEX_TTL_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Deserialize)]
struct NugetIndex {
    versions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteVersion {
    pub version: String,
    /// 该系列最新补丁会带系列号（如 "3.12"），给 UI 当 badge / 「仅最新」筛选。
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
    let store = RuntimeStore::python(paths);
    store.ensure_dirs()?;
    let cache_path = store.cache_dir().join("nuget-index.json");
    let meta_path = store.cache_dir().join("nuget-index.meta.json");
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
    let mut req = download::agent(paths)?.get(NUGET_INDEX);
    if let Some(tag) = etag {
        req = req.set("If-None-Match", tag);
    }
    let resp = req.call().with_context(|| format!("GET {NUGET_INDEX}"))?;
    if resp.status() == 304 {
        return Ok(FetchOutcome::NotModified);
    }
    if resp.status() >= 400 {
        bail!("HTTP {} from {NUGET_INDEX}", resp.status());
    }
    let new_etag = resp.header("etag").map(|s| s.to_string());
    let raw: NugetIndex = resp
        .into_json()
        .with_context(|| format!("parse JSON {NUGET_INDEX}"))?;
    Ok(FetchOutcome::Fresh {
        versions: parse_index(raw.versions),
        etag: new_etag,
    })
}

fn parse_index(raw: Vec<String>) -> Vec<RemoteVersion> {
    let mut vers: Vec<String> = raw.into_iter().filter(|v| is_stable_python(v)).collect();
    vers.sort_by(|a, b| cmp_py(b, a));
    vers.dedup();
    tag_latest_series(vers)
}

/// 每个 minor 系列（3.12 / 3.13）最新补丁打上系列号，对应 Node 的 LTS badge。
fn tag_latest_series(vers: Vec<String>) -> Vec<RemoteVersion> {
    let mut seen = HashSet::new();
    vers.into_iter()
        .map(|version| {
            let series = series_of(&version);
            let latest = series
                .as_ref()
                .map(|s| seen.insert(s.clone()))
                .unwrap_or(false);
            RemoteVersion {
                lts: if latest { series } else { None },
                version,
            }
        })
        .collect()
}

fn series_of(v: &str) -> Option<String> {
    let mut parts = v.split('.');
    let maj = parts.next()?;
    let min = parts.next()?;
    if maj.is_empty() || min.is_empty() {
        return None;
    }
    Some(format!("{maj}.{min}"))
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
    let store = RuntimeStore::python(paths);
    super::with_runtime_op(&store, || {
        store.ensure_dirs()?;
        let ver = normalize_version(store.kind, version)?;
        let dest = store.version_dir(&ver)?;
        store.assert_dir_under_versions(&dest)?;
        emit(ProgressEvent::python(
            "install",
            "resolve",
            &ver,
            format!("准备安装 {ver}"),
        ));
        super::check_cancel()?;

        if dest.join("python.exe").is_file() {
            super::store::ensure_exe_alias(&dest, "python.exe", "python3.exe");
            super::store::ensure_exe_alias(&dest, "pythonw.exe", "python3w.exe");
            if store.current_version().is_none() {
                emit(ProgressEvent::python(
                    "install",
                    "link",
                    &ver,
                    "已安装，设为 current",
                ));
                store.set_current(db, &ver)?;
            }
            emit(
                ProgressEvent::python("install", "done", &ver, "已存在，跳过下载").with_percent(100.0),
            );
            return Ok(store.status());
        }

        let pkg_name = format!("python.{ver}.nupkg");
        let url = format!("{NUGET_PKG}/{ver}/{pkg_name}");
        let nupkg = store.cache_dir().join(&pkg_name);
        let pkg_ok = nupkg.is_file()
            && fs::metadata(&nupkg)
                .map(|m| m.len() > 1_000_000)
                .unwrap_or(false);

        if !pkg_ok {
            emit(ProgressEvent::python(
                "install",
                "download",
                &ver,
                format!("下载 {pkg_name}"),
            ));
            download::download_file_progress(paths, &url, &nupkg, &mut |bytes, total| {
                emit(
                    ProgressEvent::python("install", "download", &ver, format!("下载 {pkg_name}"))
                        .with_bytes(bytes, total),
                );
            })?;
            super::check_cancel()?;
        } else {
            emit(
                ProgressEvent::python("install", "download", &ver, "使用本地 nupkg 缓存")
                    .with_percent(100.0),
            );
        }

        let staging = store.cache_dir().join(format!("{ver}-extract"));
        let staging_guard = download::TempPath::dir(staging.clone());
        emit(ProgressEvent::python("install", "extract", &ver, "解压"));
        download::extract_zip_progress(&nupkg, &staging, &mut |done, total| {
            let pct = if total > 0 {
                (done as f32 / total as f32) * 100.0
            } else {
                0.0
            };
            emit(
                ProgressEvent::python("install", "extract", &ver, format!("解压 {done}/{total}"))
                    .with_percent(pct),
            );
        })?;
        super::check_cancel()?;
        download::hoist_subdir(&staging, "tools")?;
        if !staging.join("python.exe").is_file() {
            let _ = fs::remove_file(&nupkg);
            bail!("nuget 包里没有 python.exe（{ver}）");
        }
        super::check_cancel()?;
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        fs::rename(&staging, &dest)?;
        staging_guard.persist();
        super::store::ensure_exe_alias(&dest, "python.exe", "python3.exe");
        super::store::ensure_exe_alias(&dest, "pythonw.exe", "python3w.exe");

        journal::log_action(
            db,
            "python",
            "install",
            Some(ver.clone()),
            None,
            Some(&serde_json::json!({ "dir": dest.display().to_string() })),
            None,
            true,
        )?;

        if store.current_version().is_none() {
            emit(ProgressEvent::python("install", "link", &ver, "设为 current"));
            store.set_current(db, &ver)?;
        }
        emit(ProgressEvent::python("install", "done", &ver, "安装完成").with_percent(100.0));
        Ok(store.status())
    })
}

pub fn pip_install_version(
    db: &Db,
    paths: &AppPaths,
    py_ver: &str,
    pip_ver: &str,
    mut emit: impl FnMut(ProgressEvent),
) -> Result<String> {
    let store = RuntimeStore::python(paths);
    super::with_runtime_op(&store, || {
        let ver = normalize_version(store.kind, py_ver)?;
        let dir = store.version_dir(&ver)?;
        let python = dir.join("python.exe");
        if !python.is_file() {
            bail!("{} 没有 python.exe", dir.display());
        }
        let pip_ver = pip_ver.trim().trim_start_matches('v').trim_start_matches('V');
        if !super::store::is_tool_version(pip_ver) {
            bail!("非法 pip 版本号");
        }
        let spec = format!("pip=={pip_ver}");
        emit(ProgressEvent::python(
            "pip",
            "pip",
            &ver,
            format!("安装 {spec} 到 {ver}"),
        ));
        let r = crate::winutil::run_cmd_interruptible(
            &python.display().to_string(),
            &["-m", "pip", "install", "--upgrade", &spec],
            Some(&dir),
            Duration::from_secs(180),
            super::is_cancelled,
        );
        journal::log_command(db, paths, "python", "pip_upgrade", &r, None, None, None)?;
        if super::is_cancelled() || r.stderr.contains(super::CANCEL_MSG) {
            bail!("{}", super::CANCEL_MSG);
        }
        if r.ok {
            emit(
                ProgressEvent::python("pip", "done", &ver, format!("pip → {pip_ver}")).with_percent(100.0),
            );
            Ok(r.stdout)
        } else {
            emit(ProgressEvent::python("pip", "error", &ver, r.stderr.clone()));
            bail!(r.stderr)
        }
    })
}

fn is_stable_python(v: &str) -> bool {
    let mut parts = v.split('.');
    let Some(maj) = parts.next() else { return false };
    let Some(min) = parts.next() else { return false };
    let Some(pat) = parts.next() else { return false };
    parts.next().is_none()
        && maj.chars().all(|c| c.is_ascii_digit())
        && min.chars().all(|c| c.is_ascii_digit())
        && pat.chars().all(|c| c.is_ascii_digit())
        && maj != "2"
}

fn cmp_py(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<u32> = a.split('.').filter_map(|p| p.parse().ok()).collect();
    let pb: Vec<u32> = b.split('.').filter_map(|p| p.parse().ok()).collect();
    pa.cmp(&pb)
}
