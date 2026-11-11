import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { EnvChange, EnvVar } from "../types";
import { useConfirm } from "../components/ConfirmDialog";
import { useToast } from "../components/Toast";

const PROTECTED = new Set([
  "SYSTEMROOT",
  "WINDIR",
  "COMSPEC",
  "PATHEXT",
  "OS",
  "PROCESSOR_ARCHITECTURE",
  "PROCESSOR_IDENTIFIER",
  "NUMBER_OF_PROCESSORS",
  "PROMPT",
]);

const ENV_DRAFT_KEY = "envdesk:env-draft";

interface EnvDraft {
  name: string;
  value: string;
}

function readEnvDraft(): EnvDraft | null {
  try {
    const raw = localStorage.getItem(ENV_DRAFT_KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as EnvDraft;
    return typeof v?.name === "string" && typeof v?.value === "string" ? v : null;
  } catch {
    return null;
  }
}

function writeEnvDraft(d: EnvDraft | null) {
  try {
    if (d) localStorage.setItem(ENV_DRAFT_KEY, JSON.stringify(d));
    else localStorage.removeItem(ENV_DRAFT_KEY);
  } catch {
    /* ignore */
  }
}

export default function EnvPage() {
  const confirm = useConfirm();
  const toast = useToast();
  const [user, setUser] = useState<EnvVar[]>([]);
  const [system, setSystem] = useState<EnvVar[]>([]);
  const [tab, setTab] = useState<"user" | "system">("user");
  const [q, setQ] = useState("");
  const [newName, setNewName] = useState("");
  const [newVal, setNewVal] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [draftVal, setDraftVal] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [msg, setMsg] = useState("");
  const [pendingDraft, setPendingDraft] = useState<EnvDraft | null>(null);

  async function load() {
    const s = await api.envSnapshot();
    setUser(s.user);
    setSystem(s.system);
  }

  useEffect(() => {
    void load();
  }, []);

  // 编辑中的单行值先落盘，webview 被闲置释放后回来还能捡。
  const hadEditing = useRef(false);

  useEffect(() => {
    const d = readEnvDraft();
    if (!d) return;
    setTab("user");
    setEditing(d.name);
    setDraftVal(d.value);
    setPendingDraft(d);
  }, []);

  useEffect(() => {
    if (!editing) {
      if (hadEditing.current) {
        writeEnvDraft(null);
        hadEditing.current = false;
      }
      return;
    }
    hadEditing.current = true;
    const t = window.setTimeout(() => writeEnvDraft({ name: editing, value: draftVal }), 400);
    return () => window.clearTimeout(t);
  }, [editing, draftVal]);

  const visible = useMemo(() => {
    const src = tab === "user" ? user : system;
    const qq = q.trim().toLowerCase();
    return src.filter((v) => !qq || v.name.toLowerCase().includes(qq) || v.value.toLowerCase().includes(qq));
  }, [tab, user, system, q]);

  async function commit(changes: EnvChange[], okMsg: string) {
    setBusy(true);
    setErr("");
    try {
      await api.applyEnvChanges(changes);
      setMsg(okMsg);
      toast("ok", okMsg);
      setEditing(null);
      setDraftVal("");
      await load();
    } catch (e) {
      setErr(String(e));
      toast("err", String(e));
    } finally {
      setBusy(false);
    }
  }

  function startEdit(v: EnvVar) {
    if (PROTECTED.has(v.name.toUpperCase())) {
      toast("err", "受保护变量禁止修改");
      return;
    }
    if (editing && editing !== v.name) {
      toast("err", "请先保存或取消当前编辑");
      return;
    }
    setEditing(v.name);
    setDraftVal(v.value);
    setErr("");
    setMsg("");
  }

  function cancelEdit() {
    setEditing(null);
    setDraftVal("");
  }

  async function saveEdit(name: string) {
    const orig = user.find((u) => u.name === name)?.value;
    if (orig === draftVal) {
      cancelEdit();
      return;
    }
    await commit([{ name, action: "set", value: draftVal }], `已更新 ${name}（新开进程生效）`);
  }

  async function addVar() {
    const name = newName.trim();
    if (!name || busy) return;
    if (PROTECTED.has(name.toUpperCase())) {
      setErr("受保护变量禁止修改");
      toast("err", "受保护变量禁止修改");
      return;
    }
    if (user.some((v) => v.name.toLowerCase() === name.toLowerCase())) {
      setErr(`已存在 ${name}`);
      toast("err", `已存在 ${name}`);
      return;
    }
    if (editing) {
      toast("err", "请先保存或取消当前编辑");
      return;
    }
    await commit([{ name, action: "set", value: newVal }], `已添加 ${name}`);
    setNewName("");
    setNewVal("");
  }

  async function removeVar(name: string) {
    if (editing) {
      toast("err", "请先保存或取消当前编辑");
      return;
    }
    if (
      !(await confirm({
        title: "删除变量？",
        body: `将从用户环境变量删除 ${name}，立即写入注册表。\n新开的进程才会读到新值。`,
        danger: true,
        okText: "删除并生效",
      }))
    ) {
      return;
    }
    await commit([{ name, action: "delete" }], `已删除 ${name}`);
  }

  return (
    <div className="col env-page">
      <div className="card">
        <div className="tab-bar">
          <button type="button" className={`tab ${tab === "user" ? "active" : ""}`} onClick={() => setTab("user")}>
            用户变量
          </button>
          <button type="button" className={`tab ${tab === "system" ? "active" : ""}`} onClick={() => setTab("system")}>
            系统变量
          </button>
          <input className="env-filter" placeholder="过滤名称 / 值" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        {tab === "system" && (
          <p className="muted" style={{ marginTop: 10 }}>
            系统变量只读（HKLM）。要改请用 Windows「系统属性 → 环境变量」。
          </p>
        )}
        {tab === "user" && (
          <p className="muted" style={{ marginTop: 10 }}>
            点行尾 ✎ 编辑，✓ 立即写入 HKCU 并广播 WM_SETTINGCHANGE；已打开的终端需新开才生效。
          </p>
        )}
      </div>

      {err && <div className="err">{err}</div>}
      {msg && <div className="ok">{msg}</div>}
      {pendingDraft && (
        <div className="ok row" style={{ justifyContent: "space-between", gap: 8 }}>
          <span>已恢复上次编辑 {pendingDraft.name} 的未保存改动。</span>
          <button
            type="button"
            className="btn"
            onClick={() => {
              setPendingDraft(null);
              cancelEdit();
            }}
          >
            放弃这些修改
          </button>
        </div>
      )}

      {tab === "user" && (
        <div className="card env-add">
          <input
            placeholder="变量名"
            value={newName}
            disabled={busy}
            onChange={(e) => setNewName(e.target.value)}
          />
          <input
            placeholder="值"
            value={newVal}
            disabled={busy}
            onChange={(e) => setNewVal(e.target.value)}
          />
          <button className="btn primary" disabled={!newName.trim() || busy} onClick={() => void addVar()}>
            添加并生效
          </button>
        </div>
      )}

      <div className="card env-list-card">
        {visible.length === 0 && <div className="muted">没有匹配的变量</div>}
        {visible.map((v) => {
          const prot = PROTECTED.has(v.name.toUpperCase());
          const isEditing = editing === v.name;
          const isPath = v.name.toUpperCase() === "PATH";
          const val = isEditing ? draftVal : v.value;
          return (
            <div key={v.name} className={`env-row ${isEditing ? "is-edited" : ""}`}>
              <div className="env-name">
                <code>{v.name}</code>
                {prot && <span className="badge warn">保护</span>}
                {isEditing && <span className="badge warn">编辑中</span>}
              </div>
              <div className="env-val">
                {tab === "user" && isEditing ? (
                  isPath ? (
                    <PathEditor value={val} disabled={busy} onChange={setDraftVal} />
                  ) : (
                    <input
                      value={val}
                      disabled={busy}
                      onChange={(e) => setDraftVal(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") void saveEdit(v.name);
                        if (e.key === "Escape") cancelEdit();
                      }}
                    />
                  )
                ) : (
                  <span className="mono env-ro">{v.value}</span>
                )}
              </div>
              {tab === "user" && !prot && (
                <div className="env-actions">
                  {isEditing ? (
                    <>
                      <button
                        type="button"
                        className="btn hosts-text-btn ok"
                        title="保存并生效"
                        disabled={busy}
                        onClick={() => void saveEdit(v.name)}
                      >
                        ✓
                      </button>
                      <button
                        type="button"
                        className="btn hosts-text-btn"
                        title="取消"
                        disabled={busy}
                        onClick={cancelEdit}
                      >
                        ✕
                      </button>
                    </>
                  ) : (
                    <>
                      <button
                        type="button"
                        className="btn hosts-text-btn"
                        title="编辑"
                        disabled={busy || !!editing}
                        onClick={() => startEdit(v)}
                      >
                        编辑
                      </button>
                      <button
                        type="button"
                        className="btn danger icon-btn"
                        title="删除"
                        disabled={busy || !!editing}
                        onClick={() => void removeVar(v.name)}
                      >
                        ×
                      </button>
                    </>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function PathEditor({
  value,
  onChange,
  disabled,
}: {
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  const parts = value.split(";").filter((p) => p.length);
  function move(i: number, dir: number) {
    const j = i + dir;
    if (j < 0 || j >= parts.length) return;
    const n = [...parts];
    [n[i], n[j]] = [n[j], n[i]];
    onChange(n.join(";"));
  }
  return (
    <div className="path-ed">
      {parts.map((p, i) => (
        <div key={`${p}-${i}`} className="path-ed-row">
          <input
            value={p}
            disabled={disabled}
            onChange={(e) => {
              const n = [...parts];
              n[i] = e.target.value;
              onChange(n.join(";"));
            }}
          />
          <button type="button" className="btn icon-btn" title="上移" disabled={disabled} onClick={() => move(i, -1)}>
            ↑
          </button>
          <button type="button" className="btn icon-btn" title="下移" disabled={disabled} onClick={() => move(i, 1)}>
            ↓
          </button>
          <button
            type="button"
            className="btn danger icon-btn"
            title="去掉这条路径"
            disabled={disabled}
            onClick={() => onChange(parts.filter((_, k) => k !== i).join(";"))}
          >
            ×
          </button>
        </div>
      ))}
      <button type="button" className="btn" disabled={disabled} onClick={() => onChange([...parts, ""].join(";"))}>
        添加路径
      </button>
    </div>
  );
}
