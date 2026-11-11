## 一、调研总结

### 1.1 参考项目分析

**SwitchHosts**（最核心参考模版）
- **技术栈**：Tauri v2 + Rust后端 + React前端（原Electron版已迁移至Tauri）
- **架构特点**：Rust负责系统级操作（hosts写入、权限提升、HTTP服务），React负责UI渲染
- **系统托盘**：通过Tauri的tray API实现，提供快速切换hosts的菜单
- **权限提升**：使用`swh_helper`实现Windows下的特权写入
- **本地HTTP API**：基于Axum提供外部工具集成能力（默认端口5005）
- **数据存储**：`~/.SwitchHosts/internal/state.json` 存储UI状态

**其他可参考项目**：
- **Ip-Switch**：Tauri + Rust，支持hosts编辑、代理配置、托盘颜色自定义
- **RsCat**：纯Rust托盘应用，使用`tray-icon` + Slint UI
- **Envarly**：Windows环境变量管理器，Tauri v2 + React， staged change + diff预览
- **nvm-desktop**：Node.js版本管理GUI，Rust核心 + Tauri + React
- **EnvNexus AI**：Rust + Tauri 2，可识别fnm、pyenv-win等多种版本管理器

### 1.2 关键技术选型建议

| 组件 | 推荐方案 | 理由 |
|------|---------|------|
| GUI框架 | **Tauri v2** | SwitchHosts等成熟项目验证，Rust后端+Web前端，小体积高性能 |
| 系统托盘 | Tauri内置tray API / `tray-icon` | Tauri官方支持完善 |
| Hosts操作 | `hostfile` crate / 自行实现 | 存在成熟的解析crate |
| 环境变量 | Windows Registry API / `winreg` crate | Windows下通过注册表操作 |
| fnm集成 | 调用fnm CLI / 直接使用其库 | fnm本身就是Rust编写的 |
| pyenv集成 | 调用pyenv-win CLI | Windows下标准方案 |
| 异步运行时 | Tokio | SwitchHosts等使用 |
| HTTP服务 | Axum | 提供本地API |
| 序列化 | Serde + JSON | 配置文件存储 |
| 权限提升 | 自研elevation helper | 参考SwitchHosts的`swh_helper`方案 |

### 1.3 功能模块可行性评估

| 功能模块 | 可行性 | 关键依赖/挑战 |
|---------|--------|--------------|
| 系统托盘菜单 | ✅ 高 | Tauri原生支持 |
| Hosts多环境管理 | ✅ 高 | 参考SwitchHosts成熟方案，需处理Windows权限提升 |
| 环境变量可视化编辑 | ✅ 高 | Windows Registry API，注意系统变量保护 |
| fnm版本管理 | ✅ 高 | fnm本身Rust编写，可直接调用其CLI或库 |
| pyenv版本管理 | ✅ 中 | Windows下使用pyenv-win，需处理路径和shell集成 |
| 配置持久化 | ✅ 高 | JSON存储，参考SwitchHosts的state.json方案 |

---

## 二、架构设计

```
┌─────────────────────────────────────────────────────┐
│                  用户界面 (React/TypeScript)          │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────┐ │
│  │ Hosts管理 │ │ 环境变量  │ │ fnm管理  │ │pyenv管理│ │
│  └──────────┘ └──────────┘ └──────────┘ └────────┘ │
└─────────────────────┬───────────────────────────────┘
                      │ Tauri IPC
┌─────────────────────▼───────────────────────────────┐
│              Rust 后端 (Tauri Command)               │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ │
│  │ hosts_apply  │ │  env_manager │ │ version_mgr  │ │
│  │ (权限提升)    │ │ (注册表操作)  │ │ (fnm/pyenv)  │ │
│  └──────────────┘ └──────────────┘ └──────────────┘ │
│  ┌──────────────┐ ┌──────────────┐                  │
│  │  http_api    │ │   storage    │                  │
│  │  (Axum)      │ │  (state.json)│                  │
│  └──────────────┘ └──────────────┘                  │
└─────────────────────┬───────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────┐
│              系统托盘 (Tauri Tray)                   │
│        快速切换 / 显示主窗口 / 退出                  │
└─────────────────────────────────────────────────────┘
```

### 核心目录结构（参考SwitchHosts）

```
src-tauri/
├── src/
│   ├── main.rs              # 入口，托盘初始化
│   ├── lib.rs               # 模块导出
│   ├── hosts_apply/         # hosts操作模块
│   │   ├── mod.rs
│   │   ├── elevation.rs     # 权限提升（参考SwitchHosts）
│   │   ├── aggregate.rs     # 多配置聚合
│   │   └── cmd_runner.rs    # 后置命令执行
│   ├── env_manager/         # 环境变量管理
│   │   ├── mod.rs
│   │   ├── registry.rs      # Windows注册表操作
│   │   └── diff.rs          # 变更预览（参考Envarly）
│   ├── version_mgr/         # 版本管理器
│   │   ├── mod.rs
│   │   ├── fnm.rs           # fnm封装
│   │   └── pyenv.rs         # pyenv-win封装
│   ├── http_api.rs          # Axum本地API服务
│   ├── storage/             # 持久化存储
│   │   ├── mod.rs
│   │   └── state.rs         # state.json读写
│   └── tray.rs              # 托盘菜单定义
├── Cargo.toml
└── tauri.conf.json
```

