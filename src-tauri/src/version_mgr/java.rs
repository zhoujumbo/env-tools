use super::download;
use super::progress::ProgressEvent;
use super::store::{normalize_version, RuntimeStore};
use crate::journal::{self, Db};
use crate::paths::AppPaths;
use crate::winutil::{self, SysEnvTask};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const ADOPTIUM: &str = "https://mirrors.tuna.tsinghua.edu.cn/Adoptium";
const INDEX_TTL_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteVersion {
    pub version: String,
    /// 主版本号（如 "21"），每个 major 最新补丁会带上。
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
struct JavaAsset {
    version: String,
    major: u32,
    file: String,
    lts: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IndexMeta {
    fetched_at: i64,
    etag: Option<String>,
}

pub fn list_remote(paths: &AppPaths, force: bool) -> Result<RemoteList> {
    let store = RuntimeStore::java(paths);
    store.ensure_dirs()?;
    let cache_path = store.cache_dir().join("adoptium-index.json");
    let meta_path = store.cache_dir().join("adoptium-index.meta.json");
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
    assets: &[JavaAsset],
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

fn fetch_assets(paths: &AppPaths) -> Result<Vec<JavaAsset>> {
    let html = fetch_text(paths, &format!("{ADOPTIUM}/"))?;
    let majors = parse_major_dirs(&html);
    let mut assets = Vec::new();
    for major in majors {
        let url = format!("{ADOPTIUM}/{major}/jdk/x64/windows/");
        let page = match fetch_text(paths, &url) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Adoptium {major} windows listing: {e}");
                continue;
            }
        };
        for (version, file) in parse_jdk_zips(&page) {
            assets.push(JavaAsset {
                version,
                major,
                file,
                lts: None,
            });
        }
    }
    assets.sort_by(|a, b| cmp_java(&b.version, &a.version));
    tag_latest_major(&mut assets);
    Ok(assets)
}

fn tag_latest_major(assets: &mut [JavaAsset]) {
    let mut seen = HashSet::new();
    for a in assets.iter_mut() {
        if seen.insert(a.major) {
            a.lts = Some(a.major.to_string());
        }
    }
}

fn parse_major_dirs(html: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for cap in href_dirs(html) {
        if matches!(cap.as_str(), "deb" | "rpm") {
            continue;
        }
        if let Ok(n) = cap.parse::<u32>() {
            if n >= 8 {
                out.push(n);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn parse_jdk_zips(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for name in href_files(html) {
        if !name.ends_with(".zip") || !name.contains("jdk_x64_windows_hotspot") {
            continue;
        }
        if let Some(ver) = parse_temurin_version(&name) {
            out.push((ver, name));
        }
    }
    out
}

fn parse_temurin_version(file: &str) -> Option<String> {
    // OpenJDK21U-jdk_x64_windows_hotspot_21.0.13_9.zip
    if let Some(rest) = file
        .strip_prefix("OpenJDK")
        .and_then(|s| s.split_once("U-jdk_x64_windows_hotspot_"))
        .map(|(_, rest)| rest)
        .and_then(|s| s.strip_suffix(".zip"))
    {
        return Some(rest.to_string());
    }
    None
}

fn href_dirs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in html.split("href=\"").skip(1) {
        let Some(end) = m.find('"') else { continue };
        let href = &m[..end];
        let name = href.trim_end_matches('/');
        if name.is_empty() || name == ".." || name.contains('/') || name.starts_with('?') {
            continue;
        }
        if href.ends_with('/') {
            out.push(name.to_string());
        }
    }
    out
}

fn href_files(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in html.split("href=\"").skip(1) {
        let Some(end) = m.find('"') else { continue };
        let href = &m[..end];
        if href.ends_with('/') || href.starts_with('?') || href.contains('/') {
            continue;
        }
        if !href.is_empty() && href != ".." {
            out.push(href.to_string());
        }
    }
    out
}

fn fetch_text(paths: &AppPaths, url: &str) -> Result<String> {
    let resp = download::agent(paths)?
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    if resp.status() >= 400 {
        bail!("HTTP {} from {url}", resp.status());
    }
    resp.into_string().with_context(|| format!("read {url}"))
}

fn read_assets_cache(cache: &Path, meta: &Path) -> Option<(Vec<JavaAsset>, IndexMeta)> {
    let assets: Vec<JavaAsset> = serde_json::from_slice(&fs::read(cache).ok()?).ok()?;
    let meta: IndexMeta = serde_json::from_slice(&fs::read(meta).ok()?).ok()?;
    Some((assets, meta))
}

fn write_index_meta(path: &Path, meta: &IndexMeta) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(meta)?)?;
    Ok(())
}

fn find_asset(paths: &AppPaths, version: &str) -> Result<JavaAsset> {
    let store = RuntimeStore::java(paths);
    let cache_path = store.cache_dir().join("adoptium-index.json");
    if let Ok(raw) = fs::read(&cache_path) {
        if let Ok(assets) = serde_json::from_slice::<Vec<JavaAsset>>(&raw) {
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
        .ok_or_else(|| anyhow::anyhow!("镜像站没有 Java {version}"))
}

pub fn install(
    db: &Db,
    paths: &AppPaths,
    version: &str,
    mut emit: impl FnMut(ProgressEvent),
) -> Result<super::store::RuntimeStatus> {
    let store = RuntimeStore::java(paths);
    super::with_runtime_op(&store, || {
        store.ensure_dirs()?;
        let ver = normalize_version(store.kind, version)?;
        let dest = store.version_dir(&ver)?;
        store.assert_dir_under_versions(&dest)?;
        emit(ProgressEvent::java(
            "install",
            "resolve",
            &ver,
            format!("准备安装 {ver}"),
        ));
        super::check_cancel()?;

        if store.is_complete(&dest) {
            if store.current_version().is_none() {
                emit(ProgressEvent::java(
                    "install",
                    "link",
                    &ver,
                    "已安装，设为 current",
                ));
                store.set_current(db, &ver)?;
            }
            emit(
                ProgressEvent::java("install", "done", &ver, "已存在，跳过下载").with_percent(100.0),
            );
            return Ok(store.status());
        }

        let asset = find_asset(paths, &ver)?;
        let url = format!("{ADOPTIUM}/{}/jdk/x64/windows/{}", asset.major, asset.file);
        let zip_path = store.cache_dir().join(&asset.file);
        let pkg_ok = zip_path.is_file()
            && fs::metadata(&zip_path)
                .map(|m| m.len() > 10_000_000)
                .unwrap_or(false);

        if !pkg_ok {
            emit(ProgressEvent::java(
                "install",
                "download",
                &ver,
                format!("下载 {}", asset.file),
            ));
            download::download_file_progress(paths, &url, &zip_path, &mut |bytes, total| {
                emit(
                    ProgressEvent::java("install", "download", &ver, format!("下载 {}", asset.file))
                        .with_bytes(bytes, total),
                );
            })?;
            super::check_cancel()?;
        } else {
            emit(
                ProgressEvent::java("install", "download", &ver, "使用本地 zip 缓存")
                    .with_percent(100.0),
            );
        }

        let staging = store.cache_dir().join(format!("{ver}-extract"));
        let staging_guard = download::TempPath::dir(staging.clone());
        emit(ProgressEvent::java("install", "extract", &ver, "解压"));
        download::extract_zip_progress(&zip_path, &staging, &mut |done, total| {
            let pct = if total > 0 {
                (done as f32 / total as f32) * 100.0
            } else {
                0.0
            };
            emit(
                ProgressEvent::java("install", "extract", &ver, format!("解压 {done}/{total}"))
                    .with_percent(pct),
            );
        })?;
        super::check_cancel()?;
        download::hoist_single_root(&staging)?;
        if !store.is_complete(&staging) {
            let _ = fs::remove_file(&zip_path);
            bail!("zip 里没有 bin\\java.exe（{ver}）");
        }
        super::check_cancel()?;
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        fs::rename(&staging, &dest)?;
        staging_guard.persist();

        journal::log_action(
            db,
            "java",
            "install",
            Some(ver.clone()),
            None,
            Some(&serde_json::json!({ "dir": dest.display().to_string(), "url": url })),
            None,
            true,
        )?;

        if store.current_version().is_none() {
            emit(ProgressEvent::java("install", "link", &ver, "设为 current"));
            store.set_current(db, &ver)?;
        }
        emit(ProgressEvent::java("install", "done", &ver, "安装完成").with_percent(100.0));
        Ok(store.status())
    })
}

fn cmp_java(a: &str, b: &str) -> std::cmp::Ordering {
    // 8u512b09 vs 21.0.13_9
    let pa = parse_java_parts(a);
    let pb = parse_java_parts(b);
    pa.cmp(&pb)
}

fn parse_java_parts(s: &str) -> Vec<u32> {
    if let Some(rest) = s.strip_prefix("8u") {
        let mut parts = vec![8];
        let digits: String = rest
            .chars()
            .map(|c| if c.is_ascii_digit() { c } else { ' ' })
            .collect();
        parts.extend(digits.split_whitespace().filter_map(|p| p.parse::<u32>().ok()));
        return parts;
    }
    s.split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse().ok())
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaEnvReport {
    pub current: Option<String>,
    pub current_home: Option<String>,
    pub current_bin: Option<String>,
    pub user_java_home: Option<String>,
    pub machine_java_home: Option<String>,
    pub user_path_hits: Vec<String>,
    pub machine_path_hits: Vec<String>,
    pub foreign_install_hint: Option<String>,
    pub needs_claim: bool,
}

pub fn env_report(paths: &AppPaths) -> JavaEnvReport {
    let store = RuntimeStore::java(paths);
    let current = store.current_version();
    let home = store.current_dir();
    let bin = home.join("bin");
    let home_s = winutil::strip_verbatim_prefix(&home.display().to_string());
    let bin_s = winutil::strip_verbatim_prefix(&bin.display().to_string());

    let user_java_home = winutil::get_user_env("JAVA_HOME").ok().flatten();
    let machine_java_home = winutil::get_system_env("JAVA_HOME").ok().flatten();
    let user_path = winutil::get_user_env("Path").ok().flatten().unwrap_or_default();
    let machine_path = winutil::get_system_env("Path").ok().flatten().unwrap_or_default();

    let user_path_hits = collect_java_path_hits(&user_path, &bin_s);
    let machine_path_hits = collect_java_path_hits(&machine_path, &bin_s);

    let foreign_home = machine_java_home
        .as_ref()
        .filter(|h| !winutil::path_eq(h, &home_s) && !is_under_our_runtime(h, &home_s));
    let user_home_bad = user_java_home
        .as_ref()
        .map(|h| !winutil::path_eq(h, &home_s))
        .unwrap_or(current.is_some());

    let foreign_install_hint = foreign_home.cloned().or_else(|| {
        machine_path_hits
            .iter()
            .chain(user_path_hits.iter())
            .find(|p| {
                let e = winutil::strip_verbatim_prefix(p).to_ascii_lowercase();
                e.contains(r"programdata\java") || e.contains("openlogic") || e.contains("openjdk")
            })
            .cloned()
    });

    let needs_claim = current.is_some()
        && (foreign_home.is_some()
            || user_home_bad
            || !user_path_hits.is_empty()
            || !machine_path_hits.is_empty()
            || user_java_home
                .as_ref()
                .is_some_and(|h| h.contains(r"\\?\")));

    JavaEnvReport {
        current_home: if current.is_some() { Some(home_s) } else { None },
        current_bin: if current.is_some() { Some(bin_s) } else { None },
        current,
        user_java_home,
        machine_java_home,
        user_path_hits,
        machine_path_hits,
        foreign_install_hint,
        needs_claim,
    }
}

fn is_under_our_runtime(path: &str, our_home: &str) -> bool {
    let p = winutil::strip_verbatim_prefix(path).to_ascii_lowercase();
    let ours = winutil::strip_verbatim_prefix(our_home).to_ascii_lowercase();
    p.starts_with(&ours) || p.contains(r"envts\runtimes\java")
}

fn collect_java_path_hits(path: &str, our_bin: &str) -> Vec<String> {
    path.split(';')
        .filter(|p| !p.is_empty())
        .filter(|p| looks_like_java_bin(p) && !winutil::path_eq(p, our_bin))
        .map(|s| s.to_string())
        .collect()
}

fn looks_like_java_bin(entry: &str) -> bool {
    let e = winutil::strip_verbatim_prefix(entry)
        .replace('/', "\\")
        .to_ascii_lowercase();
    if !e.ends_with("\\bin") {
        return false;
    }
    e.contains("jdk")
        || e.contains("jre")
        || e.contains("openjdk")
        || e.contains("temurin")
        || e.contains("adoptium")
        || e.contains("graalvm")
        || e.contains(r"\java\")
        || e.contains(r"envts\runtimes\java")
        || e.contains("openlogic")
}

/// 把 Java 环境收归本面板：用户级立刻改；系统级弹 UAC 清理。
pub fn claim_env(db: &Db, paths: &AppPaths) -> Result<JavaEnvReport> {
    let store = RuntimeStore::java(paths);
    if store.current_version().is_none() {
        bail!("还没有 current Java，请先安装并设为 current");
    }
    store.ensure_user_path(db)?;

    let report_before = env_report(paths);
    let home = report_before
        .current_home
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing current home"))?;
    let bin = report_before
        .current_bin
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing current bin"))?;

    // 用户 PATH：去掉其它 Java bin，确保 current\bin 置顶
    let user_path = winutil::get_user_env("Path")?.unwrap_or_default();
    let mut remove = report_before.user_path_hits.clone();
    // 也清掉我们自己的 \\?\versions\..\bin 之类
    for p in user_path.split(';') {
        if p.is_empty() {
            continue;
        }
        if looks_like_java_bin(p) && !winutil::path_eq(p, &bin) {
            remove.push(p.to_string());
        }
    }
    remove.sort();
    remove.dedup();
    let cleaned = winutil::path_without_entries(&user_path, &remove);
    let next = if cleaned
        .split(';')
        .any(|p| winutil::path_eq(p, &bin))
    {
        // 已存在则 prepend 一下保证最前
        let without = winutil::path_without_entries(&cleaned, &[bin.clone()]);
        if without.is_empty() {
            bin.clone()
        } else {
            format!("{bin};{without}")
        }
    } else if cleaned.is_empty() {
        bin.clone()
    } else {
        format!("{bin};{cleaned}")
    };
    if next != user_path {
        winutil::set_user_env("Path", &next)?;
        db.add_touched_env_key("Path")?;
    }
    winutil::prepend_process_path(&bin);

    // 用户 JAVA_HOME → current
    let before_home = winutil::get_user_env("JAVA_HOME")?;
    if before_home.as_deref() != Some(home.as_str()) {
        winutil::set_user_env("JAVA_HOME", &home)?;
        db.add_touched_env_key("JAVA_HOME")?;
    }
    std::env::set_var("JAVA_HOME", &home);

    // 系统级：清 Machine JAVA_HOME + 外站 Path
    let mut delete_machine_vars = Vec::new();
    if let Some(mh) = &report_before.machine_java_home {
        if !winutil::path_eq(mh, &home) {
            delete_machine_vars.push("JAVA_HOME".into());
        }
    }
    let mut remove_machine_path = report_before.machine_path_hits.clone();
    // Machine Path 里我们的 \\?\versions 残留也清掉
    if let Some(mp) = winutil::get_system_env("Path")? {
        for p in mp.split(';') {
            if p.is_empty() {
                continue;
            }
            if looks_like_java_bin(p) && !winutil::path_eq(p, &bin) {
                remove_machine_path.push(p.to_string());
            }
        }
    }
    remove_machine_path.sort();
    remove_machine_path.dedup();

    if !delete_machine_vars.is_empty() || !remove_machine_path.is_empty() {
        let task = SysEnvTask {
            delete_machine_vars,
            remove_machine_path,
        };
        // 先试直接写；失败再 UAC
        match try_apply_machine(&task) {
            Ok(()) => {}
            Err(_) => winutil::elevate_sysenv_task(&task)?,
        }
    }

    winutil::broadcast_environment();
    journal::log_action(
        db,
        "java",
        "claim_env",
        Some(home),
        Some(&serde_json::json!({
            "machine_java_home": report_before.machine_java_home,
            "machine_path_hits": report_before.machine_path_hits,
        })),
        Some(&serde_json::to_value(env_report(paths)).unwrap_or_default()),
        None,
        true,
    )?;
    Ok(env_report(paths))
}

fn try_apply_machine(task: &SysEnvTask) -> Result<()> {
    for name in &task.delete_machine_vars {
        winutil::delete_system_env(name)?;
    }
    if !task.remove_machine_path.is_empty() {
        let current = winutil::get_system_env("Path")?.unwrap_or_default();
        let next = winutil::path_without_entries(&current, &task.remove_machine_path);
        if next != current {
            winutil::set_system_env("Path", &next)?;
        }
    }
    Ok(())
}
