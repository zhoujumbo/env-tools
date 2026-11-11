use crate::journal::{self, Db};
use crate::paths::{paths_json_path, AppPaths};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub created_at: String,
}

pub fn list(paths: &AppPaths) -> Vec<BackupInfo> {
    let mut v = Vec::new();
    let dir = paths.backups_dir();
    let rd = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return v,
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("zip") {
            continue;
        }
        let meta = e.metadata().ok();
        v.push(BackupInfo {
            name: p.file_name().unwrap().to_string_lossy().into_owned(),
            path: p.display().to_string(),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            created_at: meta
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .to_rfc3339()
                })
                .unwrap_or_default(),
        });
    }
    v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    v
}

pub fn create(paths: &AppPaths, db: &Db, include_runtime: bool, dest_dir: Option<String>) -> Result<BackupInfo> {
    let dest = paths.backups_dir();
    if let Some(raw) = dest_dir {
        let requested = PathBuf::from(raw);
        if requested != dest && !crate::paths::is_inside(&dest, &requested) {
            anyhow::bail!("备份只能写到 backups 目录");
        }
    }
    fs::create_dir_all(&dest)?;
    let name = format!("envdesk-backup-{}.zip", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let zip_path = dest.join(&name);
    let file = fs::File::create(&zip_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    add_file(&mut zip, &paths_json_path(), "paths.json", opts)?;
    add_dir(&mut zip, &paths.config_dir, "config", opts)?;
    add_dir_filtered(&mut zip, &paths.data_dir, "data", opts, &["backups"])?;

    let prefs = crate::paths::Prefs::load(paths);
    if include_runtime {
        let rt = paths.runtimes_dir();
        if rt.exists() {
            add_dir_filtered(&mut zip, &rt, "runtimes", opts, &[".cache"])?;
        }
    }

    zip.finish()?;
    prune(paths, prefs.backup_keep.max(1))?;
    let info = BackupInfo {
        name,
        path: zip_path.display().to_string(),
        size: fs::metadata(&zip_path)?.len(),
        created_at: chrono::Local::now().to_rfc3339(),
    };
    journal::log_action(
        db,
        "backup",
        "create_backup",
        Some(info.path.clone()),
        None,
        Some(&serde_json::json!({ "path": info.path })),
        None,
        true,
    )?;
    Ok(info)
}

fn add_file(
    zip: &mut zip::ZipWriter<fs::File>,
    src: &Path,
    name: &str,
    opts: SimpleFileOptions,
) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    zip.start_file(name.replace('\\', "/"), opts)?;
    let mut f = fs::File::open(src)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    zip.write_all(&buf)?;
    Ok(())
}

fn add_dir(
    zip: &mut zip::ZipWriter<fs::File>,
    src: &Path,
    prefix: &str,
    opts: SimpleFileOptions,
) -> Result<()> {
    add_dir_filtered(zip, src, prefix, opts, &[])
}

fn add_dir_filtered(
    zip: &mut zip::ZipWriter<fs::File>,
    src: &Path,
    prefix: &str,
    opts: SimpleFileOptions,
    skip_names: &[&str],
) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let rel = match entry.path().strip_prefix(src) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if rel
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .map(|n| skip_names.iter().any(|s| s.eq_ignore_ascii_case(n)))
            .unwrap_or(false)
        {
            continue;
        }
        if entry.file_type().is_file() {
            let name = format!("{}/{}", prefix, rel.to_string_lossy()).replace('\\', "/");
            add_file(zip, entry.path(), &name, opts)?;
        }
    }
    Ok(())
}

fn prune(paths: &AppPaths, keep: u32) -> Result<()> {
    let mut items = list(paths);
    if items.len() <= keep as usize {
        return Ok(());
    }
    items.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let drop_n = items.len() - keep as usize;
    for i in items.iter().take(drop_n) {
        let _ = fs::remove_file(&i.path);
    }
    Ok(())
}

pub fn restore(paths: &AppPaths, db: &Db, zip_path: &str) -> Result<String> {
    let zip = PathBuf::from(zip_path);
    if !crate::paths::is_inside(&paths.backups_dir(), &zip) {
        anyhow::bail!("只能还原 backups 目录中的 zip");
    }
    let _ = create(paths, db, false, None);
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let tmp = paths.data_dir.join(format!("restore-{}", crate::winutil::now_stamp()));
    fs::create_dir_all(&tmp)?;
    archive.extract(&tmp)?;

    let cfg_src = tmp.join("config");
    if cfg_src.exists() {
        copy_over(&cfg_src, &paths.config_dir)?;
    }
    let data_src = tmp.join("data");
    if data_src.exists() {
        copy_over(&data_src, &paths.data_dir)?;
    }
    let _ = fs::remove_dir_all(&tmp);
    journal::log_action(
        db,
        "backup",
        "restore_backup",
        Some(zip_path.into()),
        None,
        Some(&serde_json::json!({ "zip": zip_path })),
        None,
        true,
    )?;
    Ok("备份已还原到当前数据/配置目录。根目录（ENVT_PATH / %APPDATA%\\envts）未改动。".into())
}

fn copy_over(src: &Path, dst: &Path) -> Result<()> {
    for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let rel = match entry.path().strip_prefix(src) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(rel);
        if rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            continue;
        }
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