---

## 三、核心功能实现方案

### 3.1 Hosts管理（参考SwitchHosts）

**数据模型**：
- 支持文件夹/分组组织hosts配置
- 暂时不去支持远程hosts（从URL获取，自动刷新）
- 软删除和恢复（Trashcan）

**权限提升方案**：
1. 将新内容写入临时文件（无需特权）
2. 运行平台特定的elevation helper（`swh_helper`）执行特权写入
3. 支持写入后执行自定义命令（如DNS刷新）

**技术依赖**：
- `hostfile` crate：解析/etc/hosts格式
- 或自行实现解析（格式简单，可控性强）

### 3.2 环境变量管理

**Windows Registry路径**：
- 用户变量：`HKEY_CURRENT_USER\Environment`
- 系统变量：`HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\Session Manager\Environment`

**安全设计（参考Envarly）**：
- **Staged Change**：所有修改先暂存，用户review后统一应用
- **Diff预览**：显示增删改对比
- **系统变量保护**：对SYSTEMROOT、WINDIR、COMSPEC等额外警告
- **PATH编辑器**：支持拖拽排序、路径有效性验证
- **撤销功能**：Ctrl+Z回退未暂存的编辑

**技术依赖**：
- `winreg` crate：Windows注册表操作
- 修改后广播`WM_SETTINGCHANGE`通知系统更新

### 3.3 fnm版本管理

fnm本身就是Rust编写的Node.js版本管理器。

**集成方式**：
1. **CLI调用**：通过`std::process::Command`执行`fnm list`、`fnm install`、`fnm use`等命令
2. **库集成**（更优）：直接引用fnm的crate（如可行），避免进程开销

**功能需求**：
- 列出已安装版本 / 远程可用版本
- 安装/卸载特定版本
- 切换当前使用的版本
- 支持`.node-version`和`.nvmrc`文件解析
- 展示已安装node列表和对应npm、pnpm版本，支持npm版本升降

### 3.4 pyenv版本管理

Windows下使用**pyenv-win**。

**集成方式**：
- 通过`std::process::Command`调用pyenv-win命令
- 需检测pyenv-win是否已安装并配置PATH
- 参考EnvNexus AI的检测方案

**功能需求**：
- 列出已安装Python版本
- 安装/卸载特定版本
- 切换全局/本地Python版本
- 虚拟环境管理

### 3.5 系统托盘菜单设计

```
[应用图标]
├── 🖥️ 显示主窗口
├── ─────────────
├── 📁 Hosts快速切换
│   ├── ▶ 开发环境
│   ├── ▶ 测试环境
│   └── ▶ 生产环境
├── ─────────────
├── 🔧 环境变量
│   ├── 查看/编辑用户变量
│   └── 查看/编辑系统变量
├── ─────────────
├── 📦 Node.js (fnm)
│   ├── 当前: v20.10.0
│   └── 切换版本...
├── ─────────────
├── 🐍 Python (pyenv)
│   ├── 当前: 3.11.0
│   └── 切换版本...
├── ─────────────
└── ❌ 退出
```

参考SwitchHosts的托盘实现和Tauri官方tray API。

### 3.6 本地HTTP API（可选，参考SwitchHosts）

提供REST API供外部工具集成：
- `GET /api/hosts/current` - 获取当前hosts配置
- `POST /api/hosts/switch` - 切换hosts环境
- `GET /api/env` - 获取环境变量列表
- `GET /api/fnm/versions` - 获取Node.js版本列表

使用Axum框架实现。

---

## 四、关键依赖清单

```toml
[dependencies]
# GUI框架
tauri = { version = "2", features = ["tray-icon", ...] }

# 系统托盘
tray-icon = "0.19"  # 或使用Tauri内置

# 异步运行时
tokio = { version = "1", features = ["full"] }

# HTTP服务
axum = "0.8"

# HTTP客户端（远程hosts）
reqwest = { version = "0.12", features = ["json"] }

# 序列化
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Windows注册表
winreg = "0.52"

# 路径操作
dirs = "5"

# 日志
log = "0.4"
env_logger = "0.11"

# 错误处理
anyhow = "1"
thiserror = "2"
```

---

## 五、风险与注意事项

1. **权限问题**：修改hosts和系统环境变量需要管理员权限，需实现可靠的权限提升机制
2. **fnm/pyenv检测**：用户可能未安装或PATH未配置，需做友好提示
3. **环境变量生效**：修改后需广播`WM_SETTINGCHANGE`，部分应用需重启才能生效
4. **跨平台考虑**：虽以Windows为主，但Tauri天然支持跨平台，设计时留有余地
5. **安全性**：环境变量可能包含敏感信息，注意内存安全；已有环境变量注意做好备份能够一键恢复到最开始系统原生状态
6. **数据存储**：考虑适用sqlite3作为基础数据存储；像fnm、pyenv原始存储目录增加面板设置页面，可手动指定存储目录（例如fnm可通过环境变量进行修改配置）