import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { NpmUserConfig, RemoteNodeList, RemoteNodeVersion, RuntimeProgress, RuntimeStatus } from "../types";
import { useConfirm } from "../components/ConfirmDialog";
import { isInstallCancel, RuntimeProgressCard } from "../components/RuntimeProgressCard";
import { useToast } from "../components/Toast";

type Tab = "installed" | "remote";

/// 远程列表首屏只挂这么多行，其余按需「显示更多」。
const REMOTE_PAGE_SIZE = 150;

const MIRRORS: { id: string; label: string; url: string }[] = [
  { id: "npmjs", label: "官方", url: "https://registry.npmjs.org/" },
  { id: "npmmirror", label: "npmmirror", url: "https://registry.npmmirror.com" },
  { id: "tencent", label: "腾讯云", url: "https://mirrors.cloud.tencent.com/npm/" },
];

function registryKey(url: string) {
  return url.trim().replace(/\/+$/, "").toLowerCase();
}

function fmtCacheAge(ts: number | null) {
  if (!ts) return "";
  const sec = Math.max(0, Math.floor(Date.now() / 1000 - ts));
  if (sec < 60) return "刚刚";
  if (sec < 3600) return `${Math.floor(sec / 60)} 分钟前`;
  if (sec < 86400) return `${Math.floor(sec / 3600)} 小时前`;
  return `${Math.floor(sec / 86400)} 天前`;
}

function brief(s: string) {
  const t = s.trim();
  if (!t) return "";
  const line = t.split("\n").filter(Boolean).pop() || t;
  return line.length > 90 ? `${line.slice(0, 90)}…` : line;
}

