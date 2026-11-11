# EnvtsPanel

Windows 本机环境面板：Hosts 方案、用户环境变量、自管 Node / Python 运行时。Tauri v2 + React + Rust，托盘常驻。

产品名 `EnvtsPanel`，crate 名 `envdesk`。只做 Windows。

## 做什么

| 模块 | 实际行为 |
| --- | --- |
| Hosts | 多方案聚合写入系统 hosts，不可写时走 UAC（自身 `runas`，不经过 `cmd /c copy`） |
| 环境变量 | 读写 `HKCU\Environment`，改完广播 `WM_SETTINGCHANGE` |
| Node | 拉 nodejs.org zip，解到 `runtimes/node/versions`，`current` junction，用户 PATH 只 prepend 这一条 |
| Python | 拉 NuGet `python` nupkg，同样 `current` junction；会关掉微软商店的 `python.exe` 别名，并把 `current` 置顶 PATH |
| 自检 / 日志 / 备份 | 探测 cmd/PS 通道、操作日志（SQLite）、zip 备份、原装基线一键恢复 |

不依赖系统 fnm / pyenv。面板自己下运行时。

## 数据目录

默认 `%APPDATA%\envts`（`C:\Users\<用户>\AppData\Roaming\envts`）。

启动前把用户环境变量 `ENVT_PATH` 设成别的盘（例如 `D:\envts-data`），整个根目录改到那里。改完必须重新打开本程序。

```
<根>/
  config/          基线、prefs
  data/            sqlite、hosts 方案、备份
  logs/            app.log + 外部命令 stdout/stderr
  runtimes/node/
  runtimes/python/
  paths.json
```

## 环境要求

- Windows 10/11 x64
- [Node.js](https://nodejs.org/)（开发/构建前端；面板装的 Node 跟这个无关）
- [Rust](https://rustup.rs/) stable，`cargo` 在 PATH 里
- WebView2 Runtime（Win10/11 一般都有）
- 打包机不要用会把 crate 编成 `cdylib` 的配置；本仓库 `src-tauri` 已固定 `crate-type = ["rlib"]`（GNU ld 否则会炸 PE ordinal）

Release 开了 LTO + `opt-level = "s"`，完整编一次大约几分钟。`src-tauri/.cargo/config.toml` 把 rustc `jobs` 钉成 1（MinGW dlltool 并行会死锁）。

## 开发

```powershell
git clone <repo>
cd desktop-env-tool-win
npm install
npm run tauri:dev
```

等价于 `npx tauri dev`：Vite 起在 `http://localhost:1420`，再拉起带热更新的桌面窗。

只跑前端（没有 Rust / 托盘 / 系统调用）：

```powershell
npm run dev
```

后端单独检查：

```powershell
cd src-tauri
cargo check
```

日志默认写到 `%APPDATA%\envts\logs\app.log`，同时 tee 到 stderr。`tauri dev` 控制台能看到启动路径。

## 构建便携包

日常发货用这个：

```powershell
npm run portable
```

流程：

1. `npm run build` → `tsc` + Vite，产物进 `dist/`
2. `tauri build --no-bundle` → `src-tauri\target\release\EnvtsPanel.exe`
3. `scripts/pack-portable.ps1` 把 exe + `WebView2Loader.dll`（以及同目录其它 dll）拷到 `dist-portable\EnvtsPanel\`，再打带时间戳的 zip

产物：

```
dist-portable/
  EnvtsPanel/                              解压即用目录
  EnvtsPanel-portable-YYYYMMDD-HHMMSS.zip
```

把 zip 丢到另一台机器，解压后 **exe 和 dll 放一起**，双击 `EnvtsPanel.exe`。不要只拷 exe。

### 打包注意

- **先关掉正在跑的面板。** 脚本会删 `dist-portable\EnvtsPanel\`。若你正在跑这个目录里的 exe，`Remove-Item` 会 Access Denied，zip 打不出来。
- 只要 exe、不要安装器：`npx tauri build --no-bundle`，产物在 `src-tauri\target\release\EnvtsPanel.exe`。
- 走 Tauri 安装包（NSIS 等）：`npm run tauri:build`。便携发货不需要。

手动只打 zip（exe 已经编好）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/pack-portable.ps1
```

## 运行注意

- 改 PATH / hosts 之后，**已打开的 cmd / 终端不会自动刷新**。新开一个。
- 新装 Windows 上 `python` 可能弹微软商店：那是 `%LOCALAPPDATA%\Microsoft\WindowsApps` 里的 0 字节别名。面板在设 Python current / 启动时会拆掉 stub；自检页也有修复按钮。修完仍要新开 cmd。
- Hosts 写入系统文件要 UAC。点取消就是没写上。

## 目录

```
src/                 React
src-tauri/src/       Rust（commands / hosts / env / version_mgr / tray）
scripts/
  pack-portable.ps1
  gen_icons.py
docs/requirements.md 早期调研，和当前实现不完全一致，以代码为准
```
