use crate::journal::{self, Db};
use crate::paths::{AppPaths, MARKER_BEGIN, MARKER_END};
use crate::winutil;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

const PS_BODY: &str =
    "fnm env --use-on-cd --shell powershell | Out-String | Invoke-Expression";

const FNM_INIT_CMD: &str = r#"@echo off
where fnm >nul 2>&1
if %ERRORLEVEL% NEQ 0 exit /b 0
if not defined FNM_AUTORUN_GUARD (
  set "FNM_AUTORUN_GUARD=AutorunGuard"
  FOR /f "tokens=*" %%z IN ('fnm env --use-on-cd --shell cmd') DO CALL %%z
)
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellCompatStatus {
    pub ps5_path: String,
    pub ps5_injected: bool,
    pub ps5_exists: bool,
    pub ps7_path: String,
    pub ps7_injected: bool,
    pub ps7_exists: bool,
    pub execution_policy: String,
    pub cmd_init_path: String,
    pub cmd_init_exists: bool,
    pub cmd_autorun: Option<String>,
    pub cmd_autorun_points_to_us: bool,
}

pub fn status(paths: &AppPaths) -> ShellCompatStatus {
    let ps5 = winutil::ps5_profile_path();
    let ps7 = winutil::ps7_profile_path();
    let init = paths.fnm_init_cmd();
    let autorun = winutil::cmd_autorun_get().ok().flatten();
    let points = autorun
        .as_deref()
        .map(|v| {
            v.to_lowercase()
                .contains(&init.display().to_string().to_lowercase())
        })
        .unwrap_or(false);
    ShellCompatStatus {
        ps5_path: ps5.display().to_string(),
        ps5_injected: winutil::file_contains_marker(&ps5, MARKER_BEGIN),
        ps5_exists: ps5.exists(),
        ps7_path: ps7.display().to_string(),
        ps7_injected: winutil::file_contains_marker(&ps7, MARKER_BEGIN),
        ps7_exists: ps7.exists(),
        execution_policy: winutil::execution_policy(),
        cmd_init_path: init.display().to_string(),
        cmd_init_exists: init.exists(),
        cmd_autorun: autorun,
        cmd_autorun_points_to_us: points,
    }
}

#[allow(dead_code)]
pub fn inject(
    paths: &AppPaths,
    db: &Db,
    powershell: bool,
    cmd: bool,
) -> Result<ShellCompatStatus> {
    if powershell {
        for p in [winutil::ps5_profile_path(), winutil::ps7_profile_path()] {
            let before = winutil::upsert_marked_block(&p, MARKER_BEGIN, MARKER_END, PS_BODY)?;
            journal::log_action(
                db,
                "shell",
                "ps_profile_inject",
                Some(p.display().to_string()),
                Some(&serde_json::json!({ "content": before })),
                Some(&serde_json::json!({ "injected": true, "path": p.display().to_string() })),
                Some(&serde_json::json!({ "type": "ps_profile", "path": p.display().to_string(), "previous_content": before })),
                true,
            )?;
        }
    }
    if cmd {
        fs::create_dir_all(paths.shell_dir())?;
        fs::write(paths.fnm_init_cmd(), FNM_INIT_CMD)?;
        let prev = winutil::cmd_autorun_get()?;
        let init = paths.fnm_init_cmd();
        let quoted = format!("\"{}\"", init.display());
        let new_val = match &prev {
            Some(v) if v.to_lowercase().contains(&init.display().to_string().to_lowercase()) => {
                v.clone()
            }
            Some(v) if !v.trim().is_empty() => format!("{v} & {quoted}"),
            _ => quoted,
        };
        winutil::cmd_autorun_set(Some(&new_val))?;
        journal::log_action(
            db,
            "shell",
            "cmd_autorun_inject",
            Some(new_val.clone()),
            Some(&serde_json::json!({ "autorun": prev })),
            Some(&serde_json::json!({ "autorun": new_val })),
            Some(&serde_json::json!({ "type": "autorun", "previous": prev })),
            true,
        )?;
    }
    Ok(status(paths))
}

pub fn remove(paths: &AppPaths, db: &Db, powershell: bool, cmd: bool) -> Result<ShellCompatStatus> {
    if powershell {
        for p in [winutil::ps5_profile_path(), winutil::ps7_profile_path()] {
            let before = winutil::strip_marked_block(&p, MARKER_BEGIN, MARKER_END)?;
            journal::log_action(
                db,
                "shell",
                "ps_profile_strip",
                Some(p.display().to_string()),
                Some(&serde_json::json!({ "content": before })),
                Some(&serde_json::json!({ "stripped": true })),
                Some(&serde_json::json!({ "type": "ps_profile", "path": p.display().to_string(), "previous_content": before })),
                true,
            )?;
        }
    }
    if cmd {
        let prev = winutil::cmd_autorun_get()?;
        if let Some(v) = &prev {
            let init = paths.fnm_init_cmd().display().to_string();
            let quoted = format!("\"{init}\"");
            let mut next = v.replace(&format!(" & {quoted}"), "");
            next = next.replace(&quoted, "");
            next = next.replace(&init, "");
            next = next
                .trim()
                .trim_start_matches('&')
                .trim_end_matches('&')
                .trim()
                .to_string();
            winutil::cmd_autorun_set(if next.is_empty() { None } else { Some(&next) })?;
            journal::log_action(
                db,
                "shell",
                "cmd_autorun_strip",
                Some(next.clone()),
                Some(&serde_json::json!({ "autorun": prev })),
                Some(&serde_json::json!({ "autorun": next })),
                Some(&serde_json::json!({ "type": "autorun", "previous": prev })),
                true,
            )?;
        }
    }
    Ok(status(paths))
}

pub fn strip_all_markers(paths: &AppPaths) -> Result<()> {
    let _ = winutil::strip_marked_block(&winutil::ps5_profile_path(), MARKER_BEGIN, MARKER_END);
    let _ = winutil::strip_marked_block(&winutil::ps7_profile_path(), MARKER_BEGIN, MARKER_END);
    if let Some(v) = winutil::cmd_autorun_get()? {
        let init = paths.fnm_init_cmd().display().to_string();
        let quoted = format!("\"{init}\"");
        let mut next = v.replace(&format!(" & {quoted}"), "");
        next = next.replace(&quoted, "");
        next = next.replace(&init, "");
        next = next
            .trim()
            .trim_start_matches('&')
            .trim_end_matches('&')
            .trim()
            .to_string();
        winutil::cmd_autorun_set(if next.is_empty() { None } else { Some(&next) })?;
    }
    Ok(())
}

pub fn undo_from_json(undo: &serde_json::Value) -> Result<()> {
    match undo.get("type").and_then(|t| t.as_str()) {
        Some("ps_profile") => {
            if let (Some(path), Some(prev)) = (
                undo.get("path").and_then(|v| v.as_str()),
                undo.get("previous_content").and_then(|v| v.as_str()),
            ) {
                let p = std::path::Path::new(path);
                if prev.is_empty() {
                    let _ = std::fs::remove_file(p);
                } else {
                    if let Some(parent) = p.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(p, prev)?;
                }
            }
        }
        Some("autorun") => {
            let prev = undo.get("previous").and_then(|v| v.as_str());
            winutil::cmd_autorun_set(prev)?;
        }
        _ => {}
    }
    Ok(())
}
