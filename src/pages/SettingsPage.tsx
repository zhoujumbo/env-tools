import { useEffect, useState } from "react";
import { api } from "../api";
import type { AppPaths, NodePkgLayout, Prefs, ProxyMode, ToolCache } from "../types";
import type { ThemePref } from "../theme";
import { applyTheme } from "../theme";
import BackupPage from "./BackupPage";
import { useConfirm } from "../components/ConfirmDialog";
import { useToast } from "../components/Toast";

const THEMES: { id: ThemePref; label: string; hint: string }[] = [
  { id: "light", label: "明亮", hint: "浅色界面" },
  { id: "dark", label: "暗黑", hint: "默认" },
  { id: "system", label: "跟随系统", hint: "看 Windows 浅色/深色" },
];

const PROXY_MODES: { id: ProxyMode; label: string; hint: string }[] = [
  { id: "none", label: "不使用代理", hint: "直连，忽略系统代理" },
  { id: "system", label: "跟随系统", hint: "Windows 系统代理 / HTTP_PROXY" },
  { id: "manual", label: "手动代理", hint: "指定地址、端口、认证" },
];

type SettingsSection =
  | "appearance"
  | "proxy"
  | "node_pkg"
  | "cache"
  | "baseline"
  | "backup"
  | "storage"
  | "misc";

const SETTINGS_NAV: { id: SettingsSection; label: string }[] = [
  { id: "appearance", label: "外观" },
  { id: "proxy", label: "代理" },
  { id: "node_pkg", label: "Node 目录" },
  { id: "cache", label: "缓存" },
  { id: "baseline", label: "基线" },
  { id: "backup", label: "备份" },
  { id: "storage", label: "存储" },
  { id: "misc", label: "其它" },
];

const SECTION_TITLE: Record<SettingsSection, string> = {
  appearance: "外观",
  proxy: "代理",
  node_pkg: "Node 目录",
  cache: "缓存",
  baseline: "基线",
  backup: "备份",
  storage: "存储",
  misc: "其它",
};

function fmtBytes(n: number) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function withPrefs(p: Prefs): Prefs {
  const mode: ProxyMode = p.proxy_mode === "none" || p.proxy_mode === "manual" ? p.proxy_mode : "system";
  return {
    ...p,
    node_pkg_override: !!p.node_pkg_override,
    node_pkg_root: p.node_pkg_root ?? "",
    proxy_mode: mode,
    proxy_host: p.proxy_host ?? "",
    proxy_port: Number(p.proxy_port) || 0,
    proxy_user: p.proxy_user ?? "",
    proxy_pass: p.proxy_pass ?? "",
    minimize_to_tray: p.minimize_to_tray !== false,
  };
}

