import { useEffect, useState } from "react";
import { api } from "../api";
import type { BackupInfo, BaselineManifest } from "../types";
import { useConfirm } from "../components/ConfirmDialog";
import { useToast } from "../components/Toast";

export default function BackupPage(props: { section: "baseline" | "backup" }) {
  const { section } = props;
  const confirm = useConfirm();
  const toast = useToast();
  const [list, setList] = useState<BackupInfo[]>([]);
  const [base, setBase] = useState<BaselineManifest | null>(null);
  const [runtime, setRuntime] = useState(false);
  const [err, setErr] = useState("");
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);

  async function load() {
    setList(await api.listBackups());
    setBase(await api.baselineStatus());
  }
  useEffect(() => {
    void load();
  }, []);

  return (
    <div className="col">
      {section === "baseline" && (
        <>
          <p className="muted" style={{ marginTop: 0 }}>
            第一次打开面板时拍了一张「当时的 OS 表面」快照：系统 hosts、用户环境变量、CMD AutoRun、PowerShell
            profile。这不是 Windows 安装盘还原，也<strong>不会</strong>删 Node/Python 安装目录。
          </p>
          <p className="muted">
            <b>一键恢复</b>会把 hosts 写回快照、把本面板动过的用户环境变量滚回去、清掉旧壳钩子。已经开着的终端不会自动刷新 PATH。
          </p>
          <p className="muted">
            <b>重新采集</b>会用<strong>现在</strong>的系统状态覆盖那张快照。采完之后，「一键恢复」就回到这一刻，回不到更早。
          </p>
          {base ? (
            <div className="mono baseline-meta">
              采集时间 {base.captured_at}
              {"\n"}hosts {base.hosts_path}
            </div>
          ) : (
            <p>尚未采集</p>
          )}
          <div className="row" style={{ marginTop: 10 }}>
            <button
              className="btn"
              disabled={busy}
              onClick={async () => {
                if (
                  !(await confirm({
                    title: "覆盖原装基线？",
                    body: "用当前 hosts / 用户环境变量 / AutoRun / PS profile 覆盖第一次启动时的快照。之后「一键恢复」只能回到现在，回不到更早。",
                    danger: true,
                    okText: "覆盖采集",
                  }))
                ) {
                  return;
                }
                setBusy(true);
                try {
                  setBase(await api.recaptureBaseline());
                  setMsg("已重新采集基线");
                  toast("ok", "已重新采集基线");
                } catch (e) {
                  setErr(String(e));
                  toast("err", String(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              重新采集基线
            </button>
            <button
              className="btn danger"
              disabled={busy}
              onClick={async () => {
                if (
                  !(await confirm({
                    title: "恢复到原装基线？",
                    body: "将执行：\n· 系统 hosts ← 基线副本（可能 UAC）\n· 本面板写过的用户环境变量 ← 基线值\n· CMD AutoRun / PS profile 清掉本面板痕迹\n\n不会卸载 runtimes 里的 Node/Python。已打开的程序要重开才吃到环境变量。",
                    danger: true,
                    okText: "一键恢复",
                  }))
                ) {
                  return;
                }
                setBusy(true);
                setErr("");
                try {
                  const out = await api.restoreOsSurface();
                  setMsg(out);
                  toast("ok", out);
                } catch (e) {
                  setErr(String(e));
                  toast("err", String(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              一键恢复系统原装
            </button>
          </div>
        </>
      )}

      {section === "backup" && (
        <>
          <p className="muted" style={{ marginTop: 0 }}>
            打 zip：配置、hosts 方案、SQLite 操作记录。默认不含 <code>runtimes/</code> 安装包（体积大）。还原前会再自动打一份当前快照。
          </p>
          <div className="row">
            <label>
              <input type="checkbox" checked={runtime} onChange={(e) => setRuntime(e.target.checked)} /> 含运行时目录
            </label>
            <button
              className="btn primary"
              disabled={busy}
              onClick={async () => {
                if (
                  !(await confirm({
                    title: "立即备份？",
                    body: runtime
                      ? "将打包配置 + hosts + 数据库 + runtimes 安装目录，文件可能很大。"
                      : "将打包配置、hosts 方案和操作库，不含 Node/Python 安装包。",
                    okText: "开始备份",
                  }))
                ) {
                  return;
                }
                setBusy(true);
                try {
                  const b = await api.createBackup(runtime, null);
                  setMsg(`已备份 ${b.path}`);
                  toast("ok", `已备份 ${b.path}`);
                  await load();
                } catch (e) {
                  setErr(String(e));
                  toast("err", String(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              立即备份
            </button>
          </div>
          <div className="list" style={{ maxHeight: 280, marginTop: 10 }}>
            {list.length === 0 && <div className="muted">还没有备份</div>}
            {list.map((b) => (
              <div key={b.path} className="list-item" style={{ cursor: "default" }}>
                <div>
                  <div>{b.name}</div>
                  <div className="muted">
                    {b.created_at} · {(b.size / 1024).toFixed(1)} KB
                  </div>
                </div>
                <button
                  className="btn"
                  disabled={busy}
                  onClick={async () => {
                    if (
                      !(await confirm({
                        title: "还原这份备份？",
                        body: `${b.name}\n还原面板配置 / hosts 方案 / 操作库。还原前会先再备份当前状态。不会动系统原装基线那张快照。`,
                        danger: true,
                        okText: "还原",
                      }))
                    ) {
                      return;
                    }
                    setBusy(true);
                    try {
                      const out = await api.restoreBackup(b.path);
                      setMsg(out);
                      toast("ok", out);
                    } catch (e) {
                      setErr(String(e));
                      toast("err", String(e));
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  还原此备份
                </button>
              </div>
            ))}
          </div>
        </>
      )}

      {err && <div className="err">{err}</div>}
      {msg && <pre className="mono">{msg}</pre>}
    </div>
  );
}
