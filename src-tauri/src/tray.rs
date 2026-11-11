use crate::hosts_store;
use crate::paths::{AppPaths, Prefs};
use crate::version_mgr::{node, RuntimeStore};
use crate::AppState;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::menu::{Menu, MenuBuilder, MenuItem, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::webview::WebviewWindowBuilder;
use tauri::{AppHandle, Manager};

pub static QUITTING: AtomicBool = AtomicBool::new(false);

/// 每次「显示主窗口」自增，用来取消挂起的延迟销毁。
static RELEASE_GEN: AtomicU64 = AtomicU64::new(0);

/// 最小化到托盘后 WebView2 进程树仍然整份驻留（150–280MB）。
/// 闲置这么久就销毁窗口，下次点托盘重建 —— 页面状态靠前端的 localStorage 兜。
const IDLE_RELEASE_SECS: u64 = 120;

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    if let Some(tray) = app.tray_by_id("main") {
        tray.set_menu(Some(menu))?;
        let _ = tray.set_tooltip(Some("环境面板"));
        attach_handlers(&tray, app);
    } else {
        let icon = app
            .default_window_icon()
            .cloned()
            .expect("missing window icon");
        let tray = TrayIconBuilder::with_id("main")
            .icon(icon)
            .menu(&menu)
            .tooltip("环境面板")
            .show_menu_on_left_click(false)
            .build(app)?;
        attach_handlers(&tray, app);
    }
    Ok(())
}

fn attach_handlers(tray: &tauri::tray::TrayIcon, app: &AppHandle) {
    let handle = app.clone();
    tray.on_menu_event(move |app, event| {
        on_menu(app, event.id.as_ref());
    });
    tray.on_tray_icon_event(move |_t, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            show_main(&handle);
        }
    });
}

pub fn rebuild_menu(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    if let Some(tray) = app.tray_by_id("main") {
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let (hosts_menu, node_menu, python_menu, java_menu, go_menu) =
        if let Some(state) = app.try_state::<AppState>() {
            let paths: AppPaths = lock_paths(&state);
            (
                hosts_submenu(app, &paths)?,
                runtime_submenu(app, &paths, "node")?,
                runtime_submenu(app, &paths, "python")?,
                runtime_submenu(app, &paths, "java")?,
                runtime_submenu(app, &paths, "golang")?,
            )
        } else {
            (
                SubmenuBuilder::new(app, "Hosts 快速切换").build()?,
                SubmenuBuilder::new(app, "Node.js").build()?,
                SubmenuBuilder::new(app, "Python").build()?,
                SubmenuBuilder::new(app, "Java").build()?,
                SubmenuBuilder::new(app, "Golang").build()?,
            )
        };

    MenuBuilder::new(app)
        .item(&show)
        .separator()
        .item(&hosts_menu)
        .item(&node_menu)
        .item(&python_menu)
        .item(&java_menu)
        .item(&go_menu)
        .separator()
        .item(&quit)
        .build()
}

fn hosts_submenu(
    app: &AppHandle,
    paths: &AppPaths,
) -> tauri::Result<tauri::menu::Submenu<tauri::Wry>> {
    let mut hosts_sub = SubmenuBuilder::new(app, "Hosts 快速切换");
    let man = hosts_store::load_manifest(paths);
    if man.entries.is_empty() {
        let empty = MenuItem::with_id(app, "hosts-empty", "(暂无方案)", false, None::<&str>)?;
        hosts_sub = hosts_sub.item(&empty);
    } else {
        for e in man.entries {
            let label = format!("{} {}", if e.on { "●" } else { "○" }, e.title);
            let item =
                MenuItem::with_id(app, format!("hosts:{}", e.id), label, true, None::<&str>)?;
            hosts_sub = hosts_sub.item(&item);
        }
    }
    hosts_sub.build()
}

fn runtime_submenu(
    app: &AppHandle,
    paths: &AppPaths,
    kind: &str,
) -> tauri::Result<tauri::menu::Submenu<tauri::Wry>> {
    let store = match kind {
        "python" => RuntimeStore::python(paths),
        "java" => RuntimeStore::java(paths),
        "golang" => RuntimeStore::golang(paths),
        _ => RuntimeStore::node(paths),
    };
    let current = store.current_version();
    let versions = store.list_installed();
    let title = match (kind, current.as_deref()) {
        ("python", Some(v)) => format!("Python  ·  {v}"),
        ("python", None) => "Python".into(),
        ("java", Some(v)) => format!("Java  ·  {v}"),
        ("java", None) => "Java".into(),
        ("golang", Some(v)) => format!("Golang  ·  {v}"),
        ("golang", None) => "Golang".into(),
        (_, Some(v)) => format!("Node.js  ·  {v}"),
        (_, None) => "Node.js".into(),
    };
    let mut sub = SubmenuBuilder::new(app, title);
    if versions.is_empty() {
        let empty = MenuItem::with_id(app, format!("{kind}-empty"), "(未安装)", false, None::<&str>)?;
        sub = sub.item(&empty);
    } else {
        for v in versions {
            let mark = if current.as_deref() == Some(v.as_str()) {
                "●"
            } else {
                "○"
            };
            let item = MenuItem::with_id(
                app,
                format!("{kind}:{v}"),
                format!("{mark} {v}"),
                true,
                None::<&str>,
            )?;
            sub = sub.item(&item);
        }
    }
    sub.build()
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "show" => show_main(app),
        "quit" => {
            QUITTING.store(true, Ordering::SeqCst);
            app.exit(0);
        }
        other if other.starts_with("hosts:") => toggle_hosts(app, other.trim_start_matches("hosts:")),
        other if other.starts_with("node:") => set_runtime(app, "node", other.trim_start_matches("node:")),
        other if other.starts_with("python:") => {
            set_runtime(app, "python", other.trim_start_matches("python:"))
        }
        other if other.starts_with("java:") => set_runtime(app, "java", other.trim_start_matches("java:")),
        other if other.starts_with("golang:") => {
            set_runtime(app, "golang", other.trim_start_matches("golang:"))
        }
        _ => {}
    }
}

