# env-tools

Windows 本机环境面板（产品名 **EnvtsPanel**）：切换 / 维护 Node、Python、Java、Go、用户环境变量、Hosts。Tauri v2 + React + Rust，托盘常驻，目前仅支持 Windows。

| | |
| --- | --- |
| 仓库 | [zhoujumbo/env-tools](https://github.com/zhoujumbo/env-tools) |
| 版本 | `0.0.1-rc1` |
| 平台 | Windows 10/11 x64 |
| 数据目录 | `%APPDATA%\envts`（可用用户环境变量 `ENVT_PATH` 改根路径） |

## 下载

预编译便携包见 [Releases](https://github.com/zhoujumbo/env-tools/releases)：

1. 下载 `EnvtsPanel-0.0.1-rc1-win-x64.zip`
2. 解压后保持 `EnvtsPanel.exe` 与 `WebView2Loader.dll` 同目录
3. 双击运行（需要本机已装 [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)，Win10/11 通常自带）

## 功能

| 模块 | 行为 |
| --- | --- |
| Hosts | 多方案聚合写入系统 hosts；不可写时走 UAC（自身 `runas`） |
| 环境变量 | 读写 `HKCU\Environment`，改完广播 `WM_SETTINGCHANGE` |
| Node / Python / Java / Go | 面板自管下载与版本切换（`runtimes/<runtime>/versions` + `current` junction），用户 PATH 只 prepend 当前入口 |
| 自检 / 日志 / 备份 | 探测 cmd/PS 通道、SQLite 操作日志、zip 备份、原装基线恢复 |

不依赖系统 fnm / pyenv / jenv 等外部版本管理器。

## 数据目录

默认 `%APPDATA%\envts`。启动前设置用户环境变量 `ENVT_PATH`（例如 `D:\envts-data`）可改根目录；改完需重新打开程序。

```
<根>/
  config/           基线、prefs
  data/             sqlite、hosts 方案、备份
  logs/             app.log + 外部命令输出
  runtimes/node|python|java|golang/
  paths.json
```

## 开发

```powershell
git clone https://github.com/zhoujumbo/env-tools.git
cd env-tools
npm install
npm run tauri:dev
```

- 仅前端：`npm run dev`（无 Rust / 托盘 / 系统调用）
- 仅后端检查：`cd src-tauri && cargo check`

环境：Node.js、Rust stable、WebView2。本仓库 `src-tauri` 固定 `crate-type = ["rlib"]`（避免 GNU ld PE ordinal 溢出）。Release 开了 LTO + `opt-level = "s"`；`src-tauri/.cargo/config.toml` 将 rustc `jobs` 钉为 1（MinGW dlltool 并行会死锁）。

日志：`%APPDATA%\envts\logs\app.log`，同时 tee 到 stderr。

## 构建便携包

```powershell
npm run portable
```

1. `npm run build` → `dist/`
2. `tauri build --no-bundle` → `src-tauri\target\release\EnvtsPanel.exe`
3. `scripts/pack-portable.ps1` → `dist-portable\EnvtsPanel\` 与版本化 zip

产物：

```
dist-portable/
  EnvtsPanel/
  EnvtsPanel-0.0.1-rc1-win-x64.zip
```

注意：打包前关掉正在跑的面板，否则清理 `dist-portable\EnvtsPanel\` 可能 Access Denied。只要 exe：`npx tauri build --no-bundle`；要 NSIS 安装器：`npm run tauri:build`。

已有 release 产物时只打 zip：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/pack-portable.ps1
```

## 运行注意

- 改 PATH / hosts 后，**已打开的终端不会自动刷新**，需新开。
- 新装 Windows 上 `python` 可能弹微软商店（`WindowsApps` 0 字节别名）；设 Python current / 启动时会拆 stub，自检页也可手动修。
- 写系统 hosts 需要 UAC；取消即未写入。

## 目录结构

```
src/                  React
src-tauri/src/        Rust（commands / hosts / env / version_mgr / tray）
scripts/              pack-portable.ps1、gen_icons.py
docs/requirements.md  早期调研，以代码为准
```
