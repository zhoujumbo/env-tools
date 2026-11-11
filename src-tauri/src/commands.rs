use crate::env_manager::EnvChange;
use crate::hosts_store;
use crate::journal::OpRecord;
use crate::paths::{self, AppPaths, PathsView, Prefs};
use crate::version_mgr::{golang, java, node, python, RuntimeStore};
use crate::winutil;
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Emitter, Manager, State};

fn paths(state: &State<'_, AppState>) -> AppPaths {
    match state.paths.lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

fn map_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
pub fn get_paths(state: State<AppState>) -> PathsView {
    PathsView::from_app(&paths(&state))
}

#[derive(Deserialize)]
pub struct SetPathsArgs {
    pub config_dir: String,
    pub data_dir: String,
    pub log_dir: String,
    pub migrate: bool,
}

#[tauri::command]
pub fn set_paths(state: State<AppState>, args: SetPathsArgs) -> Result<PathsView, String> {
    let old = paths(&state);
    let new = AppPaths {
        config_dir: args.config_dir.into(),
        data_dir: args.data_dir.into(),
        log_dir: args.log_dir.into(),
    };
    for d in [&new.config_dir, &new.data_dir, &new.log_dir] {
        if !paths::dir_writable(d) {
            return Err(format!("目录不可写: {}", d.display()));
        }
    }
    paths::save_paths(&new, args.migrate, &old).map_err(map_err)?;
    state.db.reopen(&new).map_err(map_err)?;
    match state.paths.lock() {
        Ok(mut g) => *g = new.clone(),
        Err(p) => *p.into_inner() = new.clone(),
    }
    crate::journal::log_action(
        &state.db,
        "settings",
        "set_paths",
        None,
        Some(&serde_json::to_value(&old).unwrap_or_default()),
        Some(&serde_json::to_value(&new).unwrap_or_default()),
        None,
        true,
    )
    .map_err(map_err)?;
    Ok(PathsView::from_app(&new))
}

#[tauri::command]
pub fn get_prefs(state: State<AppState>) -> Prefs {
    Prefs::load(&paths(&state))
}

#[derive(Serialize)]
pub struct PrefsSaveResult {
    pub pkg_changed: bool,
    pub detail: String,
    pub npm: Option<node::NpmUserConfig>,
    pub pnpm: Option<node::PnpmEnv>,
}

#[tauri::command]
pub fn set_prefs(state: State<AppState>, prefs: Prefs) -> Result<PrefsSaveResult, String> {
    let p = paths(&state);
    let old = Prefs::load(&p);
    prefs.save(&p).map_err(map_err)?;
    if prefs.node_pkg_override && !prefs.node_pkg_root.trim().is_empty() {
        node::ensure_pkg_dirs(&prefs.node_pkg_root).map_err(map_err)?;
    }
    let pkg_changed = old.node_pkg_override != prefs.node_pkg_override
        || old.node_pkg_root.trim() != prefs.node_pkg_root.trim();
    if pkg_changed {
        let npm = node::sync_npm_config_after_prefs(&state.db, &p, &old).map_err(map_err)?;
        let pnpm = node::probe_pnpm_env(&p);
        let mut bits = Vec::new();
        if !npm.detail.is_empty() {
            bits.push(npm.detail.clone());
        }
        if pnpm.needs_fix {
            bits.push(pnpm.detail.clone());
        } else if !pnpm.installed {
            bits.push("当前布局下没有 pnpm，到 Node 页点「装 pnpm」".into());
        }
        bits.push("已打开的终端不会变，请新开 cmd 再试".into());
        return Ok(PrefsSaveResult {
            pkg_changed: true,
            detail: bits.join("。"),
            npm: Some(npm),
            pnpm: Some(pnpm),
        });
    }
    let (path_changed, path_notes) =
        node::repair_pkg_path(&state.db, &p, None).map_err(map_err)?;
    if path_changed {
        let pnpm = node::probe_pnpm_env(&p);
        let mut bits = vec![path_notes];
        if pnpm.needs_fix {
            bits.push(pnpm.detail.clone());
        }
        bits.push("已打开的终端不会变，请新开 cmd 再试".into());
        return Ok(PrefsSaveResult {
            pkg_changed: false,
            detail: bits.join("。"),
            npm: None,
            pnpm: Some(pnpm),
        });
    }
    Ok(PrefsSaveResult {
        pkg_changed: false,
        detail: String::new(),
        npm: None,
        pnpm: None,
    })
}

#[tauri::command]
pub fn pick_directory(app: AppHandle, title: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app.dialog().file().set_title(&title).blocking_pick_folder();
    Ok(picked.map(|p| p.to_string()))
}

#[tauri::command]
pub async fn run_diagnose(state: State<'_, AppState>) -> Result<crate::diagnose::DiagnoseReport, String> {
    let paths = paths(&state);
    tauri::async_runtime::spawn_blocking(move || crate::diagnose::run(&paths))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn is_first_run(state: State<AppState>) -> bool {
    !paths(&state).first_run_flag().exists()
}

#[tauri::command]
pub fn complete_first_run(state: State<AppState>) -> Result<(), String> {
    let p = paths(&state);
    if !crate::baseline::exists(&p) {
        crate::baseline::capture(&p, &state.db).map_err(map_err)?;
    }
    fs::write(p.first_run_flag(), "ok").map_err(map_err)?;
    hosts_store::ensure_default_entry(&p).map_err(map_err)?;
    Ok(())
}

#[tauri::command]
pub fn recapture_baseline(state: State<AppState>) -> Result<crate::baseline::BaselineManifest, String> {
    crate::baseline::capture(&paths(&state), &state.db).map_err(map_err)
}

#[tauri::command]
pub fn restore_os_surface(state: State<AppState>) -> Result<String, String> {
    crate::baseline::restore_os_surface(&paths(&state), &state.db).map_err(map_err)
}

#[tauri::command]
pub async fn list_ops(
    app: AppHandle,
    kind: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<OpRecord>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state
            .db
            .list(kind.as_deref(), limit.unwrap_or(200))
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 日志预览上限。2MB 纯文本进 webview 会撑出几十 MB 的排版节点，
/// 512KB 对排查够用；超限只返回尾部。
const MAX_PREVIEW_BYTES: u64 = 512 * 1024;

#[tauri::command]
pub async fn read_text_file(app: AppHandle, path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{Read, Seek, SeekFrom};
        let state = app.state::<AppState>();
        let root = paths(&state).log_dir;
        let requested = std::path::PathBuf::from(&path);
        if !paths::is_inside(&root, &requested) {
            return Err("只能读取日志目录内的文件".into());
        }
        let meta = fs::metadata(&requested).map_err(map_err)?;
        if meta.len() <= MAX_PREVIEW_BYTES {
            return fs::read_to_string(requested).map_err(map_err);
        }
        let mut f = fs::File::open(&requested).map_err(map_err)?;
        let skip = meta.len() - MAX_PREVIEW_BYTES;
        f.seek(SeekFrom::Start(skip)).map_err(map_err)?;
        let mut buf = Vec::with_capacity(MAX_PREVIEW_BYTES as usize);
        f.take(MAX_PREVIEW_BYTES).read_to_end(&mut buf).map_err(map_err)?;
        // 从中间切可能切在多字节字符里，先丢到第一个换行
        let text = String::from_utf8_lossy(&buf).into_owned();
        let body = match text.find('\n') {
            Some(i) if skip > 0 => &text[i + 1..],
            _ => text.as_str(),
        };
        Ok(format!(
            "…（前 {} KB 已省略，只显示最后 {} KB）\n{body}",
            skip / 1024,
            MAX_PREVIEW_BYTES / 1024
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_hosts(app: AppHandle) -> Result<(Vec<hosts_store::HostsEntry>, Vec<hosts_store::TrashItem>), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let p = paths(&state);
        Ok((
            hosts_store::load_manifest(&p).entries,
            hosts_store::load_trash(&p).items,
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_hosts_content(app: AppHandle, id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        Ok(hosts_store::entry_content(&paths(&state), &id))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn save_hosts_entry(
    state: State<AppState>,
    id: Option<String>,
    title: String,
    content: String,
    on: bool,
) -> Result<hosts_store::HostsEntry, String> {
    hosts_store::save_entry(&paths(&state), id, title, content, on).map_err(map_err)
}

#[tauri::command]
pub fn set_hosts_on(app: AppHandle, state: State<AppState>, id: String, on: bool) -> Result<(), String> {
    hosts_store::set_on(&paths(&state), &id, on).map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(())
}

#[tauri::command]
pub fn trash_hosts(state: State<AppState>, id: String) -> Result<(), String> {
    hosts_store::trash_entry(&paths(&state), &id).map_err(map_err)
}

#[tauri::command]
pub fn restore_hosts_trash(state: State<AppState>, id: String) -> Result<hosts_store::HostsEntry, String> {
    hosts_store::restore_trash(&paths(&state), &id).map_err(map_err)
}

#[tauri::command]
pub fn purge_hosts_trash(state: State<AppState>, id: Option<String>) -> Result<(), String> {
    hosts_store::purge_trash(&paths(&state), id).map_err(map_err)
}

#[tauri::command]
pub async fn read_system_hosts() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        crate::hosts_apply::write::read_system_hosts().map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn apply_hosts(app: AppHandle, state: State<AppState>) -> Result<crate::hosts_apply::ApplyOutcome, String> {
    let p = paths(&state);
    let content = hosts_store::aggregate_enabled(&p);
    let outcome = crate::hosts_apply::apply_to_system_hosts(&content).map_err(map_err)?;
    let undo = serde_json::json!({
        "type": "hosts",
        "previous_content": outcome.previous_content
    });
    crate::journal::log_action(
        &state.db,
        "hosts",
        "hosts_apply",
        Some("apply aggregated hosts".into()),
        Some(&serde_json::json!({ "previous": outcome.previous_content })),
        Some(&serde_json::json!({ "unchanged": outcome.unchanged })),
        Some(&undo),
        true,
    )
    .map_err(map_err)?;
    let prefs = Prefs::load(&p);
    if prefs.flush_dns_after_hosts {
        let r = winutil::run_cmd_exe("ipconfig /flushdns");
        let _ = crate::journal::log_command(&state.db, &p, "hosts", "flushdns", &r, None, None, None);
    }
    let _ = crate::tray::rebuild_menu(&app);
    Ok(outcome)
}

#[tauri::command]
pub async fn env_snapshot() -> Result<crate::env_manager::EnvSnapshot, String> {
    tauri::async_runtime::spawn_blocking(crate::env_manager::snapshot)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn apply_env_changes(state: State<AppState>, changes: Vec<EnvChange>) -> Result<(), String> {
    let (before, after) = crate::env_manager::apply_user_changes(&changes).map_err(map_err)?;
    for ch in &changes {
        let _ = state.db.add_touched_env_key(&ch.name);
    }
    crate::journal::log_action(
        &state.db,
        "env",
        "apply_user_env",
        Some(format!("{} changes", changes.len())),
        Some(&before),
        Some(&after),
        Some(&serde_json::json!({ "type": "user_env", "before": before })),
        true,
    )
    .map_err(map_err)?;
    Ok(())
}

#[tauri::command]
pub async fn node_status(app: AppHandle) -> Result<crate::version_mgr::RuntimeStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let p = paths(&state);
        let st = RuntimeStore::node(&p).status();
        let env = node::probe_pnpm_env(&p);
        Ok(node::apply_pnpm_probe(st, &env))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn node_list_remote(state: State<'_, AppState>, force: bool) -> Result<node::RemoteList, String> {
    let paths = paths(&state);
    tauri::async_runtime::spawn_blocking(move || node::list_remote(&paths, force))
        .await
        .map_err(|e| e.to_string())?
        .map_err(map_err)
}

#[tauri::command]
pub async fn node_install(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let emitter = handle.clone();
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        node::install(&state.db, &paths, &version, move |ev| {
            let _ = emitter.emit("runtime-progress", ev);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub async fn node_uninstall(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        RuntimeStore::node(&paths).uninstall(&state.db, &version)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub async fn node_set_current(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let p = paths(&state);
        RuntimeStore::node(&p)
            .set_current(&state.db, &version)
            .map_err(map_err)?;
        node::apply_pkg_layout(&state.db, &p, &version).map_err(map_err)?;
        let env = node::probe_pnpm_env(&p);
        Ok::<_, String>(node::apply_pnpm_probe(RuntimeStore::node(&p).status(), &env))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub async fn node_pkg_layout(app: AppHandle) -> Result<node::NodePkgLayout, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        Ok(node::pkg_layout_status(&paths(&state)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn node_npm_config(state: State<'_, AppState>) -> Result<node::NpmUserConfig, String> {
    let paths = paths(&state);
    tauri::async_runtime::spawn_blocking(move || node::npm_user_config(&paths))
        .await
        .map_err(|e| e.to_string())?
        .map_err(map_err)
}

#[tauri::command]
pub async fn node_sync_npm_config(app: AppHandle) -> Result<node::NpmUserConfig, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        node::sync_npm_config(&state.db, &paths)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)
}

#[tauri::command]
pub async fn node_set_registry(app: AppHandle, url: String) -> Result<node::NpmUserConfig, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        node::set_npm_registry(&state.db, &paths, &url)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)
}

#[tauri::command]
pub async fn node_npm_cache_clean(app: AppHandle) -> Result<String, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        node::npm_cache_clean(&state.db, &paths)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)
}

#[tauri::command]
pub fn node_info(state: State<AppState>, version: String) -> serde_json::Value {
    node::node_info(&paths(&state), &version)
}

#[tauri::command]
pub async fn node_npm_install(
    app: AppHandle,
    node_ver: String,
    npm_ver: String,
) -> Result<String, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let emitter = handle.clone();
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        node::npm_install_version(&state.db, &paths, &node_ver, &npm_ver, move |ev| {
            let _ = emitter.emit("runtime-progress", ev);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)
}

#[tauri::command]
pub async fn node_pnpm_install(app: AppHandle, node_ver: String) -> Result<String, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let emitter = handle.clone();
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        node::pnpm_install(&state.db, &paths, &node_ver, move |ev| {
            let _ = emitter.emit("runtime-progress", ev);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)
}

#[tauri::command]
pub fn node_fix_pnpm(state: State<AppState>) -> Result<node::PnpmEnv, String> {
    let p = paths(&state);
    node::fix_pnpm_env(&state.db, &p).map_err(map_err)
}

#[tauri::command]
pub fn runtime_cancel() {
    crate::version_mgr::request_cancel();
}

#[tauri::command]
pub async fn python_status(app: AppHandle) -> Result<crate::version_mgr::RuntimeStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        Ok(RuntimeStore::python(&paths(&state)).status())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn python_list_remote(state: State<'_, AppState>, force: bool) -> Result<python::RemoteList, String> {
    let paths = paths(&state);
    tauri::async_runtime::spawn_blocking(move || python::list_remote(&paths, force))
        .await
        .map_err(|e| e.to_string())?
        .map_err(map_err)
}

#[tauri::command]
pub async fn python_install(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let emitter = handle.clone();
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        python::install(&state.db, &paths, &version, move |ev| {
            let _ = emitter.emit("runtime-progress", ev);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub async fn python_uninstall(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        RuntimeStore::python(&paths).uninstall(&state.db, &version)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub async fn python_set_current(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        RuntimeStore::python(&paths(&state))
            .set_current(&state.db, &version)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub async fn python_fix_cmd(app: AppHandle) -> Result<crate::version_mgr::RuntimeStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let store = RuntimeStore::python(&paths(&state));
        if store.current_version().is_none() {
            return Err("还没有 current Python".into());
        }
        store.ensure_user_path(&state.db).map_err(map_err)?;
        Ok(store.status())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn python_pip_install(
    app: AppHandle,
    py_ver: String,
    pip_ver: String,
) -> Result<String, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let emitter = handle.clone();
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        python::pip_install_version(&state.db, &paths, &py_ver, &pip_ver, move |ev| {
            let _ = emitter.emit("runtime-progress", ev);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)
}

#[tauri::command]
pub async fn java_status(app: AppHandle) -> Result<crate::version_mgr::RuntimeStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        Ok(RuntimeStore::java(&paths(&state)).status())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn java_list_remote(state: State<'_, AppState>, force: bool) -> Result<java::RemoteList, String> {
    let paths = paths(&state);
    tauri::async_runtime::spawn_blocking(move || java::list_remote(&paths, force))
        .await
        .map_err(|e| e.to_string())?
        .map_err(map_err)
}

#[tauri::command]
pub async fn java_install(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let emitter = handle.clone();
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        java::install(&state.db, &paths, &version, move |ev| {
            let _ = emitter.emit("runtime-progress", ev);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub async fn java_uninstall(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        RuntimeStore::java(&paths).uninstall(&state.db, &version)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub fn java_set_current(app: AppHandle, state: State<AppState>, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let st = RuntimeStore::java(&paths(&state))
        .set_current(&state.db, &version)
        .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub fn java_env_report(state: State<AppState>) -> java::JavaEnvReport {
    java::env_report(&paths(&state))
}

#[tauri::command]
pub fn java_claim_env(state: State<AppState>) -> Result<java::JavaEnvReport, String> {
    java::claim_env(&state.db, &paths(&state)).map_err(map_err)
}

#[tauri::command]
pub async fn golang_status(app: AppHandle) -> Result<crate::version_mgr::RuntimeStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        Ok(RuntimeStore::golang(&paths(&state)).status())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn golang_list_remote(state: State<'_, AppState>, force: bool) -> Result<golang::RemoteList, String> {
    let paths = paths(&state);
    tauri::async_runtime::spawn_blocking(move || golang::list_remote(&paths, force))
        .await
        .map_err(|e| e.to_string())?
        .map_err(map_err)
}

#[tauri::command]
pub async fn golang_install(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let emitter = handle.clone();
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        golang::install(&state.db, &paths, &version, move |ev| {
            let _ = emitter.emit("runtime-progress", ev);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub async fn golang_uninstall(app: AppHandle, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let paths = paths(&state);
        RuntimeStore::golang(&paths).uninstall(&state.db, &version)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub fn golang_set_current(app: AppHandle, state: State<AppState>, version: String) -> Result<crate::version_mgr::RuntimeStatus, String> {
    let st = RuntimeStore::golang(&paths(&state))
        .set_current(&state.db, &version)
        .map_err(map_err)?;
    let _ = crate::tray::rebuild_menu(&app);
    Ok(st)
}

#[tauri::command]
pub fn cleanup_legacy_shell_hooks(state: State<AppState>) -> Result<crate::shell_compat::ShellCompatStatus, String> {
    crate::shell_compat::remove(&paths(&state), &state.db, true, true).map_err(map_err)
}

#[tauri::command]
pub fn legacy_shell_status(state: State<AppState>) -> crate::shell_compat::ShellCompatStatus {
    crate::shell_compat::status(&paths(&state))
}

#[tauri::command]
pub fn create_backup(
    state: State<AppState>,
    include_runtime: bool,
    dest_dir: Option<String>,
) -> Result<crate::backup::BackupInfo, String> {
    crate::backup::create(&paths(&state), &state.db, include_runtime, dest_dir).map_err(map_err)
}

#[tauri::command]
pub fn list_backups(state: State<AppState>) -> Vec<crate::backup::BackupInfo> {
    crate::backup::list(&paths(&state))
}

#[tauri::command]
pub fn restore_backup(state: State<AppState>, zip_path: String) -> Result<String, String> {
    crate::backup::restore(&paths(&state), &state.db, &zip_path).map_err(map_err)
}

#[tauri::command]
pub async fn baseline_status(app: AppHandle) -> Result<Option<crate::baseline::BaselineManifest>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        Ok(crate::baseline::load(&paths(&state)).ok().flatten())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn app_memory() -> Result<crate::winutil::AppMemory, String> {
    tauri::async_runtime::spawn_blocking(crate::winutil::app_memory)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeLogFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: Option<i64>,
}

#[tauri::command]
pub async fn list_runtime_logs(app: AppHandle) -> Result<Vec<RuntimeLogFile>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let dir = paths(&state).log_dir;
        let mut out = Vec::new();
        let rd = match fs::read_dir(&dir) {
            Ok(v) => v,
            Err(_) => return Ok(out),
        };
        for e in rd.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified = meta.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs() as i64)
            });
            out.push(RuntimeLogFile {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path().display().to_string(),
                size: meta.len(),
                modified,
            });
        }
        out.sort_by(|a, b| b.modified.cmp(&a.modified));
        if out.len() > 400 {
            out.truncate(400);
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_tool_caches(state: State<'_, AppState>) -> Result<Vec<crate::cache_clean::ToolCache>, String> {
    let paths = paths(&state);
    tauri::async_runtime::spawn_blocking(move || crate::cache_clean::list(&paths))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clean_tool_cache(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let paths = paths(&state);
    tauri::async_runtime::spawn_blocking(move || crate::cache_clean::clean(&paths, &id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(map_err)
}