export default function NodePage() {
  const confirm = useConfirm();
  const toast = useToast();
  const [st, setSt] = useState<RuntimeStatus | null>(null);
  const [npmCfg, setNpmCfg] = useState<NpmUserConfig | null>(null);
  const [registryEdit, setRegistryEdit] = useState("");
  const [ver, setVer] = useState("");
  const [tab, setTab] = useState<Tab>("installed");
  const [openRt, setOpenRt] = useState(false);
  const [remote, setRemote] = useState<RemoteNodeList | null>(null);
  const [remoteQ, setRemoteQ] = useState("");
  const [ltsOnly, setLtsOnly] = useState(false);
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [remoteLimit, setRemoteLimit] = useState(REMOTE_PAGE_SIZE);
  const [npmEdit, setNpmEdit] = useState<string | null>(null);
  const [npmVer, setNpmVer] = useState("");
  const [err, setErr] = useState("");
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [progress, setProgress] = useState<RuntimeProgress | null>(null);

  async function load() {
    setSt(await api.nodeStatus());
    try {
      const cfg = await api.nodeNpmConfig();
      setNpmCfg(cfg);
      setRegistryEdit(cfg.registry || "");
    } catch {
      setNpmCfg(null);
    }
  }
  useEffect(() => {
    void load();
  }, []);

  useEffect(() => {
    let gone = false;
    let unlisten: (() => void) | undefined;
    void listen<RuntimeProgress>("runtime-progress", (ev) => {
      if (ev.payload.runtime !== "node") return;
      setProgress(ev.payload);
    }).then((fn) => {
      if (gone) fn();
      else unlisten = fn;
    });
    return () => {
      gone = true;
      unlisten?.();
    };
  }, []);

  async function wrap(
    fn: () => Promise<string | RuntimeStatus | NpmUserConfig | void>,
    seed?: { op: string; version: string; message: string },
  ) {
    setBusy(true);
    setStopping(false);
    setErr("");
    setMsg("");
    if (seed) {
      setProgress({
        runtime: "node",
        op: seed.op,
        phase: "resolve",
        version: seed.version,
        percent: 8,
        bytes: null,
        total: null,
        message: seed.message,
      });
    }
    try {
      const r = await fn();
      if (typeof r === "string") {
        setMsg(r);
        toast("ok", brief(r) || "完成");
      } else if (r && typeof r === "object" && "detail" in r && r.detail) {
        setMsg(r.detail);
        toast("ok", r.detail);
      } else if (r && typeof r === "object" && "current" in r) {
        toast("ok", r.current ? `当前 ${r.kind ?? "运行时"} ${r.current}` : "完成");
      } else {
        toast("ok", seed ? seed.message.replace(/^安装/, "已安装") : "完成");
      }
      await load();
    } catch (e) {
      if (isInstallCancel(e)) {
        setProgress((p) =>
          p ? { ...p, phase: "cancelled", percent: 100, message: "已停止，已清理安装残余" } : p,
        );
        setMsg("已停止");
        toast("info", "已停止，已清理安装残余");
        await load();
      } else {
        setErr(String(e));
        toast("err", String(e));
        setProgress((p) => (p ? { ...p, phase: "error", message: String(e) } : p));
      }
    } finally {
      setBusy(false);
      setStopping(false);
    }
  }

  async function loadRemote(force: boolean) {
    setRemoteLoading(true);
    setErr("");
    try {
      setRemote(await api.nodeListRemote(force));
    } catch (e) {
      setErr(String(e));
      toast("err", String(e));
    } finally {
      setRemoteLoading(false);
    }
  }

  useEffect(() => {
    if (tab !== "remote") return;
    if (remote) return;
    void loadRemote(false);
    // 只在切入远程 tab 时拉一次；失败走刷新按钮，避免空结果死循环
  }, [tab]);

  const installed = new Set((st?.versions ?? []).map((v) => v.toLowerCase()));
  const filteredRemote = useMemo(() => {
    const q = remoteQ.trim().toLowerCase().replace(/^v/, "");
    return (remote?.versions ?? []).filter((r) => {
      if (ltsOnly && !r.lts) return false;
      if (!q) return true;
      return r.version.toLowerCase().includes(q) || (r.lts ?? "").toLowerCase().includes(q);
    });
  }, [remote, remoteQ, ltsOnly]);

  // 换筛选条件就回到首屏，避免上次展开的几百行又全量渲染。
  useEffect(() => {
    setRemoteLimit(REMOTE_PAGE_SIZE);
  }, [remoteQ, ltsOnly]);

  const shownRemote = useMemo(
    () => filteredRemote.slice(0, remoteLimit),
    [filteredRemote, remoteLimit],
  );

  const npmOf = (v: string) => st?.version_meta?.find((m) => m.version === v)?.npm ?? null;
  const pnpmOf = (v: string) => {
    const m = st?.version_meta?.find((x) => x.version === v);
    return m && m.pnpm != null ? m.pnpm : null;
  };

  return (
    <div className="col">
      <div className="card">
        <button type="button" className="collapse-head" onClick={() => setOpenRt((v) => !v)}>
          <span className="chevron">{openRt ? "▼" : "▶"}</span>
          <h3>本机 Node 运行时</h3>
          <span className="runtime-summary">
            {st ? (
              <>
                {st.current ?? "未设置 current"}
                <span className="muted"> · PATH {st.path_ok ? "已接入" : "未写入"}</span>
                <span className="muted"> · {st.versions.length} 个版本</span>
              </>
            ) : (
              <span className="muted">读取中…</span>
            )}
          </span>
        </button>
        {openRt && (
          <>
            <p className="muted">
              zip 解到 <code>runtimes/node/versions</code>，默认是 <code>current</code> junction。
              用户 PATH 只 prepend 这一条。版本号 <code>22.14.0</code> / <code>v22.14.0</code> 均可。
            </p>
            {st && (
              <pre className="mono">
{`root=${st.root}
current=${st.current ?? "-"}
current_path=${st.current_path ?? "-"}
path_ok=${st.path_ok}
path=${st.path_entries.join(";")}`}
              </pre>
            )}
          </>
        )}
      </div>

      {npmCfg && (
        <div className="card">
          <h3>npm 配置</h3>
          {npmCfg.available ? (
            <>
              <div className="npm-kv">
                <span className="muted">registry</span>
                <span className="mono">{npmCfg.registry || "—"}</span>
                <span className="muted">prefix</span>
                <span>
                  <span className="mono">{npmCfg.prefix || "—"}</span>{" "}
                  <span className={`badge ${npmCfg.prefix_ok ? "ok" : "warn"}`}>
                    {npmCfg.prefix_ok ? (npmCfg.custom ? "面板自定义" : "系统默认") : "与面板不一致"}
                  </span>
                </span>
                <span className="muted">cache</span>
                <span>
                  <span className="mono">{npmCfg.cache || "—"}</span>{" "}
                  <span className={`badge ${npmCfg.cache_ok ? "ok" : "warn"}`}>
                    {npmCfg.cache_ok ? (npmCfg.custom ? "面板自定义" : "系统默认") : "与面板不一致"}
                  </span>
                </span>
              </div>
              {(!npmCfg.prefix_ok || !npmCfg.cache_ok) && (
                <div className="row" style={{ marginTop: 8 }}>
                  <span className="muted">
                    目标 prefix {npmCfg.prefix_desired} · cache {npmCfg.cache_desired}
                  </span>
                  <button className="btn" disabled={busy} onClick={() => void wrap(() => api.nodeSyncNpmConfig())}>
                    立即同步
                  </button>
                </div>
              )}
              <p className="muted" style={{ marginTop: 12 }}>
                第三方镜像示例：<code>npm config set registry https://registry.npmmirror.com</code>
              </p>
              <div className="mirror-picks">
                {MIRRORS.map((m) => (
                  <button
                    type="button"
                    key={m.id}
                    className={`btn ${registryKey(npmCfg.registry) === registryKey(m.url) ? "primary" : ""}`}
                    disabled={busy}
                    onClick={() => void wrap(() => api.nodeSetRegistry(m.url))}
                  >
                    {m.label}
                  </button>
                ))}
              </div>
              <div className="row" style={{ marginTop: 8 }}>
                <input
                  placeholder="https://registry.npmmirror.com"
                  value={registryEdit}
                  onChange={(e) => setRegistryEdit(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && registryEdit.trim() && !busy) {
                      void wrap(() => api.nodeSetRegistry(registryEdit.trim()));
                    }
                  }}
                />
                <button
                  className="btn primary"
                  disabled={busy || !registryEdit.trim()}
                  onClick={() => void wrap(() => api.nodeSetRegistry(registryEdit.trim()))}
                >
                  应用镜像
                </button>
              </div>
            </>
          ) : (
            <p className="muted">{npmCfg.detail || "没有 current Node，无法读取 npm config"}</p>
          )}
        </div>
      )}

      {progress && (
        <RuntimeProgressCard
          progress={progress}
          busy={busy}
          stopping={stopping}
          onStopping={() => {
            setStopping(true);
            setProgress((p) => (p ? { ...p, message: "正在停止…" } : p));
          }}
        />
      )}

      <div className="card">
        <div className="tab-bar">
          <button type="button" className={`tab ${tab === "installed" ? "active" : ""}`} onClick={() => setTab("installed")}>
            已安装{st ? ` (${st.versions.length})` : ""}
          </button>
          <button type="button" className={`tab ${tab === "remote" ? "active" : ""}`} onClick={() => setTab("remote")}>
            远程列表
          </button>
          {tab === "remote" && (
            <button
              type="button"
              className="btn icon-btn"
              title="刷新远程列表"
              disabled={remoteLoading}
              onClick={() => void loadRemote(true)}
            >
              <RefreshIcon spinning={remoteLoading} />
            </button>
          )}
        </div>

        {tab === "installed" && (
          <>
            <div className="row" style={{ marginTop: 10 }}>
              <input
                placeholder="安装版本，22.14.0 或 v22.14.0"
                value={ver}
                onChange={(e) => setVer(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && ver && !busy) {
                    void wrap(() => api.nodeInstall(ver), {
                      op: "install",
                      version: ver,
                      message: `安装 ${ver}`,
                    });
                  }
                }}
              />
              <button
                className="btn primary"
                disabled={busy || !ver.trim()}
                onClick={() =>
                  void wrap(() => api.nodeInstall(ver), {
                    op: "install",
                    version: ver,
                    message: `安装 ${ver}`,
                  })
                }
              >
                安装
              </button>
            </div>
            <div className="list" style={{ maxHeight: 420, marginTop: 10 }}>
              {(st?.versions ?? []).length === 0 && <div className="muted">还没有本地面板安装的 Node</div>}
              {(st?.versions ?? []).map((v) => (
                <InstalledRow
                  key={v}
                  version={v}
                  current={st?.current === v}
                  npm={npmOf(v)}
                  pnpm={pnpmOf(v)}
                  pnpmCmdOk={!!st?.pnpm_cmd_ok && st?.current === v}
                  pnpmNeedsFix={!!st?.pnpm_needs_fix && st?.current === v}
                  busy={busy}
                  editing={npmEdit === v}
                  npmVer={npmVer}
                  onNpmVer={setNpmVer}
                  onToggleNpm={() => {
                    if (npmEdit === v) {
                      setNpmEdit(null);
                    } else {
                      setNpmEdit(v);
                      setNpmVer(npmOf(v) ?? "");
                    }
                  }}
                  onSetCurrent={() => void wrap(() => api.nodeSetCurrent(v))}
                  onUninstall={() => void wrap(() => api.nodeUninstall(v))}
                  onCacheClean={
                    st?.current === v
                      ? () =>
                          void (async () => {
                            if (
                              !(await confirm({
                                title: "清理 npm cache？",
                                body: "执行 npm cache clean --force\n只清缓存，不动全局包。",
                                danger: true,
                                okText: "清理",
                              }))
                            ) {
                              return;
                            }
                            await wrap(() => api.nodeNpmCacheClean());
                          })()
                      : undefined
                  }
                  onNpmSave={() =>
                    void wrap(
                      async () => {
                        const out = await api.nodeNpmInstall(v, npmVer);
                        setNpmEdit(null);
                        return out || `npm → ${npmVer}`;
                      },
                      { op: "npm", version: v, message: `安装 npm ${npmVer}` },
                    )
                  }
                  onPnpmInstall={() =>
                    void wrap(() => api.nodePnpmInstall(v), {
                      op: "pnpm",
                      version: v,
                      message: `安装 pnpm 到 ${v}`,
                    })
                  }
                  onFixPnpm={() =>
                    void wrap(async () => {
                      const env = await api.nodeFixPnpm();
                      if (env.needs_fix) throw new Error(env.detail || "pnpm 仍不可用，请新开 cmd 再试");
                      if (!env.installed) return env.detail || "PATH 已对齐当前布局。当前没有 pnpm，请点「装 pnpm」";
                      return env.detail || "PATH 已对齐当前布局。已打开的 cmd 不会变，请新开窗口再试 pnpm -v";
                    })
                  }
                />
              ))}
            </div>
          </>
        )}

        {tab === "remote" && (
          <>
            <div className="row" style={{ marginTop: 10 }}>
              <input placeholder="筛选版本 / LTS 名" value={remoteQ} onChange={(e) => setRemoteQ(e.target.value)} />
              <label className="row" style={{ gap: 6 }}>
                <input type="checkbox" checked={ltsOnly} onChange={(e) => setLtsOnly(e.target.checked)} />
                仅 LTS
              </label>
              <span className="muted">
                {remoteLoading
                  ? "加载中…"
                  : remote
                    ? `${remote.stale ? "离线缓存 · " : remote.from_cache ? "缓存 · " : "已刷新 · "}${fmtCacheAge(remote.fetched_at)} · ${filteredRemote.length} 项`
                    : ""}
              </span>
            </div>
            <div className="list" style={{ maxHeight: 420, marginTop: 10 }}>
              {shownRemote.map((r) => (
                <RemoteRow
                  key={r.version}
                  item={r}
                  installed={installed.has(r.version.toLowerCase())}
                  busy={busy}
                  onInstall={() =>
                    void wrap(() => api.nodeInstall(r.version), {
                      op: "install",
                      version: r.version,
                      message: `安装 ${r.version}`,
                    })
                  }
                />
              ))}
              {filteredRemote.length > shownRemote.length && (
                <div className="vlist-more">
                  <button
                    type="button"
                    className="btn"
                    onClick={() => setRemoteLimit((n) => n + REMOTE_PAGE_SIZE)}
                  >
                    显示更多（还有 {filteredRemote.length - shownRemote.length} 项）
                  </button>
                </div>
              )}
            </div>
          </>
        )}
      </div>
      {err && <div className="err">{err}</div>}
      {msg && <pre className="mono">{msg}</pre>}
    </div>
  );
}