fn lock_paths(state: &tauri::State<'_, AppState>) -> AppPaths {
    match state.paths.lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

fn toggle_hosts(app: &AppHandle, hid: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let paths = lock_paths(&state);
    let man = hosts_store::load_manifest(&paths);
    if let Some(e) = man.entries.iter().find(|x| x.id == hid) {
        let _ = hosts_store::set_on(&paths, hid, !e.on);
    }
    let content = hosts_store::aggregate_enabled(&paths);
    if let Ok(outcome) = crate::hosts_apply::apply_to_system_hosts(&content) {
        let undo = serde_json::json!({
            "type": "hosts",
            "previous_content": outcome.previous_content
        });
        let _ = crate::journal::log_action(
            &state.db,
            "tray",
            "hosts_apply",
            Some("tray toggle".into()),
            None,
            Some(&serde_json::json!({ "unchanged": outcome.unchanged })),
            Some(&undo),
            true,
        );
        if crate::paths::Prefs::load(&paths).flush_dns_after_hosts {
            let r = crate::winutil::run_cmd_exe("ipconfig /flushdns");
            let _ = crate::journal::log_command(
                &state.db,
                &paths,
                "tray",
                "flushdns",
                &r,
                None,
                None,
                None,
            );
        }
    }
    let _ = rebuild_menu(app);
}

fn set_runtime(app: &AppHandle, kind: &str, version: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let paths = lock_paths(&state);
    let store = match kind {
        "python" => RuntimeStore::python(&paths),
        "java" => RuntimeStore::java(&paths),
        "golang" => RuntimeStore::golang(&paths),
        _ => RuntimeStore::node(&paths),
    };
    if store.current_version().as_deref() == Some(version) {
        return;
    }
    if store.set_current(&state.db, version).is_err() {
        return;
    }
    if kind == "node" {
        let _ = node::apply_pkg_layout(&state.db, &paths, version);
    }
    let _ = rebuild_menu(app);
}

pub fn drop_stray_webviews(app: &AppHandle, keep: Option<&str>) {
    for (label, w) in app.webview_windows() {
        if keep.is_some_and(|k| k == label) {
            continue;
        }
        log::info!("destroy idle webview {}", label);
        let _ = w.destroy();
    }
}

pub fn on_window_event(app: &AppHandle, label: &str, event: tauri::WindowEvent) {
    match event {
        tauri::WindowEvent::CloseRequested { .. } => {
            if label != "main" {
                if let Some(w) = app.get_webview_window(label) {
                    let _ = w.destroy();
                }
            }
        }
        tauri::WindowEvent::Resized(_) => {
            if label == "main" {
                if let Some(w) = app.get_webview_window(label) {
                    if w.is_minimized().unwrap_or(false)
                        && w.is_visible().unwrap_or(false)
                        && minimize_to_tray_enabled(app)
                    {
                        let _ = w.hide();
                        schedule_release(app, label);
                    }
                }
            }
        }
        tauri::WindowEvent::Destroyed => {
            if label == "main" {
                drop_stray_webviews(app, None);
            } else {
                drop_stray_webviews(app, Some("main"));
            }
        }
        _ => {}
    }
}

fn minimize_to_tray_enabled(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|state| {
            let p = match state.paths.lock() {
                Ok(g) => g.clone(),
                Err(e) => e.into_inner().clone(),
            };
            Prefs::load(&p).minimize_to_tray
        })
        .unwrap_or(true)
}

/// 排一次延迟销毁。期间如果窗口被重新显示、或者有安装/下载在跑，就作废。
fn schedule_release(app: &AppHandle, label: &str) {
    let gen = RELEASE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(IDLE_RELEASE_SECS));
        if RELEASE_GEN.load(Ordering::SeqCst) != gen {
            return;
        }
        if crate::version_mgr::op_running() {
            return;
        }
        let Some(w) = app.get_webview_window(&label) else {
            return;
        };
        if w.is_visible().unwrap_or(true) {
            return;
        }
        log::info!("idle {}s, releasing webview {label}", IDLE_RELEASE_SECS);
        let _ = w.destroy();
    });
}

pub fn show_main(app: &AppHandle) {
    // 取消挂起的延迟销毁
    RELEASE_GEN.fetch_add(1, Ordering::SeqCst);
    drop_stray_webviews(app, Some("main"));
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let Some(cfg) = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
    else {
        log::error!("missing main window config");
        return;
    };
    match WebviewWindowBuilder::from_config(app, &cfg) {
        Ok(builder) => {
            if let Err(e) = builder.build() {
                log::error!("create main window: {e}");
            }
        }
        Err(e) => log::error!("main window builder: {e}"),
    }
}
