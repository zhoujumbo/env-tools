import type { RuntimeProgress } from "../types";
import { api } from "../api";

export const RUNTIME_PHASE_LABEL: Record<string, string> = {
  resolve: "准备",
  download: "下载",
  extract: "解压",
  link: "设置 current",
  npm: "npm",
  pnpm: "pnpm",
  pip: "pip",
  done: "完成",
  error: "失败",
  cancelled: "已停止",
};

function fmtBytes(n: number) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function isInstallCancel(e: unknown) {
  const s = String(e);
  return s.includes("已取消") || s.includes("已停止");
}

export function canStopProgress(p: RuntimeProgress | null, busy: boolean) {
  if (!busy || !p) return false;
  return p.phase !== "done" && p.phase !== "error" && p.phase !== "cancelled";
}

export function RuntimeProgressCard(props: {
  progress: RuntimeProgress;
  busy: boolean;
  stopping: boolean;
  onStopping: () => void;
}) {
  const { progress, busy, stopping, onStopping } = props;
  const pct = progress.percent ?? (progress.phase !== "done" ? 8 : 0);
  const stoppable = canStopProgress(progress, busy);

  return (
    <div className="card progress-card">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <div>
          <b>{RUNTIME_PHASE_LABEL[progress.phase] ?? progress.phase}</b>
          <span className="muted"> · {progress.version}</span>
        </div>
        <div className="row">
          <span className="muted">
            {progress.bytes != null
              ? `${fmtBytes(progress.bytes)}${progress.total ? ` / ${fmtBytes(progress.total)}` : ""}`
              : progress.percent != null
                ? `${Math.round(progress.percent)}%`
                : ""}
          </span>
          {stoppable && (
            <button
              type="button"
              className="btn danger"
              disabled={stopping}
              onClick={() => {
                onStopping();
                void api.runtimeCancel();
              }}
            >
              {stopping ? "正在停止…" : "停止"}
            </button>
          )}
        </div>
      </div>
      <div className="progress-track">
        <i style={{ width: `${Math.max(progress.phase === "done" || progress.phase === "cancelled" ? 100 : pct, 4)}%` }} />
      </div>
      <div className="muted">{progress.message}</div>
    </div>
  );
}