function InstalledRow(props: {
  version: string;
  current: boolean;
  npm: string | null;
  pnpm: string | null;
  pnpmCmdOk?: boolean;
  pnpmNeedsFix?: boolean;
  busy: boolean;
  editing: boolean;
  npmVer: string;
  onNpmVer: (v: string) => void;
  onToggleNpm: () => void;
  onSetCurrent: () => void;
  onUninstall: () => void;
  onNpmSave: () => void;
  onPnpmInstall: () => void;
  onFixPnpm?: () => void;
  onCacheClean?: () => void;
}) {
  return (
    <div className="list-item" style={{ flexDirection: "column", alignItems: "stretch", cursor: "default" }}>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <div className="row" style={{ flexWrap: "wrap" }}>
          {props.version}
          {props.current ? <span className="badge ok">current</span> : null}
          <span className="muted"> · npm {props.npm ?? "—"}</span>
          {props.pnpm != null || props.pnpmNeedsFix || props.pnpmCmdOk ? (
            <>
              <span className={`muted ${props.pnpmNeedsFix ? "err" : ""}`}>
                {" "}
                · pnpm
                {props.pnpm ? ` ${props.pnpm}` : ""}
                {props.pnpmNeedsFix
                  ? " 环境不一致"
                  : !props.pnpm && props.pnpmCmdOk
                    ? " 可用"
                    : ""}
              </span>
              {props.pnpmNeedsFix && (
                <button
                  className="btn"
                  disabled={props.busy}
                  title="同步 PATH / PNPM_HOME 到当前全局目录"
                  onClick={props.onFixPnpm}
                >
                  修复环境
                </button>
              )}
            </>
          ) : (
            <button
              className="btn"
              disabled={props.busy}
              title="corepack enable / npm i -g pnpm"
              onClick={props.onPnpmInstall}
            >
              装 pnpm
            </button>
          )}
          {props.onCacheClean && (
            <button
              className="btn"
              disabled={props.busy}
              title="npm cache clean --force"
              onClick={props.onCacheClean}
            >
              清缓存
            </button>
          )}
        </div>
        <div className="row">
          {!props.current && (
            <button className="btn primary" disabled={props.busy} onClick={props.onSetCurrent}>
              设为 current
            </button>
          )}
          <button className="btn" disabled={props.busy} onClick={props.onToggleNpm}>
            {props.editing ? "取消" : "改 npm"}
          </button>
          <button className="btn danger" disabled={props.busy} onClick={props.onUninstall}>
            卸载
          </button>
        </div>
      </div>
      {props.editing && (
        <div className="row npm-edit">
          <input
            placeholder="npm 版本，如 10.9.2"
            value={props.npmVer}
            onChange={(e) => props.onNpmVer(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && props.npmVer.trim()) props.onNpmSave();
            }}
          />
          <button className="btn primary" disabled={props.busy || !props.npmVer.trim()} onClick={props.onNpmSave}>
            应用
          </button>
        </div>
      )}
    </div>
  );
}

function RemoteRow(props: { item: RemoteNodeVersion; installed: boolean; busy: boolean; onInstall: () => void }) {
  return (
    <div className="list-item vlist-item" style={{ cursor: "default" }}>
      <div>
        {props.item.version} {props.item.lts ? <span className="badge ok">{props.item.lts}</span> : null}
        {props.installed ? <span className="muted"> · 已安装</span> : null}
      </div>
      <button className="btn" disabled={props.busy || props.installed} onClick={props.onInstall}>
        {props.installed ? "已安装" : "安装"}
      </button>
    </div>
  );
}

function RefreshIcon({ spinning }: { spinning: boolean }) {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      className={spinning ? "spin" : undefined}
      aria-hidden
    >
      <path
        d="M13.5 8A5.5 5.5 0 1 1 8 2.5"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
      <path d="M8 1v3.2L10.6 2.8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