export default function SettingsPage(props: { themePref: ThemePref; onTheme: (t: ThemePref) => void }) {
  const confirm = useConfirm();
  const toast = useToast();
  const [section, setSection] = useState<SettingsSection>("appearance");
  const [paths, setPaths] = useState<AppPaths | null>(null);
  const [prefs, setPrefs] = useState<Prefs | null>(null);
  const [layout, setLayout] = useState<NodePkgLayout | null>(null);
  const [caches, setCaches] = useState<ToolCache[]>([]);
  const [cachesLoaded, setCachesLoaded] = useState(false);
  const [cacheBusy, setCacheBusy] = useState(false);
  const [migrate, setMigrate] = useState(true);
  const [err, setErr] = useState("");
  const [msg, setMsg] = useState("");

  function ok(s: string) {
    setMsg(s);
    toast("ok", s);
  }
  function fail(e: unknown) {
    const s = String(e);
    setErr(s);
    toast("err", s);
  }

  async function loadCaches() {
    setCacheBusy(true);
    try {
      setCaches(await api.listToolCaches());
      setCachesLoaded(true);
    } catch (e) {
      fail(e);
    } finally {
      setCacheBusy(false);
    }
  }

  useEffect(() => {
    void (async () => {
      // 三个读取互不依赖，并行；任何一个慢都不拖另一个。
      const [p, pr, lay] = await Promise.all([
        api.getPaths(),
        api.getPrefs(),
        api.nodePkgLayout(),
      ]);
      setPaths(p);
      setPrefs(withPrefs(pr));
      setLayout(lay);
    })();
  }, []);

  // 缓存扫描会起 npm/pip 子进程再 walkdir 整个缓存目录，等用户真的点进「缓存」再扫。
  useEffect(() => {
    if (section === "cache" && !cachesLoaded && !cacheBusy) void loadCaches();
  }, [section, cachesLoaded]);

  async function browse(key: "config_dir" | "data_dir" | "log_dir") {
    const p = await api.pickDirectory("选择目录");
    if (p && paths) setPaths({ ...paths, [key]: p });
  }

  async function persistTheme(t: ThemePref) {
    props.onTheme(t);
    applyTheme(t);
    const cur = withPrefs(prefs ?? (await api.getPrefs()));
    const next = { ...cur, theme: t };
    setPrefs(next);
    try {
      await api.setPrefs(next);
      ok(`外观已保存：${THEMES.find((x) => x.id === t)?.label ?? t}`);
    } catch (e) {
      fail(e);
    }
  }

  async function savePrefs(next: Prefs) {
    setErr("");
    const p = withPrefs({ ...next, theme: props.themePref });
    setPrefs(p);
    const r = await api.setPrefs(p);
    setLayout(await api.nodePkgLayout());
    return r;
  }

  function assertManualProxy(p: Prefs) {
    const host = p.proxy_host.trim();
    const portFromHost = /:\d+$/.test(host.replace(/\/+$/, ""));
    if (!host) throw new Error("手动代理需要填写地址");
    if (!portFromHost && (p.proxy_port < 1 || p.proxy_port > 65535)) {
      throw new Error("手动代理需要填写 1–65535 的端口");
    }
  }

  async function persistProxyMode(mode: ProxyMode) {
    if (!prefs) return;
    const next = { ...prefs, proxy_mode: mode };
    setPrefs(next);
    try {
      await savePrefs(next);
      ok("代理设置已保存");
    } catch (e) {
      fail(e);
    }
  }

  if (!paths || !prefs) {
    return (
      <div className="settings-page">
        <div className="settings-layout">
          <div className="card settings-side">
            <div className="settings-side-list">
              {SETTINGS_NAV.map((n) => (
                <button type="button" key={n.id} className="settings-nav-item" disabled>
                  {n.label}
                </button>
              ))}
            </div>
          </div>
          <div className="card settings-main">
            <div className="settings-main-head">
              <h3>{SECTION_TITLE[section]}</h3>
            </div>
            <div className="settings-body">
              <div className="muted">读取配置…</div>
            </div>
          </div>
        </div>
      </div>
    );
  }

  const customRoot = prefs.node_pkg_root.trim();
  const previewGlobal = customRoot ? `${customRoot.replace(/[\\/]+$/, "")}\\node_global` : (layout?.npm_prefix_default ?? "");
  const previewCache = customRoot ? `${customRoot.replace(/[\\/]+$/, "")}\\node_cache` : (layout?.npm_cache_default ?? "");
  const rootFilled = prefs.node_pkg_override && customRoot.length > 0;

  return (
    <div className="settings-page">
      {(err || msg) && (
        <div className="settings-status">
          {err && <div className="err">{err}</div>}
          {msg && <div className="ok">{msg}</div>}
        </div>
      )}
      <div className="settings-layout">
        <div className="card settings-side">
          <div className="settings-side-list">
            {SETTINGS_NAV.map((n) => (
              <button
                type="button"
                key={n.id}
                className={`settings-nav-item ${section === n.id ? "active" : ""}`}
                onClick={() => {
                  setErr("");
                  setMsg("");
                  setSection(n.id);
                }}
              >
                {n.label}
              </button>
            ))}
          </div>
        </div>

        <div className="card settings-main">
          <div className="settings-main-head">
            <h3>{SECTION_TITLE[section]}</h3>
          </div>
          <div className="settings-body">
            {section === "appearance" && (
              <div className="theme-picks">
                {THEMES.map((t) => (
                  <button
                    type="button"
                    key={t.id}
                    className={`theme-pick ${props.themePref === t.id ? "active" : ""}`}
                    onClick={() => void persistTheme(t.id)}
                  >
                    <b>{t.label}</b>
                    <span className="muted">{t.hint}</span>
                  </button>
                ))}
              </div>
            )}

            {section === "proxy" && (
              <>
                <p className="muted" style={{ marginTop: 0 }}>
                  用于拉取 Node / Python 版本列表和安装包。跟随系统读取 Windows 系统代理，没有则看{" "}
                  <code>HTTP_PROXY</code> / <code>HTTPS_PROXY</code>；PAC 自动配置脚本暂不跟随。
                </p>
                <div className="theme-picks">
                  {PROXY_MODES.map((t) => (
                    <button
                      type="button"
                      key={t.id}
                      className={`theme-pick ${prefs.proxy_mode === t.id ? "active" : ""}`}
                      onClick={() => void persistProxyMode(t.id)}
                    >
                      <b>{t.label}</b>
                      <span className="muted">{t.hint}</span>
                    </button>
                  ))}
                </div>
                {prefs.proxy_mode === "manual" && (
                  <div className="proxy-form">
                    <div className="path-row">
                      <label>代理地址</label>
                      <input
                        placeholder="127.0.0.1"
                        value={prefs.proxy_host}
                        onChange={(e) => setPrefs({ ...prefs, proxy_host: e.target.value })}
                      />
                    </div>
                    <div className="path-row">
                      <label>端口</label>
                      <input
                        type="number"
                        min={1}
                        max={65535}
                        placeholder="7890"
                        value={prefs.proxy_port || ""}
                        onChange={(e) => {
                          const n = Number(e.target.value);
                          setPrefs({ ...prefs, proxy_port: Number.isFinite(n) ? n : 0 });
                        }}
                      />
                    </div>
                    <div className="path-row">
                      <label>用户名</label>
                      <input
                        placeholder="可空"
                        autoComplete="off"
                        value={prefs.proxy_user}
                        onChange={(e) => setPrefs({ ...prefs, proxy_user: e.target.value })}
                      />
                    </div>
                    <div className="path-row">
                      <label>密码</label>
                      <input
                        type="password"
                        placeholder="可空"
                        autoComplete="new-password"
                        value={prefs.proxy_pass}
                        onChange={(e) => setPrefs({ ...prefs, proxy_pass: e.target.value })}
                      />
                    </div>
                    <button
                      className="btn primary"
                      onClick={async () => {
                        try {
                          assertManualProxy(prefs);
                          await savePrefs(prefs);
                          ok("代理设置已保存");
                        } catch (e) {
                          fail(e);
                        }
                      }}
                    >
                      保存代理设置
                    </button>
                  </div>
                )}
              </>
            )}

            {section === "node_pkg" && (
              <>
                <p className="muted" style={{ marginTop: 0 }}>
                  默认关闭。打开且填了根目录后，切换 Node、保存本项、以及每次启动面板时，会核对 npm 的 prefix/cache，并把用户 PATH /
                  PNPM_HOME 对齐到该根下的 <code>node_global</code> / <code>node_cache</code>
                  （目录不存在会创建）。换根会从 PATH 摘掉旧的 <code>node_global</code>
                  ，不会搬迁旧全局包。关闭或路径留空时，启动会重置为系统默认。
                </p>
                <label>
                  <input
                    type="checkbox"
                    checked={prefs.node_pkg_override}
                    onChange={(e) => setPrefs({ ...prefs, node_pkg_override: e.target.checked })}
                  />{" "}
                  切换 Node 时自定义全局包 / 缓存目录
                </label>
                {prefs.node_pkg_override && (
                  <>
                    <div className="path-row" style={{ marginTop: 10 }}>
                      <label>自定义根目录</label>
                      <input
                        placeholder="留空 = 系统默认，不自动改写"
                        value={prefs.node_pkg_root}
                        onChange={(e) => setPrefs({ ...prefs, node_pkg_root: e.target.value })}
                      />
                      <button
                        className="btn"
                        onClick={async () => {
                          const p = await api.pickDirectory("选择 Node 包根目录");
                          if (p) setPrefs({ ...prefs, node_pkg_root: p });
                        }}
                      >
                        浏览
                      </button>
                    </div>
                    {layout && (
                      <div className="mono baseline-meta">
                        {rootFilled
                          ? `将使用（保存时创建缺失目录）
全局包  ${previewGlobal}
缓存    ${previewCache}`
                          : `系统默认（不改写）
npm prefix  ${layout.npm_prefix_default}
npm cache   ${layout.npm_cache_default}
pnpm home   ${layout.pnpm_home_default}
pnpm store  ${layout.pnpm_store_default}
pnpm cache  ${layout.pnpm_cache_default}`}
                      </div>
                    )}
                  </>
                )}
                <button
                  className="btn primary"
                  style={{ marginTop: 10 }}
                  onClick={async () => {
                    try {
                      const r = await savePrefs(prefs);
                      ok(r.detail ? r.detail : "Node 目录设置已保存");
                    } catch (e) {
                      fail(e);
                    }
                  }}
                >
                  保存 Node 设置
                </button>
              </>
            )}

            {section === "cache" && (
              <>
                <div className="row" style={{ justifyContent: "space-between" }}>
                  <p className="muted" style={{ margin: 0 }}>
                    按本机常见工具目录估算。只删缓存，不动全局安装的包（node_global）。
                  </p>
                  <button className="btn" disabled={cacheBusy} onClick={() => void loadCaches()}>
                    {cacheBusy ? "扫描中…" : "重新扫描"}
                  </button>
                </div>
                {caches.length === 0 && !cacheBusy && <div className="muted">没有扫到缓存项</div>}
                <div className="list" style={{ maxHeight: 420, marginTop: 8 }}>
                  {caches.map((c) => (
                    <div key={c.id} className="list-item" style={{ cursor: "default" }}>
                      <div>
                        <div>
                          {c.title} <span className="badge">{c.tool}</span>
                        </div>
                        <div className="muted">
                          {c.path}
                          {c.exists ? ` · ${fmtBytes(c.size)}` : " · 目录不存在"}
                        </div>
                      </div>
                      <button
                        className="btn danger"
                        disabled={!c.exists || cacheBusy}
                        onClick={async () => {
                          if (
                            !(await confirm({
                              title: `清理 ${c.title}？`,
                              body: `${c.path}\n约 ${fmtBytes(c.size)}，删除后不可恢复。`,
                              danger: true,
                              okText: "清理",
                            }))
                          ) {
                            return;
                          }
                          setCacheBusy(true);
                          setErr("");
                          try {
                            const cleaned = await api.cleanToolCache(c.id);
                            ok(cleaned);
                            setCaches(await api.listToolCaches());
                          } catch (e) {
                            fail(e);
                          } finally {
                            setCacheBusy(false);
                          }
                        }}
                      >
                        清理
                      </button>
                    </div>
                  ))}
                </div>
              </>
            )}

            {section === "baseline" && <BackupPage section="baseline" />}
            {section === "backup" && <BackupPage section="backup" />}

            {section === "storage" && (
              <>
                <p className="muted" style={{ marginTop: 0 }}>
                  启动前在用户环境变量里设置 <code>ENVT_PATH</code>（例如 <code>d:\envts-data</code>），本面板会把该路径当作根目录。
                  未配置则使用 <code>%APPDATA%\envts</code>（即{" "}
                  <code>C:\Users\本机\AppData\Roaming\envts</code>）。改 ENVT_PATH 后需重新打开本程序。
                </p>
                <div className="path-row" style={{ marginBottom: 8 }}>
                  <label>当前根目录</label>
                  <input value={paths.root_dir} readOnly />
                  <span className={`badge ${paths.from_envt_path ? "ok" : "warn"}`}>
                    {paths.from_envt_path ? "ENVT_PATH" : "APPDATA"}
                  </span>
                </div>
                {paths.envt_path && <div className="muted">ENVT_PATH = {paths.envt_path}</div>}
                <h4 className="settings-subhead">三目录（paths.json 写在根目录下）</h4>
                {(
                  [
                    ["config_dir", "核心配置目录"],
                    ["data_dir", "数据存储目录"],
                    ["log_dir", "日志目录"],
                  ] as ["config_dir" | "data_dir" | "log_dir", string][]
                ).map(([k, label]) => (
                  <div className="path-row" key={k} style={{ marginBottom: 8 }}>
                    <label>{label}</label>
                    <input value={paths[k]} onChange={(e) => setPaths({ ...paths, [k]: e.target.value })} />
                    <button className="btn" onClick={() => void browse(k)}>
                      浏览
                    </button>
                  </div>
                ))}
                <label>
                  <input type="checkbox" checked={migrate} onChange={(e) => setMigrate(e.target.checked)} />{" "}
                  变更时复制旧目录内容（不删除旧目录）
                </label>
                <div className="row" style={{ marginTop: 10 }}>
                  <button
                    className="btn primary"
                    onClick={async () => {
                      setErr("");
                      try {
                        const n = await api.setPaths({ ...paths, migrate });
                        setPaths(n);
                        ok("目录已更新");
                      } catch (e) {
                        fail(e);
                      }
                    }}
                  >
                    保存目录
                  </button>
                </div>
              </>
            )}

            {section === "misc" && (
              <>
                <p className="muted" style={{ marginTop: 0 }}>
                  Node / Python 安装包放在数据根目录的 <code>runtimes/</code>，不额外写 FNM_DIR / PYENV_*。
                </p>
                <label>
                  <input
                    type="checkbox"
                    checked={prefs.flush_dns_after_hosts}
                    onChange={(e) => setPrefs({ ...prefs, flush_dns_after_hosts: e.target.checked })}
                  />{" "}
                  应用 hosts 后执行 ipconfig /flushdns
                </label>
                <label style={{ marginTop: 8, display: "block" }}>
                  <input
                    type="checkbox"
                    checked={prefs.minimize_to_tray}
                    onChange={(e) => setPrefs({ ...prefs, minimize_to_tray: e.target.checked })}
                  />{" "}
                  最小化时收进托盘并释放 WebView2（闲置 2 分钟后释放约 150–250MB，再点托盘重建页面）
                </label>
                <div className="row" style={{ marginTop: 8 }}>
                  <label>保留备份份数</label>
                  <input
                    type="number"
                    value={prefs.backup_keep}
                    onChange={(e) => setPrefs({ ...prefs, backup_keep: Number(e.target.value) || 10 })}
                  />
                </div>
                <button
                  className="btn primary"
                  style={{ marginTop: 10 }}
                  onClick={async () => {
                    try {
                      await savePrefs(prefs);
                      ok("偏好已保存");
                    } catch (e) {
                      fail(e);
                    }
                  }}
                >
                  保存偏好
                </button>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
