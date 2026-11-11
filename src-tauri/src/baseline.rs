use crate::hosts_apply::write as hosts_write;
use crate::journal::{self, Db};
use crate::paths::AppPaths;
use crate::winutil;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineManifest {
    pub captured_at: String,
    pub hosts_path: String,
    pub autorun: Option<String>,
    pub ps5_existed: bool,
    pub ps7_existed: bool,
}

pub fn capture(paths: &AppPaths, db: &Db) -> Result<BaselineManifest> {
    let dir = paths.baseline_dir();
    fs::create_dir_all(&dir)?;

    let hosts_path = hosts_write::windows_system_hosts_path()?;
    let hosts = fs::read_to_string(&hosts_path).unwrap_or_default();
    fs::write(dir.join("hosts"), hosts)?;

    let user_env = winutil::read_env_map(false).unwrap_or_default();
    fs::write(
        dir.join("user_env.json"),
        serde_json::to_string_pretty(&user_env)?,
    )?;

    let autorun = winutil::cmd_autorun_get()?;
    fs::write(
        dir.join("autorun.txt"),
        autorun.clone().unwrap_or_default(),
    )?;

    let ps5 = winutil::ps5_profile_path();
    let ps7 = winutil::ps7_profile_path();
    let ps5_existed = copy_if_exists(&ps5, &dir.join("ps5_profile.ps1"))?;
    let ps7_existed = copy_if_exists(&ps7, &dir.join("ps7_profile.ps1"))?;

    let man = BaselineManifest {
        captured_at: chrono::Local::now().to_rfc3339(),
        hosts_path: hosts_path.display().to_string(),
        autorun,
        ps5_existed,
        ps7_existed,
    };
    fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&man)?)?;
    db.meta_set("baseline_captured_at", &man.captured_at)?;
    Ok(man)
}

fn copy_if_exists(src: &Path, dst: &Path) -> Result<bool> {
    if src.exists() {
        fs::copy(src, dst)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn load(paths: &AppPaths) -> Result<Option<BaselineManifest>> {
    let p = paths.baseline_dir().join("manifest.json");
    if !p.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(p)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

pub fn exists(paths: &AppPaths) -> bool {
    paths.baseline_dir().join("manifest.json").exists()
}

pub fn restore_os_surface(paths: &AppPaths, db: &Db) -> Result<String> {
    let man = load(paths)?.ok_or_else(|| anyhow::anyhow!("尚未采集基线"))?;
    let dir = paths.baseline_dir();
    let mut notes = Vec::new();

    for rec in db.list_with_undo().unwrap_or_default() {
        let Some(raw) = rec.undo_json.as_ref() else { continue };
        let Ok(undo) = serde_json::from_str::<serde_json::Value>(raw) else { continue };
        match undo.get("type").and_then(|t| t.as_str()) {
            Some("user_env") => {
                if let Some(before) = undo.get("before") {
                    let _ = crate::env_manager::undo_env_map(before);
                }
            }
            Some("ps_profile") | Some("autorun") => {
                let _ = crate::shell_compat::undo_from_json(&undo);
            }
            _ => {}
        }
    }

    let hosts = fs::read_to_string(dir.join("hosts")).unwrap_or_default();
    match hosts_write::apply_to_system_hosts(&hosts) {
        Ok(outcome) => {
            notes.push(format!(
                "hosts 已恢复基线 (unchanged={})",
                outcome.unchanged
            ));
            let undo = serde_json::json!({
                "type": "hosts",
                "previous_content": outcome.previous_content
            });
            let _ = journal::log_action(
                db,
                "restore",
                "hosts_restore_baseline",
                Some("restore system hosts from baseline".into()),
                Some(&serde_json::json!({ "previous": outcome.previous_content })),
                Some(&serde_json::json!({ "restored": true })),
                Some(&undo),
                true,
            );
        }
        Err(e) => notes.push(format!("hosts 恢复失败: {e}")),
    }

    let baseline_env: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("user_env.json")).unwrap_or_else(|_| "{}".into()))
            .unwrap_or(serde_json::json!({}));
    let touched = db.touched_env_keys().unwrap_or_default();
    for key in touched {
        let baseline_val = baseline_env.get(&key).and_then(|v| v.as_str());
        let current = winutil::get_user_env(&key).ok().flatten();
        match (baseline_val, current) {
            (Some(b), Some(c)) if b == c => {}
            (Some(b), _) => {
                let _ = winutil::set_user_env(&key, b);
                notes.push(format!("还原用户环境变量 {key}"));
            }
            (None, Some(_)) => {
                let _ = winutil::delete_user_env(&key);
                notes.push(format!("删除本面板写入的用户环境变量 {key}"));
            }
            _ => {}
        }
    }
    winutil::broadcast_environment();

    let baseline_autorun = fs::read_to_string(dir.join("autorun.txt")).ok();
    let baseline_autorun = baseline_autorun
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    winutil::cmd_autorun_set(baseline_autorun.as_deref())?;
    notes.push("已还原 CMD AutoRun".into());

    restore_profile(
        &winutil::ps5_profile_path(),
        &dir.join("ps5_profile.ps1"),
        man.ps5_existed,
        &mut notes,
        "PS5",
    );
    restore_profile(
        &winutil::ps7_profile_path(),
        &dir.join("ps7_profile.ps1"),
        man.ps7_existed,
        &mut notes,
        "PS7",
    );

    crate::shell_compat::strip_all_markers(paths)?;

    let _ = journal::log_action(
        db,
        "restore",
        "restore_os_surface",
        Some("one-click restore to baseline".into()),
        None,
        Some(&serde_json::json!({ "notes": notes })),
        None,
        true,
    );
    Ok(notes.join("\n"))
}

fn restore_profile(
    live: &Path,
    backup: &Path,
    existed: bool,
    notes: &mut Vec<String>,
    label: &str,
) {
    if existed && backup.exists() {
        if let Some(parent) = live.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::copy(backup, live);
        notes.push(format!("{label} profile 已还原基线副本"));
    } else {
        let _ = winutil::strip_marked_block(
            live,
            crate::paths::MARKER_BEGIN,
            crate::paths::MARKER_END,
        );
        notes.push(format!("{label} profile 已去掉本面板标记块"));
    }
}
