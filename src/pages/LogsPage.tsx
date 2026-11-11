import { useEffect, useState } from "react";
import { api } from "../api";
import type { OpRecord, RuntimeLogFile } from "../types";

type Tab = "ops" | "runtime";

function fmtBytes(n: number) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function fmtTs(sec: number | null) {
  if (!sec) return "";
  const d = new Date(sec * 1000);
  return d.toLocaleString();
}

export default function LogsPage() {
  const [tab, setTab] = useState<Tab>("ops");
  const [rows, setRows] = useState<OpRecord[]>([]);
  const [kind, setKind] = useState("");
  const [open, setOpen] = useState<OpRecord | null>(null);
  const [body, setBody] = useState("");
  const [files, setFiles] = useState<RuntimeLogFile[]>([]);
  const [file, setFile] = useState<RuntimeLogFile | null>(null);
  const [fileBody, setFileBody] = useState("");
  const [err, setErr] = useState("");

  async function loadOps() {
    setRows(await api.listOps(kind || undefined, 80));
  }
  async function loadFiles() {
    setFiles(await api.listRuntimeLogs());
  }

  useEffect(() => {
    if (tab === "ops") void loadOps();
  }, [tab, kind]);

  useEffect(() => {
    if (tab === "runtime") void loadFiles();
  }, [tab]);

  async function inspect(r: OpRecord) {
    setOpen(r);
    let t = "";
    if (r.stdout_path) {
      try {
        t += "=== stdout ===\n" + (await api.readTextFile(r.stdout_path)) + "\n";
      } catch {
        /* */
      }
    }
    if (r.stderr_path) {
      try {
        t += "=== stderr ===\n" + (await api.readTextFile(r.stderr_path)) + "\n";
      } catch {
        /* */
      }
    }
    if (r.before_json) t += "=== before ===\n" + r.before_json + "\n";
    if (r.after_json) t += "=== after ===\n" + r.after_json + "\n";
    if (r.undo_json) t += "=== undo ===\n" + r.undo_json + "\n";
    setBody(t);
  }

  async function openFile(f: RuntimeLogFile) {
    setFile(f);
    setErr("");
    try {
      setFileBody(await api.readTextFile(f.path));
    } catch (e) {
      setFileBody("");
      setErr(String(e));
    }
  }

  return (
    <div className="col">
      <div className="card">
        <div className="tab-bar">
          <button type="button" className={`tab ${tab === "ops" ? "active" : ""}`} onClick={() => setTab("ops")}>
            操作日志
          </button>
          <button type="button" className={`tab ${tab === "runtime" ? "active" : ""}`} onClick={() => setTab("runtime")}>
            运行日志
          </button>
          <button
            type="button"
            className="btn icon-btn"
            title="刷新"
            onClick={() => void (tab === "ops" ? loadOps() : loadFiles())}
          >
            刷新
          </button>
        </div>
        {tab === "ops" ? (
          <p className="muted" style={{ marginTop: 10 }}>
            面板做过的事：装版本、改 hosts、写环境变量。点一行看前后对比和命令输出。
          </p>
        ) : (
          <p className="muted" style={{ marginTop: 10 }}>
            日志目录里的文件：app.log 是面板自身输出，stdout/stderr 是外部命令抓下来的原文。
          </p>
        )}
      </div>

      {tab === "ops" && (
        <>
          <div className="row">
            <input placeholder="按 kind 过滤，如 hosts / env / node / python" value={kind} onChange={(e) => setKind(e.target.value)} />
          </div>
          <div className="card">
            <div className="log-table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>时间</th>
                    <th>kind</th>
                    <th>结果</th>
                    <th>命令</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((r) => (
                    <tr
                      key={r.id}
                      onClick={() => void inspect(r)}
                      style={{ cursor: "pointer" }}
                      className={open?.id === r.id ? "is-selected" : undefined}
                    >
                      <td className="muted">{r.ts}</td>
                      <td>{r.kind}</td>
                      <td>
                        <span className={`badge ${r.ok ? "ok" : "error"}`}>
                          {r.ok ? "ok" : "fail"} {r.exit_code}
                        </span>
                      </td>
                      <td className="mono">{r.cmdline}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
          {open && (
            <div className="card">
              <h3>
                {open.kind} · {open.id}
              </h3>
              <pre className="mono log-body">{body || "(无附加输出)"}</pre>
            </div>
          )}
        </>
      )}

      {tab === "runtime" && (
        <div className="hosts-layout">
          <div className="card hosts-side">
            <h3>文件</h3>
            <div className="hosts-side-list">
              {files.length === 0 && <div className="muted">日志目录还是空的</div>}
              {files.map((f) => (
                <button
                  type="button"
                  key={f.path}
                  className={`hosts-item ${file?.path === f.path ? "active" : ""}`}
                  onClick={() => void openFile(f)}
                >
                  <span className="hosts-item-body">
                    <div>{f.name}</div>
                    <div className="muted">
                      {fmtBytes(f.size)} · {fmtTs(f.modified)}
                    </div>
                  </span>
                </button>
              ))}
            </div>
          </div>
          <div className="card hosts-main">
            <div className="hosts-main-head">
              <h3>{file?.name ?? "选择一个文件"}</h3>
            </div>
            {err && <div className="err">{err}</div>}
            <pre className="mono hosts-readonly">{fileBody || (file ? "(空文件)" : "点左侧文件查看内容")}</pre>
          </div>
        </div>
      )}
    </div>
  );
}
