import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type {
  JavaEnvReport,
  RemoteNodeList,
  RemoteNodeVersion,
  RuntimeProgress,
  RuntimeStatus,
} from "../types";
import { isInstallCancel, RuntimeProgressCard } from "../components/RuntimeProgressCard";
import { useToast } from "../components/Toast";
import { useConfirm } from "../components/ConfirmDialog";

type Tab = "installed" | "remote";

/// 远程列表首屏只挂这么多行，其余按需「显示更多」。
const REMOTE_PAGE_SIZE = 150;

function fmtCacheAge(ts: number | null) {
  if (!ts) return "";
  const sec = Math.max(0, Math.floor(Date.now() / 1000 - ts));
  if (sec < 60) return "刚刚";
  if (sec < 3600) return `${Math.floor(sec / 60)} 分钟前`;
  if (sec < 86400) return `${Math.floor(sec / 3600)} 小时前`;
  return `${Math.floor(sec / 86400)} 天前`;
}

export default function JavaPage() {
  const toast = useToast();
  const confirm = useConfirm();
  const [st, setSt] = useState<RuntimeStatus | null>(null);
  const [envRep, setEnvRep] = useState<JavaEnvReport | null>(null);
  const [ver, setVer] = useState("");
  const [tab, setTab] = useState<Tab>("installed");
  const [openRt, setOpenRt] = useState(false);
  const [remote, setRemote] = useState<RemoteNodeList | null>(null);
  const [remoteQ, setRemoteQ] = useState("");
  const [latestOnly, setLatestOnly] = useState(true);
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [remoteLimit, setRemoteLimit] = useState(REMOTE_PAGE_SIZE);
  const [err, setErr] = useState("");
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [progress, setProgress] = useState<RuntimeProgress | null>(null);

  async function load() {
    setSt(await api.javaStatus());
    try {
      setEnvRep(await api.javaEnvReport());
    } catch {
      setEnvRep(null);
    }
  }
  useEffect(() => {
    void load();
  }, []);

  useEffect(() => {
    let gone = false;
    let unlisten: (() => void) | undefined;
    void listen<RuntimeProgress>("runtime-progress", (ev) => {
      if (ev.payload.runtime !== "java") return;
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
    fn: () => Promise<string | RuntimeStatus | void>,
    seed?: { op: string; version: string; message: string },
  ) {
    setBusy(true);
    setStopping(false);
    setErr("");
    setMsg("");
    if (seed) {
      setProgress({
        runtime: "java",
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
        toast("ok", r.split("\n").filter(Boolean).pop() || "完成");
      } else if (r && typeof r === "object" && "current" in r) {
        toast("ok", r.current ? `当前 Java ${r.current}` : "完成");
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
      setRemote(await api.javaListRemote(force));
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
  }, [tab]);

  const installed = new Set((st?.versions ?? []).map((v) => v.toLowerCase()));
  const filteredRemote = useMemo(() => {
    const q = remoteQ.trim().toLowerCase();
    return (remote?.versions ?? []).filter((r) => {
      if (latestOnly && !r.lts) return false;
      if (!q) return true;
      return r.version.toLowerCase().includes(q) || (r.lts ?? "").toLowerCase().includes(q);
    });
  }, [remote, remoteQ, latestOnly]);

  useEffect(() => {
    setRemoteLimit(REMOTE_PAGE_SIZE);
  }, [remoteQ, latestOnly]);

  const shownRemote = useMemo(
    () => filteredRemote.slice(0, remoteLimit),
    [filteredRemote, remoteLimit],
  );

  return (
    <div className="col">
      <div className="card">
        <button type="button" className="collapse-head" onClick={() => setOpenRt((v) => !v)}>
          <span className="chevron">{openRt ? "▼" : "▶"}</span>
          <h3>本机 Java 运行时</h3>
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
              清华镜像 <code>mirrors.tuna.tsinghua.edu.cn/Adoptium</code> 的 Temurin JDK zip，解到{" "}
              <code>runtimes/java/versions</code>，<code>current</code> junction。PATH prepend{" "}
              <code>current\bin</code>，用户级 <code>JAVA_HOME</code> 指向 <code>current</code>。版本号如{" "}
              <code>21.0.13_9</code> / <code>8u512b09</code>。
              已打开的终端不会自动刷新；若机器上还有系统级 <code>JAVA_HOME</code>（例如其它 JDK 安装器写的），IDE /
              旧窗口可能仍走旧版本——新开 cmd 或清掉系统 <code>JAVA_HOME</code>。
            </p>
            {st && (
              <pre className="mono">
{`root=${st.root}
current=${st.current ?? "-"}
current_path=${st.current_path ?? "-"}
path_ok=${st.path_ok}
cmd_which=${st.cmd_which ?? "-"}
path=${st.path_entries.join(";")}`}
              </pre>
            )}
          </>
        )}
      </div>

      {envRep?.needs_claim && (
        <div className="err">
          检测到本机还有面板外的 Java 环境在抢 PATH / JAVA_HOME
          {envRep.machine_java_home ? (
            <>
              ：系统 <code>JAVA_HOME={envRep.machine_java_home}</code>
            </>
          ) : null}
          {envRep.foreign_install_hint ? (
            <>
              （来源提示：<code>{envRep.foreign_install_hint}</code>）
            </>
          ) : null}
          。点下面收归后，只由本面板的 <code>current</code> 生效（可能弹 UAC）。磁盘上的旧 JDK 目录不会自动删除。
          <div className="row" style={{ marginTop: 8 }}>
            <button
              className="btn primary"
              disabled={busy || !st?.current}
              onClick={() =>
                void wrap(async () => {
                  if (
                    !(await confirm({
                      title: "收归 Java 到本面板？",
                      body: "将：\n· 用户 JAVA_HOME → 面板 current\n· 用户 PATH 去掉其它 Java bin，置顶 current\\bin\n· 系统 JAVA_HOME / 系统 PATH 里的外站 Java 条目清掉（需 UAC）\n\n不会删除 D:\\ProgramData\\Java 等安装目录，只是不再被环境变量引用。新开终端后 java -version 应为本面板版本。",
                      okText: "收归并生效",
                      danger: true,
                    }))
                  ) {
                    return;
                  }
                  const r = await api.javaClaimEnv();
                  setEnvRep(r);
                  return r.needs_claim
                    ? "已处理，但仍有残留冲突，请新开终端再查"
                    : "已收归：JAVA_HOME / PATH 指向本面板 current";
                })
              }
            >
              收归本面板管理
            </button>
          </div>
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
                placeholder="安装版本，21.0.13_9"
                value={ver}
                onChange={(e) => setVer(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && ver && !busy) {
                    void wrap(() => api.javaInstall(ver), {
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
                  void wrap(() => api.javaInstall(ver), {
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
              {(st?.versions ?? []).length === 0 && <div className="muted">还没有本地面板安装的 Java</div>}
              {(st?.versions ?? []).map((v) => (
                <InstalledRow
                  key={v}
                  version={v}
                  current={st?.current === v}
                  busy={busy}
                  onSetCurrent={() => void wrap(() => api.javaSetCurrent(v))}
                  onUninstall={() => void wrap(() => api.javaUninstall(v))}
                />
              ))}
            </div>
          </>
        )}

        {tab === "remote" && (
          <>
            <div className="row" style={{ marginTop: 10 }}>
              <input placeholder="筛选版本 / 主版本" value={remoteQ} onChange={(e) => setRemoteQ(e.target.value)} />
              <label className="row" style={{ gap: 6 }}>
                <input type="checkbox" checked={latestOnly} onChange={(e) => setLatestOnly(e.target.checked)} />
                仅各主版本最新
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
                    void wrap(() => api.javaInstall(r.version), {
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
  busy: boolean;
  onSetCurrent: () => void;
  onUninstall: () => void;
}) {
  return (
    <div className="list-item" style={{ cursor: "default" }}>
      <div>
        {props.version}
        {props.current ? <span className="badge ok">current</span> : null}
      </div>
      <div className="row">
        {!props.current && (
          <button className="btn primary" disabled={props.busy} onClick={props.onSetCurrent}>
            设为 current
          </button>
        )}
        <button className="btn danger" disabled={props.busy} onClick={props.onUninstall}>
          卸载
        </button>
      </div>
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
