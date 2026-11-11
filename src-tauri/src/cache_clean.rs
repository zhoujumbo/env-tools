use crate::paths::AppPaths;
use crate::version_mgr::{node, RuntimeStore};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct ToolCache {
    pub id: String,
    pub tool: String,
    pub title: String,
    pub path: String,
    pub exists: bool,
    pub size: u64,
}

pub fn list(app_paths: &AppPaths) -> Vec<ToolCache> {
    let mut out = Vec::new();
    let local = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));

    let npm_cache = npm_cache_path(app_paths).unwrap_or_else(|| local.join("npm-cache"));
    push(&mut out, "npm_cache", "npm", "npm cache", &npm_cache);

    let pnpm = local.join("pnpm");
    push(&mut out, "pnpm_store", "pnpm", "pnpm store", &pnpm.join("store"));
    push(&mut out, "pnpm_cache", "pnpm", "pnpm cache", &pnpm.join("cache"));

    push(&mut out, "yarn_cache", "yarn", "Yarn cache", &local.join("Yarn").join("Cache"));

    let pip = pip_cache_path().unwrap_or_else(|| local.join("pip").join("Cache"));
    push(&mut out, "pip_cache", "pip", "pip cache", &pip);

    let uv = std::env::var_os("UV_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| local.join("uv").join("cache"));
    push(&mut out, "uv_cache", "uv", "uv cache", &uv);

    push(
        &mut out,
        "node_dl",
        "node",
        "面板 Node 安装包缓存",
        &RuntimeStore::node(app_paths).cache_dir(),
    );
    push(
        &mut out,
        "python_dl",
        "python",
        "面板 Python 安装包缓存",
        &RuntimeStore::python(app_paths).cache_dir(),
    );
    push(
        &mut out,
        "java_dl",
        "java",
        "面板 Java 安装包缓存",
        &RuntimeStore::java(app_paths).cache_dir(),
    );
    push(
        &mut out,
        "golang_dl",
        "golang",
        "面板 Golang 安装包缓存",
        &RuntimeStore::golang(app_paths).cache_dir(),
    );

    let prefs = crate::paths::Prefs::load(app_paths);
    if prefs.node_pkg_override && !prefs.node_pkg_root.trim().is_empty() {
        let (_, cache) = node::pkg_dirs(&prefs.node_pkg_root);
        push(&mut out, "node_custom_cache", "npm", "自定义 Node cache", &cache);
    }

    out
}

fn push(out: &mut Vec<ToolCache>, id: &str, tool: &str, title: &str, path: &Path) {
    let exists = path.exists();
    out.push(ToolCache {
        id: id.into(),
        tool: tool.into(),
        title: title.into(),
        path: path.display().to_string(),
        exists,
        size: if exists { dir_size(path) } else { 0 },
    });
}

fn npm_cache_path(app_paths: &AppPaths) -> Option<PathBuf> {
    let npm = RuntimeStore::node(app_paths).current_dir().join("npm.cmd");
    if npm.is_file() {
        let r = crate::winutil::run_cmd_timeout(
            &npm.display().to_string(),
            &["config", "get", "cache", "--location=user"],
            npm.parent(),
            std::time::Duration::from_secs(12),
        );
        if r.ok {
            if let Some(line) = r.stdout.lines().map(str::trim).filter(|s| !s.is_empty() && *s != "undefined").last() {
                return Some(PathBuf::from(line));
            }
        }
    }
    None
}

fn pip_cache_path() -> Option<PathBuf> {
    let py = crate::winutil::which("python").or_else(|| crate::winutil::which("py"))?;
    let r = crate::winutil::run_cmd_timeout(
        &py.display().to_string(),
        &["-m", "pip", "cache", "dir"],
        None,
        std::time::Duration::from_secs(12),
    );
    if r.ok {
        r.stdout
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .last()
            .map(PathBuf::from)
    } else {
        None
    }
}

/// 单个缓存目录最多 statistic 这么多条目。npm cache 常年几十万小文件，
/// 全量走一遍会把「扫缓存」拖成分钟级；超出部分按已扫到的计。
const MAX_SCAN_ENTRIES: usize = 200_000;

fn dir_size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .take(MAX_SCAN_ENTRIES)
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

pub fn clean(app_paths: &AppPaths, id: &str) -> Result<String> {
    let item = list(app_paths)
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| anyhow::anyhow!("未知缓存项 {id}"))?;
    let p = PathBuf::from(&item.path);
    if !p.exists() {
        bail!("{} 不存在", p.display());
    }
    if p.is_file() {
        fs::remove_file(&p).with_context(|| format!("删除 {}", p.display()))?;
    } else {
        fs::remove_dir_all(&p).with_context(|| format!("删除 {}", p.display()))?;
        let _ = fs::create_dir_all(&p);
    }
    Ok(format!("已清理 {} ({})", item.title, item.path))
}
