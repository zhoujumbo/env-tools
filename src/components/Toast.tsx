import { createContext, useCallback, useContext, useState, type ReactNode } from "react";

export type ToastKind = "ok" | "err" | "info";

export interface ToastItem {
  id: number;
  kind: ToastKind;
  text: string;
}

type ToastFn = (kind: ToastKind, text: string) => void;

const Ctx = createContext<ToastFn>(() => {});

let seq = 1;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);

  const push = useCallback<ToastFn>((kind, text) => {
    const t = String(text ?? "").trim();
    if (!t) return;
    const id = seq++;
    const short = t.length > 160 ? `${t.slice(0, 160)}…` : t;
    setItems((xs) => [...xs.slice(-4), { id, kind, text: short }]);
    window.setTimeout(() => {
      setItems((xs) => xs.filter((x) => x.id !== id));
    }, kind === "err" ? 6000 : 3200);
  }, []);

  return (
    <Ctx.Provider value={push}>
      {children}
      <div className="toast-stack" aria-live="polite">
        {items.map((t) => (
          <div key={t.id} className={`toast ${t.kind}`} role="status">
            <b>{t.kind === "err" ? "失败" : t.kind === "ok" ? "成功" : "提示"}</b>
            <div>{t.text}</div>
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}

export function useToast() {
  return useContext(Ctx);
}
