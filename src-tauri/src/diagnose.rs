use crate::hosts_apply::write as hosts_write;
use crate::paths::{dir_writable, AppPaths};
use crate::shell_compat;
use crate::version_mgr::RuntimeStore;
use crate::winutil;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckItem {
    pub id: String,
    pub title: String,
    pub ok: bool,
    pub level: String,
    pub detail: String,
    pub command: Option<String>,
    pub stdout: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnoseReport {
    pub items: Vec<CheckItem>,
    pub first_run: bool,
    pub baseline_exists: bool,
}

fn item(id: &str, title: &str, ok: bool, level: &str, detail: String, cmd: Option<String>, out: Option<String>) -> CheckItem {
    CheckItem {
        id: id.into(),
        title: title.into(),
        ok,
        level: level.into(),
        detail,
        command: cmd,
        stdout: out,
    }
}

pub fn run(paths: &AppPaths) -> DiagnoseReport {
    let mut items = Vec::new();

    let root = crate::paths::storage_root();
    let from_env = crate::paths::from_envt_path();
    items.push(item(
        "storage_root",
        "数据根目录",
        dir_writable(&root),
        "info",
        if from_env {
            format!("来自 ENVT_PATH: {}", root.display())
        } else {
            format!("未配置 ENVT_PATH，使用 %APPDATA%\\envts: {}", root.display())
        },
        Some("ENVT_PATH".into()),
        crate::paths::envt_path_value().map(|p| p.display().to_string()),
    ));

    items.push(item(
        "config_dir",
        "核心配置目录可写",
        dir_writable(&paths.config_dir),
        "info",
        paths.config_dir.display().to_string(),
        None,
        None,
    ));
    items.push(item(
        "data_dir",
        "数据存储目录可写",
        dir_writable(&paths.data_dir),
        "info",
        paths.data_dir.display().to_string(),
        None,
        None,
    ));
    items.push(item(
        "log_dir",
        "日志目录可写",
        dir_writable(&paths.log_dir),
        "info",
        paths.log_dir.display().to_string(),
        None,
        None,
    ));

    let (hr, hd) = hosts_write::can_read_hosts();
    items.push(item("hosts_read", "系统 hosts 可读", hr, "info", hd, None, None));
    let (hw, hwd) = hosts_write::can_write_hosts();
    items.push(item(
        "hosts_write",
        "系统 hosts 直接可写",
        hw,
        if hw { "ok" } else { "warn" },
        if hw {
            hwd
        } else {
            format!("{hwd}（不可写是正常的，应用时会走 UAC 提权）")
        },
        None,
        None,
    ));

    let node_rt = RuntimeStore::node(paths).status();
    items.push(item(
        "node_runtime",
        "本地面板 Node 运行时",
        node_rt.current.is_some() && node_rt.path_ok,
        if node_rt.current.is_some() && !node_rt.path_ok {
            "error"
        } else if node_rt.current.is_some() {
            "ok"
        } else {
            "info"
        },
        if node_rt.current.is_some() && !node_rt.path_ok {
            format!(
                "current={:?} 但用户 PATH 未包含 {}。新开 cmd 会找不到 node。",
                node_rt.current,
                node_rt.path_entries.join(";")
            )
        } else {
            format!(
                "root={} current={:?} versions={} path_ok={}",
                node_rt.root,
                node_rt.current,
                node_rt.versions.len(),
                node_rt.path_ok
            )
        },
        None,
        node_rt.current.clone(),
    ));

    let node_path = winutil::which("node");
    items.push(item(
        "node_path",
        "当前进程 PATH 中的 node",
        node_path.is_some(),
        "info",
        node_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "未找到".into()),
        Some("where node".into()),
        None,
    ));

    let node_cmd = winutil::run_cmd_exe("node -v");
    items.push(item(
        "node_cmd",
        "cmd 通道 node -v",
        node_cmd.ok,
        if node_rt.current.is_some() && !node_cmd.ok {
            "error"
        } else if node_cmd.ok {
            "ok"
        } else {
            "info"
        },
        if node_cmd.ok {
            node_cmd.stdout.clone()
        } else if node_rt.current.is_some() {
            format!("{}（已设 current 但 cmd 找不到 node）", node_cmd.stderr)
        } else {
            node_cmd.stderr.clone()
        },
        Some("cmd /D /C node -v".into()),
        Some(node_cmd.stdout.clone()),
    ));

    let node_ps = winutil::run_powershell("node -v");
    items.push(item(
        "node_ps",
        "PowerShell -NoProfile 通道 node -v",
        node_ps.ok,
        if node_ps.ok { "ok" } else { "warn" },
        if node_ps.ok {
            node_ps.stdout.clone()
        } else {
            node_ps.stderr.clone()
        },
        Some("powershell -NoProfile -Command node -v".into()),
        Some(node_ps.stdout.clone()),
    ));

    let npm_cmd = winutil::run_cmd_exe("npm -v");
    items.push(item(
        "npm_cmd",
        "cmd 通道 npm -v",
        npm_cmd.ok,
        if node_rt.current.is_some() && !npm_cmd.ok {
            "error"
        } else if npm_cmd.ok {
            "ok"
        } else {
            "info"
        },
        if npm_cmd.ok {
            npm_cmd.stdout.clone()
        } else if node_rt.current.is_some() {
            format!("{}（已有 Node current，cmd 找不到 npm）", if npm_cmd.stderr.is_empty() { "未找到".into() } else { npm_cmd.stderr.clone() })
        } else {
            "未找到".into()
        },
        Some("cmd /D /C npm -v".into()),
        Some(npm_cmd.stdout),
    ));

    let pnpm_env = crate::version_mgr::node::probe_pnpm_env(paths);
    items.push(item(
        "pnpm_env",
        "pnpm 命令可用",
        !pnpm_env.needs_fix,
        if pnpm_env.needs_fix {
            "error"
        } else if pnpm_env.cmd_ok {
            "ok"
        } else {
            "info"
        },
        pnpm_env.detail.clone(),
        Some("cmd /D /C pnpm -v".into()),
        pnpm_env.version.or(pnpm_env.which),
    ));

    let layout = crate::version_mgr::node::pkg_layout_status(paths);
    let prefix_on_path = {
        let current = winutil::get_user_env("Path")
            .ok()
            .flatten()
            .unwrap_or_default();
        current
            .split(';')
            .any(|p| p.eq_ignore_ascii_case(&layout.global_dir))
    };
    let stale_globals: Vec<String> = {
        let current = winutil::get_user_env("Path")
            .ok()
            .flatten()
            .unwrap_or_default();
        current
            .split(';')
            .filter(|p| {
                let t = p.trim().trim_end_matches(['\\', '/']);
                std::path::Path::new(t)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("node_global"))
                    && !p.eq_ignore_ascii_case(&layout.global_dir)
            })
            .map(|s| s.to_string())
            .collect()
    };
    let node_current = crate::version_mgr::RuntimeStore::node(paths)
        .current_version()
        .is_some();
    let prefix_needed = node_current;
    let prefix_ok = (prefix_on_path || !prefix_needed) && stale_globals.is_empty();
    items.push(item(
        "npm_prefix_path",
        "npm prefix 已在用户 PATH",
        prefix_ok,
        if !prefix_ok {
            "error"
        } else if prefix_on_path {
            "ok"
        } else {
            "info"
        },
        if !stale_globals.is_empty() {
            format!(
                "用户 PATH 仍有旧全局目录 {}；当前目标 {}",
                stale_globals.join("; "),
                layout.global_dir
            )
        } else if prefix_needed && !prefix_on_path {
            format!(
                "全局包目录 {} 不在用户 PATH，`npm i -g` / pnpm 会报不是内部或外部命令",
                layout.global_dir
            )
        } else {
            layout.global_dir.clone()
        },
        None,
        None,
    ));

    let sh = shell_compat::status(paths);
    let leftover = sh.ps5_injected || sh.ps7_injected || sh.cmd_autorun_points_to_us;
    items.push(item(
        "legacy_shell",
        "旧版 fnm 壳钩子残留",
        !leftover,
        if leftover { "warn" } else { "ok" },
        if leftover {
            format!(
                "发现本面板旧版写入的 PS profile / CMD AutoRun。新模型不再需要，可点「清理旧钩子」。PS5={} PS7={} AutoRun={:?}",
                sh.ps5_injected, sh.ps7_injected, sh.cmd_autorun
            )
        } else {
            "未发现本面板写入的 profile / AutoRun".into()
        },
        None,
        None,
    ));

    let py_rt = RuntimeStore::python(paths).status();
    items.push(item(
        "python_runtime",
        "本地面板 Python 运行时",
        py_rt.current.is_some() && py_rt.path_ok,
        if py_rt.current.is_some() && !py_rt.path_ok {
            "error"
        } else if py_rt.current.is_some() {
            "ok"
        } else {
            "info"
        },
        if py_rt.current.is_some() && !py_rt.path_ok {
            format!(
                "current={:?} 但用户 PATH 未包含 {}。新开 cmd 会找不到 python。",
                py_rt.current,
                py_rt.path_entries.join(";")
            )
        } else {
            format!(
                "root={} current={:?} versions={} path_ok={}",
                py_rt.root,
                py_rt.current,
                py_rt.versions.len(),
                py_rt.path_ok
            )
        },
        None,
        py_rt.current.clone(),
    ));

    let py_hits = winutil::which_all("python");
    let py_first = py_hits.first().map(|p| p.display().to_string());
    let py_store = winutil::python_store_alias_present();
    let py_blocked = (py_rt.current.is_some() && py_store)
        || py_first
            .as_deref()
            .is_some_and(|p| winutil::path_looks_like_windowsapps(std::path::Path::new(p)));
    items.push(item(
        "python_store_alias",
        "微软商店 python 别名",
        !py_blocked,
        if py_blocked { "error" } else { "ok" },
        if py_blocked {
            format!(
                "cmd 的 python 被 WindowsApps 占坑，会调起微软商店。where 第一条={} stubs={}",
                py_first.as_deref().unwrap_or("-"),
                winutil::python_store_alias_paths()
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        } else {
            py_first
                .clone()
                .unwrap_or_else(|| "未找到 python（未装或 PATH 未生效）".into())
        },
        Some("where python".into()),
        Some(py_hits.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")),
    ));

    let py_cmd = winutil::run_cmd_exe("python --version");
    items.push(item(
        "python_cmd",
        "cmd 通道 python --version",
        py_cmd.ok,
        if py_rt.current.is_some() && !py_cmd.ok && !py_blocked {
            "error"
        } else if py_cmd.ok {
            "ok"
        } else {
            "info"
        },
        if py_cmd.ok { py_cmd.stdout.clone() } else { py_cmd.stderr.clone() },
        Some("cmd /D /C python --version".into()),
        Some(py_cmd.stdout),
    ));
    let py_ps = winutil::run_powershell("python --version");
    items.push(item(
        "python_ps",
        "PowerShell 通道 python --version",
        py_ps.ok,
        "info",
        if py_ps.ok { py_ps.stdout.clone() } else { py_ps.stderr.clone() },
        Some("powershell -NoProfile -Command python --version".into()),
        Some(py_ps.stdout),
    ));
    let py_launcher = winutil::run_cmd_exe("py -0p");
    items.push(item(
        "py_launcher",
        "系统 Python Launcher (py -0p)",
        py_launcher.ok,
        "info",
        if py_launcher.ok {
            py_launcher.stdout.clone()
        } else {
            "未检测到 py launcher".into()
        },
        Some("cmd /D /C py -0p".into()),
        Some(py_launcher.stdout),
    ));

    let baseline = crate::baseline::exists(paths);
    items.push(item(
        "baseline",
        "原装基线已采集",
        baseline,
        if baseline { "ok" } else { "warn" },
        paths.baseline_dir().display().to_string(),
        None,
        None,
    ));

    DiagnoseReport {
        items,
        first_run: !paths.first_run_flag().exists(),
        baseline_exists: baseline,
    }
}
