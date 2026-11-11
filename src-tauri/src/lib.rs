mod backup;
mod baseline;
mod cache_clean;
mod commands;
mod diagnose;
mod env_manager;
mod hosts_apply;
mod hosts_store;
mod journal;
mod paths;
mod shell_compat;
mod tray;
mod version_mgr;
mod winutil;

use journal::Db;
use paths::AppPaths;
use std::sync::Mutex;
use tauri::Manager;

pub struct AppState {
    pub paths: Mutex<AppPaths>,
    pub db: Db,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if hosts_apply::elevation::maybe_run_as_elevation_helper() {
        return;
    }

    let app_paths = paths::load_or_init().expect("failed to init data directories");
    init_file_log(&app_paths.log_dir);
    let db = Db::open(&app_paths).expect("failed to open sqlite journal");

    // 只做纯本地、毫秒级的清理；PATH / npm / pnpm 的核对全部挪到 setup 之后，
    // 不挡托盘和窗口出现。
    for store in [
        crate::version_mgr::RuntimeStore::node(&app_paths),
        crate::version_mgr::RuntimeStore::python(&app_paths),
        crate::version_mgr::RuntimeStore::java(&app_paths),
        crate::version_mgr::RuntimeStore::golang(&app_paths),
    ] {
        store.cleanup_leftovers();
    }

    let state = AppState {
        paths: Mutex::new(app_paths),
        db,
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_paths,
            commands::set_paths,
            commands::get_prefs,
            commands::set_prefs,
            commands::pick_directory,
            commands::run_diagnose,
            commands::is_first_run,
            commands::complete_first_run,
            commands::recapture_baseline,
            commands::restore_os_surface,
            commands::list_ops,
            commands::read_text_file,
            commands::list_hosts,
            commands::get_hosts_content,
            commands::save_hosts_entry,
            commands::set_hosts_on,
            commands::trash_hosts,
            commands::restore_hosts_trash,
            commands::purge_hosts_trash,
            commands::read_system_hosts,
            commands::apply_hosts,
            commands::env_snapshot,
            commands::apply_env_changes,
            commands::node_status,
            commands::node_list_remote,
            commands::node_install,
            commands::node_uninstall,
            commands::node_set_current,
            commands::node_info,
            commands::node_npm_install,
            commands::node_pnpm_install,
            commands::node_fix_pnpm,
            commands::runtime_cancel,
            commands::python_status,
            commands::python_list_remote,
            commands::python_install,
            commands::python_uninstall,
            commands::python_set_current,
            commands::python_pip_install,
            commands::python_fix_cmd,
            commands::java_status,
            commands::java_list_remote,
            commands::java_install,
            commands::java_uninstall,
            commands::java_set_current,
            commands::golang_status,
            commands::golang_list_remote,
            commands::golang_install,
            commands::golang_uninstall,
            commands::golang_set_current,
            commands::cleanup_legacy_shell_hooks,
            commands::legacy_shell_status,
            commands::create_backup,
            commands::list_backups,
            commands::restore_backup,
            commands::baseline_status,
            commands::app_memory,
            commands::list_runtime_logs,
            commands::node_pkg_layout,
            commands::node_npm_config,
            commands::node_sync_npm_config,
            commands::node_set_registry,
            commands::node_npm_cache_clean,
            commands::list_tool_caches,
            commands::clean_tool_cache,
        ])
        .setup(|app| {
            tray::setup(app.handle())?;
            let handle = app.handle().clone();
            let _ = tauri::async_runtime::spawn_blocking(move || startup_maintenance(&handle));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building envdesk")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                if !tray::QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
                    api.prevent_exit();
                }
            }
            tauri::RunEvent::WindowEvent { label, event, .. } => {
                tray::on_window_event(app, &label, event);
            }
            _ => {}
        });
}

/// setup 之后在后台线程跑：起子进程（npm / pnpm）和写注册表都慢，
/// 失败只记日志，不影响托盘常驻。
fn startup_maintenance(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let paths = match state.paths.lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    };

    if !baseline::exists(&paths) {
        if let Err(e) = baseline::capture(&paths, &state.db) {
            log::warn!("baseline capture: {e}");
        }
    }
    if let Err(e) = hosts_store::ensure_default_entry(&paths) {
        log::warn!("hosts default entry: {e}");
    }

    let py = crate::version_mgr::RuntimeStore::python(&paths);
    if py.current_version().is_some() {
        if let Err(e) = py.ensure_user_path(&state.db) {
            log::warn!("python PATH/alias ensure: {e}");
        }
    }
    let node_store = crate::version_mgr::RuntimeStore::node(&paths);
    if node_store.current_version().is_some() {
        if let Err(e) = node_store.ensure_user_path(&state.db) {
            log::warn!("node PATH ensure: {e}");
        }
    }
    let java_store = crate::version_mgr::RuntimeStore::java(&paths);
    if java_store.current_version().is_some() {
        if let Err(e) = java_store.ensure_user_path(&state.db) {
            log::warn!("java PATH/JAVA_HOME ensure: {e}");
        }
    }
    let go_store = crate::version_mgr::RuntimeStore::golang(&paths);
    if go_store.current_version().is_some() {
        if let Err(e) = go_store.ensure_user_path(&state.db) {
            log::warn!("golang PATH/GOROOT ensure: {e}");
        }
    }
    if let Err(e) = crate::version_mgr::node::sync_npm_config(&state.db, &paths) {
        log::warn!("npm prefix/cache sync: {e}");
    }
    if let Err(e) = crate::version_mgr::node::ensure_pnpm_on_startup(&state.db, &paths) {
        log::warn!("pnpm env ensure: {e}");
    }
}

const APP_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

fn init_file_log(log_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(log_dir);
    let log_path = log_dir.join("app.log");
    rotate_log(&log_path);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(file) => {
            let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
                .format_timestamp_secs()
                .target(env_logger::Target::Pipe(Box::new(Tee { file })))
                .try_init();
            log::info!("envdesk started, log={}", log_path.display());
        }
        Err(_) => {
            let _ = env_logger::try_init();
        }
    }
}

/// app.log 超过上限就滚成 app.log.1（只保留一份），避免日志无限增长。
fn rotate_log(path: &std::path::Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= APP_LOG_MAX_BYTES {
        return;
    }
    let backup = path.with_extension("log.1");
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::rename(path, &backup);
}

struct Tee {
    file: std::fs::File,
}

impl std::io::Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // release 走 windows 子系统，stderr 是无效句柄，写它纯属浪费。
        #[cfg(debug_assertions)]
        let _ = std::io::Write::write_all(&mut std::io::stderr(), buf);
        std::io::Write::write(&mut self.file, buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        #[cfg(debug_assertions)]
        let _ = std::io::Write::flush(&mut std::io::stderr());
        std::io::Write::flush(&mut self.file)
    }
}
