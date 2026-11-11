import { useEffect, useState } from "react";
import { api } from "./api";
import type { PageId, AppMemory } from "./types";
import HostsPage from "./pages/HostsPage";
import EnvPage from "./pages/EnvPage";
import NodePage from "./pages/NodePage";
import PythonPage from "./pages/PythonPage";
import JavaPage from "./pages/JavaPage";
import GolangPage from "./pages/GolangPage";
import LogsPage from "./pages/LogsPage";
import SettingsPage from "./pages/SettingsPage";
import { applyTheme, loadThemePref, watchSystemTheme, type ThemePref } from "./theme";
import { useToast } from "./components/Toast";
import PageBoundary from "./components/PageBoundary";

const NAV: { id: PageId; label: string }[] = [
  { id: "hosts", label: "Hosts" },
  { id: "env", label: "环境变量" },
  { id: "node", label: "Node" },
  { id: "python", label: "Python" },
  { id: "java", label: "Java" },
  { id: "golang", label: "Golang" },
  { id: "logs", label: "日志" },
];

const TITLES: Record<PageId, string> = {
  hosts: "Hosts 方案",
  env: "用户环境变量",
  node: "Node.js 运行时",
  python: "Python 运行时",
  java: "Java 运行时",
  golang: "Golang 运行时",
  logs: "日志",
  settings: "设置",
};

const PAGE_KEY = "envdesk:page";
const PAGES: PageId[] = [
  "hosts",
  "env",
  "node",
  "python",
  "java",
  "golang",
  "logs",
  "settings",
];

function loadPage(): PageId {
  const v = localStorage.getItem(PAGE_KEY);
  return v && (PAGES as string[]).includes(v) ? (v as PageId) : "hosts";
}

function savePage(p: PageId) {
  try {
    localStorage.setItem(PAGE_KEY, p);
  } catch {
    /* ignore */
  }
}

export default function App() {
  const toast = useToast();
  // 窗口闲置会被销毁重建，页面停在哪存 localStorage，回来不迷路。
  const [page, setPage] = useState<PageId>(loadPage);
  const [themePref, setThemePref] = useState<ThemePref>(loadThemePref());

  const go = (p: PageId) => {
    setPage(p);
    savePage(p);
  };

  useEffect(() => {
    (async () => {
      try {
        if (await api.isFirstRun()) {
          await api.completeFirstRun();
        }
        const prefs = await api.getPrefs();
        const t = (prefs.theme === "light" || prefs.theme === "system" ? prefs.theme : "dark") as ThemePref;
        setThemePref(t);
        applyTheme(t);
      } catch (e) {
        toast("err", String(e));
      }
    })();
  }, []);

  useEffect(() => {
    applyTheme(themePref);
    return watchSystemTheme(themePref, () => applyTheme(themePref));
  }, [themePref]);

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <h1>环境面板</h1>
          <p>Windows · 可回滚</p>
        </div>
        {NAV.map((n) => (
          <button
            key={n.id}
            className={`nav-btn ${page === n.id ? "active" : ""}`}
            onClick={() => go(n.id)}
          >
            {n.label}
          </button>
        ))}
        <div className="sidebar-foot">
          <button
            type="button"
            className={`nav-btn sidebar-settings ${page === "settings" ? "active" : ""}`}
            onClick={() => go("settings")}
          >
            设置
          </button>
          <MemMeter />
        </div>
      </aside>
      <section className="main">
        <header className="topbar">
          <h2>{TITLES[page]}</h2>
        </header>
        <div className="content" key={page}>
          <PageBoundary key={page}>
            {page === "hosts" && <HostsPage />}
            {page === "env" && <EnvPage />}
            {page === "node" && <NodePage />}
            {page === "python" && <PythonPage />}
            {page === "java" && <JavaPage />}
            {page === "golang" && <GolangPage />}
            {page === "logs" && <LogsPage />}
            {page === "settings" && <SettingsPage themePref={themePref} onTheme={setThemePref} />}
          </PageBoundary>
        </div>
      </section>
    </div>
  );
}

function formatMb(bytes: number) {
  if (!bytes) return "--";
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function MemMeter() {
  const [mem, setMem] = useState<AppMemory | null>(null);

  useEffect(() => {
    let alive = true;
    const tick = async () => {
      if (document.hidden) return;
      try {
        const m = await api.appMemory();
        if (alive) setMem(m);
      } catch {
        /* ignore */
      }
    };
    void tick();
    // 30s 一次：这个数只是给用户看着安心的，8s 一次等于用功耗换数字。
    const id = window.setInterval(tick, 30000);
    const onVis = () => {
      if (!document.hidden) void tick();
    };
    document.addEventListener("visibilitychange", onVis);
    return () => {
      alive = false;
      window.clearInterval(id);
      document.removeEventListener("visibilitychange", onVis);
    };
  }, []);

  const title = mem
    ? `合计 ${formatMb(mem.working_set)} · Rust ${formatMb(mem.rust_working_set)} · WebView ${formatMb(mem.webview_working_set)} · ${mem.processes} 进程`
    : "采样中";

  return (
    <div className="mem-meter" title={title}>
      <span className="mem-label">内存</span>
      <span className="mem-value">{mem ? formatMb(mem.working_set) : "…"}</span>
    </div>
  );
}
