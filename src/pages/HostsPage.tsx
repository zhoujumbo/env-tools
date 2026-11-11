import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { HostsEntry, TrashItem } from "../types";
import { useConfirm } from "../components/ConfirmDialog";
import { useToast } from "../components/Toast";

type Sel = "system" | "draft" | "trash" | { id: string };

const DRAFT_BODY = "# 127.0.0.1 example.local\n";
const DRAFT_KEY = "envdesk:hosts-draft";

interface SavedDraft {
  title: string;
  content: string;
  on: boolean;
  sel: Sel;
}

function readDraft(): SavedDraft | null {
  try {
    const raw = localStorage.getItem(DRAFT_KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as SavedDraft;
    if (typeof v?.content !== "string" || typeof v?.title !== "string") return null;
    return { title: v.title, content: v.content, on: !!v.on, sel: v.sel ?? "draft" };
  } catch {
    return null;
  }
}

function writeDraft(d: SavedDraft | null) {
  try {
    if (d) localStorage.setItem(DRAFT_KEY, JSON.stringify(d));
    else localStorage.removeItem(DRAFT_KEY);
  } catch {
    /* ignore */
  }
}

export default function HostsPage() {
  const confirm = useConfirm();
  const toast = useToast();
  const [entries, setEntries] = useState<HostsEntry[]>([]);
  const [trash, setTrash] = useState<TrashItem[]>([]);
  const [sel, setSel] = useState<Sel>("system");
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [on, setOn] = useState(false);
  const [sys, setSys] = useState("");
  const [editing, setEditing] = useState(false);
  const [baseline, setBaseline] = useState<{ title: string; content: string; on: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [msg, setMsg] = useState("");
  const [pendingDraft, setPendingDraft] = useState<SavedDraft | null>(null);

  async function refresh() {
    const [e, t] = await api.listHosts();
    setEntries(e);
    setTrash(t);
    try {
      setSys(await api.readSystemHosts());
    } catch {
      setSys("(无法读取系统 hosts)");
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  const dirty =
    editing &&
    baseline != null &&
    (title !== baseline.title || content !== baseline.content || on !== baseline.on);

  // webview 闲置会被销毁重建：编辑中的内容先落到 localStorage，回来还能捡。
  const hadEditing = useRef(false);

  // 挂载时如果还有草稿，直接恢复成编辑态（而不是让用户再点一次「恢复」）。
  useEffect(() => {
    const d = readDraft();
    if (!d) return;
    setSel(d.sel);
    setTitle(d.title);
    setContent(d.content);
    setOn(d.on);
    setBaseline(null);
    setEditing(true);
    setPendingDraft(d);
  }, []);

  useEffect(() => {
    if (!editing) {
      // 只有「这次会话里编辑过」才清盘，否则刚挂载就把上次的草稿擦掉了。
      if (hadEditing.current) {
        writeDraft(null);
        hadEditing.current = false;
      }
      return;
    }
    hadEditing.current = true;
    if (!dirty && baseline != null) return;
    const t = window.setTimeout(() => {
      writeDraft({ title, content, on, sel });
    }, 400);
    return () => window.clearTimeout(t);
  }, [editing, dirty, baseline, title, content, on, sel]);

  function dropPendingDraft() {
    writeDraft(null);
    setPendingDraft(null);
    void cancelEdit();
  }

  async function leaveIfDirty() {
    if (!dirty) return true;
    return confirm({
      title: "放弃未保存修改？",
      body: "当前方案有未保存内容，离开后会丢掉。",
      danger: true,
      okText: "放弃",
    });
  }

  async function select(next: Sel) {
    if (sameSel(sel, next)) return;
    if (!(await leaveIfDirty())) return;
    setErr("");
    setMsg("");
    setEditing(false);
    setBaseline(null);
    if (next === "system") {
      setSel("system");
      setTitle("");
      setContent("");
      setOn(false);
      return;
    }
    if (next === "trash") {
      setSel("trash");
      return;
    }
    if (next === "draft") {
      beginDraft("新方案", DRAFT_BODY, false);
      return;
    }
    setSel(next);
    const e = entries.find((x) => x.id === next.id);
    const body = await api.getHostsContent(next.id);
    setTitle(e?.title ?? "");
    setOn(e?.on ?? false);
    setContent(body);
  }

  function beginDraft(name: string, body: string, enabled: boolean) {
    const draft = { title: name, content: body, on: enabled };
    setSel("draft");
    setTitle(draft.title);
    setContent(draft.content);
    setOn(draft.on);
    setBaseline(draft);
    setEditing(true);
    setErr("");
    setMsg("");
  }

  async function editFromSystem() {
    if (!(await leaveIfDirty())) return;
    const body = sys && !sys.startsWith("(无法读取") ? sys : DRAFT_BODY;
    beginDraft(nextCopyTitle("系统方案", entries.map((x) => x.title)), body, true);
  }

  async function copyFromSystem() {
    if (!(await leaveIfDirty())) return;
    setBusy(true);
    setErr("");
    try {
      const body = sys && !sys.startsWith("(无法读取") ? sys : DRAFT_BODY;
      const name = nextCopyTitle("系统方案", entries.map((x) => x.title));
      const created = await api.saveHostsEntry({
        id: null,
        title: name,
        content: body,
        on: false,
      });
      await refresh();
      setSel({ id: created.id });
      setTitle(created.title);
      setOn(false);
      setContent(body);
      setEditing(false);
      setBaseline(null);
      toast("ok", `已复制为方案「${name}」`);
      setMsg(`已复制为方案「${name}」（默认未启用）`);
    } catch (e) {
      setErr(String(e));
      toast("err", String(e));
    } finally {
      setBusy(false);
    }
  }

  async function startEdit(id: string) {
    if (typeof sel === "object" && sel.id === id && editing) return;
    if (!(await leaveIfDirty())) return;
    const e = entries.find((x) => x.id === id);
    if (!e) return;
    const body = await api.getHostsContent(id);
    const snap = { title: e.title, content: body, on: e.on };
    setSel({ id });
    setTitle(snap.title);
    setContent(snap.content);
    setOn(snap.on);
    setBaseline(snap);
    setEditing(true);
    setErr("");
    setMsg("");
  }

  async function cancelEdit() {
    if (dirty) {
      const ok = await confirm({
        title: "取消编辑？",
        body: "未保存的修改会丢掉。",
        danger: true,
        okText: "取消编辑",
      });
      if (!ok) return;
    }
    if (sel === "draft") {
      setEditing(false);
      setBaseline(null);
      setSel("system");
      setTitle("");
      setContent("");
      setOn(false);
      return;
    }
    if (baseline) {
      setTitle(baseline.title);
      setContent(baseline.content);
      setOn(baseline.on);
    }
    setEditing(false);
    setBaseline(null);
    setErr("");
    setMsg("");
  }

  async function applySystem(okMsg: string) {
    const o = await api.applyHosts();
    const tip = o.unchanged ? "方案已保存，系统 hosts 无需变更" : okMsg;
    setMsg(tip);
    toast("ok", tip);
  }

  async function saveEdit() {
    if (!editing || (sel !== "draft" && !(typeof sel === "object" && "id" in sel))) return;
    setBusy(true);
    setErr("");
    try {
      const id = sel === "draft" ? null : sel.id;
      const e = await api.saveHostsEntry({
        id,
        title: title.trim() || "未命名方案",
        content,
        on,
      });
      await applySystem("已保存并写入系统 hosts（已刷新 DNS）");
      await refresh();
      setSel({ id: e.id });
      setTitle(e.title);
      setOn(e.on);
      setContent(await api.getHostsContent(e.id));
      setEditing(false);
      setBaseline(null);
    } catch (e) {
      setErr(String(e));
      toast("err", String(e));
    } finally {
      setBusy(false);
    }
  }

  async function toggleOn(e: HostsEntry, next: boolean) {
    if (editing && typeof sel === "object" && sel.id === e.id) {
      setOn(next);
      return;
    }
    if (!(await leaveIfDirty())) return;
    setBusy(true);
    setErr("");
    try {
      await api.setHostsOn(e.id, next);
      await applySystem(next ? `已启用「${e.title}」并写入系统` : `已关闭「${e.title}」并写入系统`);
      await refresh();
      if (typeof sel === "object" && sel.id === e.id) setOn(next);
    } catch (err) {
      setErr(String(err));
      toast("err", String(err));
    } finally {
      setBusy(false);
    }
  }

  async function duplicate(e: HostsEntry) {
    if (!(await leaveIfDirty())) return;
    setBusy(true);
    setErr("");
    try {
      const body = await api.getHostsContent(e.id);
      const name = nextCopyTitle(e.title, entries.map((x) => x.title));
      const created = await api.saveHostsEntry({
        id: null,
        title: name,
        content: body,
        on: false,
      });
      await refresh();
      setSel({ id: created.id });
      setTitle(created.title);
      setOn(false);
      setContent(body);
      setEditing(false);
      setBaseline(null);
      toast("ok", `已复制为「${name}」`);
      setMsg(`已复制为「${name}」（默认未启用）`);
    } catch (err) {
      setErr(String(err));
      toast("err", String(err));
    } finally {
      setBusy(false);
    }
  }

  const viewingEntry = typeof sel === "object" && "id" in sel;
  const showEditor = sel === "draft" || viewingEntry;

  return (
    <div className="hosts-page">
      {err && <div className="err">{err}</div>}
      {msg && <div className="ok">{msg}</div>}
      {pendingDraft && (
        <div className="ok row" style={{ justifyContent: "space-between", gap: 8 }}>
          <span>
            已恢复上次未保存的编辑「{pendingDraft.title || "新方案"}」（最小化/关闭后内容还在）。
          </span>
          <button type="button" className="btn" onClick={dropPendingDraft}>
            放弃这些修改
          </button>
        </div>
      )}
      <div className="hosts-layout">
        <div className="card hosts-side">
          <div className="hosts-side-head">
            <h3>方案</h3>
            <button
              type="button"
              className="btn"
              disabled={busy}
              onClick={() => void select("draft")}
            >
              新建
            </button>
          </div>
          <div className="hosts-side-list">
            <div
              className={`hosts-item ${sel === "system" ? "active" : ""}`}
              role="button"
              tabIndex={0}
              onClick={() => void select("system")}
              onKeyDown={(ev) => {
                if (ev.key === "Enter" || ev.key === " ") {
                  ev.preventDefault();
                  void select("system");
                }
              }}
            >
              <span className="hosts-dot sys" />
              <span className="hosts-item-body">
                <div>当前系统 Hosts</div>
                <div className="muted">磁盘生效预览 · 只读</div>
              </span>
              <div className="hosts-item-actions" onClick={(ev) => ev.stopPropagation()}>
                <button
                  type="button"
                  className="btn hosts-text-btn"
                  title="复制为新方案"
                  disabled={busy}
                  onClick={() => void copyFromSystem()}
                >
                  复制
                </button>
                <button
                  type="button"
                  className="btn hosts-text-btn"
                  title="基于系统内容新建并编辑"
                  disabled={busy}
                  onClick={() => void editFromSystem()}
                >
                  编辑
                </button>
              </div>
            </div>

            {sel === "draft" && (
              <div className={`hosts-item active ${editing ? "is-editing" : ""}`}>
                <span className="hosts-dot" />
                <span className="hosts-item-body">
                  <div>{title || "新方案"}</div>
                  <div className="muted">未保存草稿</div>
                </span>
                <div className="hosts-item-actions" onClick={(ev) => ev.stopPropagation()}>
                  <button
                    type="button"
                    className="btn hosts-text-btn ok"
                    title="保存并应用到系统"
                    disabled={busy}
                    onClick={() => void saveEdit()}
                  >
                    ✓
                  </button>
                  <button
                    type="button"
                    className="btn hosts-text-btn"
                    title="取消"
                    disabled={busy}
                    onClick={() => void cancelEdit()}
                  >
                    ✕
                  </button>
                </div>
              </div>
            )}

            {entries.length === 0 && sel !== "draft" && (
              <div className="hosts-empty muted">还没有方案。点上方「新建」，或用系统项的「编辑 / 复制」。</div>
            )}

            {entries.map((e) => {
              const active = viewingEntry && sel.id === e.id;
              const isEditingThis = active && editing;
              return (
                <div
                  key={e.id}
                  className={`hosts-item ${active ? "active" : ""} ${isEditingThis ? "is-editing" : ""}`}
                  role="button"
                  tabIndex={0}
                  onClick={() => void select({ id: e.id })}
                  onKeyDown={(ev) => {
                    if (ev.key === "Enter" || ev.key === " ") {
                      ev.preventDefault();
                      void select({ id: e.id });
                    }
                  }}
                >
                  <span className={`hosts-dot ${e.on ? "on" : ""}`} />
                  <span className="hosts-item-body">
                    <div>{isEditingThis ? title || e.title : e.title}</div>
                    <div className="muted">
                      {isEditingThis
                        ? dirty
                          ? "编辑中 · 未保存"
                          : "编辑中"
                        : e.on
                          ? "已启用 · 写入系统"
                          : "未启用"}
                    </div>
                  </span>
                  <div className="hosts-item-actions" onClick={(ev) => ev.stopPropagation()}>
                    {!isEditingThis && (
                      <label className="hosts-toggle" title={e.on ? "关闭并写入系统" : "启用并写入系统"}>
                        <input
                          type="checkbox"
                          checked={e.on}
                          disabled={busy}
                          onChange={(ev) => void toggleOn(e, ev.target.checked)}
                        />
                      </label>
                    )}
                    {isEditingThis ? (
                      <>
                        <button
                          type="button"
                          className="btn hosts-text-btn ok"
                          title="保存并应用到系统"
                          disabled={busy}
                          onClick={() => void saveEdit()}
                        >
                          ✓
                        </button>
                        <button
                          type="button"
                          className="btn hosts-text-btn"
                          title="取消"
                          disabled={busy}
                          onClick={() => void cancelEdit()}
                        >
                          ✕
                        </button>
                      </>
                    ) : (
                      <>
                        <button
                          type="button"
                          className="btn hosts-text-btn"
                          title="复制一份"
                          disabled={busy}
                          onClick={() => void duplicate(e)}
                        >
                          复制
                        </button>
                        <button
                          type="button"
                          className="btn hosts-text-btn"
                          title="编辑"
                          disabled={busy}
                          onClick={() => void startEdit(e.id)}
                        >
                          编辑
                        </button>
                      </>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
          <button
            type="button"
            className={`hosts-trash-btn ${sel === "trash" ? "active" : ""}`}
            onClick={() => void select("trash")}
          >
            回收站 {trash.length ? `(${trash.length})` : ""}
          </button>
        </div>

        <div className="card hosts-main">
          {sel === "system" && (
            <>
              <div className="hosts-main-head">
                <div>
                  <h3>当前系统 Hosts</h3>
                  <div className="muted">
                    C:\Windows\System32\drivers\etc\hosts。只读；点左侧「编辑」基于当前内容建方案并保存后写入。
                  </div>
                </div>
                <button className="btn" disabled={busy} onClick={() => void copyFromSystem()}>
                  复制为方案
                </button>
                <button className="btn primary" disabled={busy} onClick={() => void editFromSystem()}>
                  编辑
                </button>
              </div>
              <div className="hosts-body">
                <pre className="mono hosts-readonly">{sys || "（空）"}</pre>
              </div>
            </>
          )}

          {showEditor && (
            <>
              <div className="hosts-main-head">
                {editing ? (
                  <input
                    className="hosts-title"
                    value={title}
                    disabled={busy}
                    onChange={(e) => setTitle(e.target.value)}
                    placeholder="方案名称"
                  />
                ) : (
                  <div>
                    <h3>{title || "方案"}</h3>
                    <div className="muted">{on ? "已启用" : "未启用"} · 点左侧「编辑」修改</div>
                  </div>
                )}
                {editing && (
                  <label className="row" style={{ gap: 6 }}>
                    <input
                      type="checkbox"
                      checked={on}
                      disabled={busy}
                      onChange={(e) => setOn(e.target.checked)}
                    />
                    启用（保存时写入系统）
                  </label>
                )}
                {viewingEntry && !editing && (
                  <button
                    className="btn danger"
                    disabled={busy}
                    onClick={async () => {
                      if (
                        !(await confirm({
                          title: "移入回收站？",
                          body: `「${title}」会从方案列表拿掉，可在回收站恢复。`,
                          danger: true,
                          okText: "移入回收站",
                        }))
                      ) {
                        return;
                      }
                      try {
                        await api.trashHosts(sel.id);
                        const wasOn = on;
                        setSel("system");
                        setEditing(false);
                        setBaseline(null);
                        await refresh();
                        if (wasOn) {
                          await applySystem("已删除启用方案并更新系统 hosts");
                          await refresh();
                        } else {
                          toast("ok", "已移入回收站");
                        }
                      } catch (e) {
                        toast("err", String(e));
                      }
                    }}
                  >
                    删除
                  </button>
                )}
              </div>
              <div className="hosts-body">
                {editing ? (
                  <textarea
                    className="hosts-editor"
                    value={content}
                    disabled={busy}
                    onChange={(e) => setContent(e.target.value)}
                    placeholder="# 127.0.0.1 example.local"
                  />
                ) : (
                  <pre className="mono hosts-readonly">{content || "（空）"}</pre>
                )}
              </div>
              {editing && dirty && <div className="muted">未保存 · 点左侧 ✓ 写入系统</div>}
            </>
          )}

          {sel === "trash" && (
            <>
              <div className="hosts-main-head">
                <h3>回收站</h3>
              </div>
              <div className="hosts-body" style={{ padding: 8 }}>
                {trash.length === 0 && <div className="muted">空</div>}
                <div className="hosts-side-list">
                  {trash.map((t) => (
                    <div key={t.id} className="list-item" style={{ cursor: "default" }}>
                      <div>
                        <div>{t.title}</div>
                        <div className="muted">{t.deleted_at}</div>
                      </div>
                      <div className="row">
                        <button
                          className="btn"
                          onClick={async () => {
                            try {
                              await api.restoreHostsTrash(t.id);
                              await refresh();
                              toast("ok", "已恢复方案");
                            } catch (e) {
                              toast("err", String(e));
                            }
                          }}
                        >
                          恢复
                        </button>
                        <button
                          className="btn danger"
                          onClick={async () => {
                            if (
                              !(await confirm({
                                title: "彻底删除？",
                                body: `「${t.title}」无法恢复。`,
                                danger: true,
                                okText: "彻底删除",
                              }))
                            ) {
                              return;
                            }
                            try {
                              await api.purgeHostsTrash(t.id);
                              await refresh();
                              toast("ok", "已彻底删除");
                            } catch (e) {
                              toast("err", String(e));
                            }
                          }}
                        >
                          彻底删除
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function sameSel(a: Sel, b: Sel) {
  if (a === b) return true;
  return typeof a === "object" && typeof b === "object" && a.id === b.id;
}

function nextCopyTitle(base: string, titles: string[]): string {
  const lower = new Set(titles.map((t) => t.toLowerCase()));
  if (!lower.has(base.toLowerCase())) return base;
  let n = 2;
  while (lower.has(`${base}${n}`.toLowerCase())) n += 1;
  return `${base}${n}`;
}
