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

const GO_MIRROR: &str = "https://mirrors.aliyun.com/golang";
const INDEX_TTL_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteVersion {
    pub version: String,
    /// 次版本系列最新（如 "1.22"）。
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GoAsset {
    version: String,
    file: String,
    lts: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IndexMeta {
    fetched_at: i64,
    etag: Option<String>,
}

pub fn list_remote(paths: &AppPaths, force: bool) -> Result<RemoteList> {
    let store = RuntimeStore::golang(paths);
    store.ensure_dirs()?;
    let cache_path = store.cache_dir().join("golang-index.json");
    let meta_path = store.cache_dir().join("golang-index.meta.json");
    let cached = read_assets_cache(&cache_path, &meta_path);
    let now = chrono::Utc::now().timestamp();

    if !force {
        if let Some((assets, meta)) = &cached {
            if now.saturating_sub(meta.fetched_at) < INDEX_TTL_SECS {
                return Ok(to_remote_list(assets, true, false, Some(meta.fetched_at)));
            }
        }
    }

    match fetch_assets(paths) {
        Ok(assets) => {
            let _ = fs::write(
                &cache_path,
                serde_json::to_vec_pretty(&assets).unwrap_or_default(),
            );
            let _ = write_index_meta(
                &meta_path,
                &IndexMeta {
                    fetched_at: now,
                    etag: None,
                },
            );
            Ok(to_remote_list(&assets, false, false, Some(now)))
        }
        Err(e) => {
            if let Some((assets, meta)) = cached {
                return Ok(to_remote_list(&assets, true, true, Some(meta.fetched_at)));
            }
            Err(e)
        }
    }
}

fn to_remote_list(
    assets: &[GoAsset],
    from_cache: bool,
    stale: bool,
    fetched_at: Option<i64>,
) -> RemoteList {
    RemoteList {
        versions: assets
            .iter()
            .map(|a| RemoteVersion {
                version: a.version.clone(),
                lts: a.lts.clone(),
            })
            .collect(),
        from_cache,
        stale,
        fetched_at,
        ttl_secs: INDEX_TTL_SECS,
    }
}

fn fetch_assets(paths: &AppPaths) -> Result<Vec<GoAsset>> {
    let url = format!("{GO_MIRROR}/");
    let html = download::agent(paths)?
        .get(&url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .into_string()
        .with_context(|| format!("read {url}"))?;

    let mut assets = Vec::new();
    let mut seen = HashSet::new();
    for name in href_files(&html) {
        let Some(ver) = parse_go_windows_zip(&name) else {
            continue;
        };
        if !seen.insert(ver.clone()) {
            continue;
        }
        assets.push(GoAsset {
            version: ver,
            file: name,
            lts: None,
        });
    }
    assets.sort_by(|a, b| cmp_go(&b.version, &a.version));
    tag_latest_minor(&mut assets);
    Ok(assets)
}

fn tag_latest_minor(assets: &mut [GoAsset]) {
    let mut seen = HashSet::new();
    for a in assets.iter_mut() {
        let series = series_of(&a.version);
        if let Some(s) = series {
            if seen.insert(s.clone()) {
                a.lts = Some(s);
            }
        }
    }
}

fn series_of(v: &str) -> Option<String> {
    let mut parts = v.split('.');
    let maj = parts.next()?;
    let min = parts.next()?;
    Some(format!("{maj}.{min}"))
}

fn parse_go_windows_zip(name: &str) -> Option<String> {
    // go1.22.5.windows-amd64.zip
    let rest = name.strip_prefix("go")?.strip_suffix(".windows-amd64.zip")?;
    if rest.is_empty()
        || !rest
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.')
    {
        return None;
    }
    let parts: Vec<&str> = rest.split('.').collect();
    if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    Some(rest.to_string())
}

fn href_files(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in html.split("href=\"").skip(1) {
        let Some(end) = m.find('"') else { continue };
        let href = &m[..end];
        let name = href.rsplit('/').next().unwrap_or(href);
        if name.is_empty() || name == ".." || name.starts_with('?') {
            continue;
        }
        out.push(name.to_string());
    }
    // also catch bare filenames in table cells without quotes variants
    for part in html.split('>') {
        let t = part.split('<').next().unwrap_or("").trim();
        if t.starts_with("go") && t.ends_with(".windows-amd64.zip") {
            out.push(t.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn read_assets_cache(cache: &Path, meta: &Path) -> Option<(Vec<GoAsset>, IndexMeta)> {
    let assets: Vec<GoAsset> = serde_json::from_slice(&fs::read(cache).ok()?).ok()?;
    let meta: IndexMeta = serde_json::from_slice(&fs::read(meta).ok()?).ok()?;
    Some((assets, meta))
}

fn write_index_meta(path: &Path, meta: &IndexMeta) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(meta)?)?;
    Ok(())
}

fn find_asset(paths: &AppPaths, version: &str) -> Result<GoAsset> {
    let store = RuntimeStore::golang(paths);
    let cache_path = store.cache_dir().join("golang-index.json");
    if let Ok(raw) = fs::read(&cache_path) {
        if let Ok(assets) = serde_json::from_slice::<Vec<GoAsset>>(&raw) {
            if let Some(a) = assets.into_iter().find(|a| a.version == version) {
                return Ok(a);
            }
        }
    }
    let assets = fetch_assets(paths)?;
    let _ = fs::write(
        &cache_path,
        serde_json::to_vec_pretty(&assets).unwrap_or_default(),
    );
    assets
        .into_iter()
        .find(|a| a.version == version)
        .ok_or_else(|| anyhow::anyhow!("镜像站没有 Go {version}"))
}

pub fn install(
    db: &Db,
    paths: &AppPaths,
    version: &str,
    mut emit: impl FnMut(ProgressEvent),
) -> Result<super::store::RuntimeStatus> {
    let store = RuntimeStore::golang(paths);
    super::with_runtime_op(&store, || {
        store.ensure_dirs()?;
        let ver = normalize_version(store.kind, version)?;
        let dest = store.version_dir(&ver)?;
        store.assert_dir_under_versions(&dest)?;
        emit(ProgressEvent::golang(
            "install",
            "resolve",
            &ver,
            format!("准备安装 {ver}"),
        ));
        super::check_cancel()?;

        if store.is_complete(&dest) {
            if store.current_version().is_none() {
                emit(ProgressEvent::golang(
                    "install",
                    "link",
                    &ver,
                    "已安装，设为 current",
                ));
                store.set_current(db, &ver)?;
            }
            emit(
                ProgressEvent::golang("install", "done", &ver, "已存在，跳过下载").with_percent(100.0),
            );
            return Ok(store.status());
        }

        let asset = find_asset(paths, &ver)?;
        let url = format!("{GO_MIRROR}/{}", asset.file);
        let zip_path = store.cache_dir().join(&asset.file);
        let pkg_ok = zip_path.is_file()
            && fs::metadata(&zip_path)
                .map(|m| m.len() > 10_000_000)
                .unwrap_or(false);

        if !pkg_ok {
            emit(ProgressEvent::golang(
                "install",
                "download",
                &ver,
                format!("下载 {}", asset.file),
            ));
            download::download_file_progress(paths, &url, &zip_path, &mut |bytes, total| {
                emit(
                    ProgressEvent::golang(
                        "install",
                        "download",
                        &ver,
                        format!("下载 {}", asset.file),
                    )
                    .with_bytes(bytes, total),
                );
            })?;
            super::check_cancel()?;
        } else {
            emit(
                ProgressEvent::golang("install", "download", &ver, "使用本地 zip 缓存")
                    .with_percent(100.0),
            );
        }

        let staging = store.cache_dir().join(format!("{ver}-extract"));
        let staging_guard = download::TempPath::dir(staging.clone());
        emit(ProgressEvent::golang("install", "extract", &ver, "解压"));
        download::extract_zip_progress(&zip_path, &staging, &mut |done, total| {
            let pct = if total > 0 {
                (done as f32 / total as f32) * 100.0
            } else {
                0.0
            };
            emit(
                ProgressEvent::golang("install", "extract", &ver, format!("解压 {done}/{total}"))
                    .with_percent(pct),
            );
        })?;
        super::check_cancel()?;
        download::hoist_single_root(&staging)?;
        if !store.is_complete(&staging) {
            let _ = fs::remove_file(&zip_path);
            bail!("zip 里没有 bin\\go.exe（{ver}）");
        }
        super::check_cancel()?;
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        fs::rename(&staging, &dest)?;
        staging_guard.persist();

        journal::log_action(
            db,
            "golang",
            "install",
            Some(ver.clone()),
            None,
            Some(&serde_json::json!({ "dir": dest.display().to_string(), "url": url })),
            None,
            true,
        )?;

        if store.current_version().is_none() {
            emit(ProgressEvent::golang("install", "link", &ver, "设为 current"));
            store.set_current(db, &ver)?;
        }
        emit(ProgressEvent::golang("install", "done", &ver, "安装完成").with_percent(100.0));
        Ok(store.status())
    })
}

fn cmp_go(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<u32> = a.split('.').filter_map(|p| p.parse().ok()).collect();
    let pb: Vec<u32> = b.split('.').filter_map(|p| p.parse().ok()).collect();
    pa.cmp(&pb)
}
