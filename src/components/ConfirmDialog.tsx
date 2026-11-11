import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";

export interface ConfirmOpts {
  title: string;
  body: string;
  okText?: string;
  cancelText?: string;
  danger?: boolean;
}

type Pending = ConfirmOpts & { resolve: (ok: boolean) => void };

const Ctx = createContext<(opts: ConfirmOpts) => Promise<boolean>>(async () => false);

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [pending, setPending] = useState<Pending | null>(null);

  const confirm = useCallback((opts: ConfirmOpts) => {
    return new Promise<boolean>((resolve) => setPending({ ...opts, resolve }));
  }, []);

  function close(ok: boolean) {
    pending?.resolve(ok);
    setPending(null);
  }

  useEffect(() => {
    if (!pending) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pending]);

  return (
    <Ctx.Provider value={confirm}>
      {children}
      {pending && (
        <div className="modal-back" onClick={() => close(false)}>
          <div className="modal confirm-modal" onClick={(e) => e.stopPropagation()}>
            <h3>{pending.title}</h3>
            <p className="muted" style={{ whiteSpace: "pre-wrap" }}>
              {pending.body}
            </p>
            <div className="row" style={{ marginTop: 16, justifyContent: "flex-end" }}>
              <button className="btn" onClick={() => close(false)}>
                {pending.cancelText ?? "取消"}
              </button>
              <button className={`btn ${pending.danger ? "danger" : "primary"}`} onClick={() => close(true)}>
                {pending.okText ?? "确认"}
              </button>
            </div>
          </div>
        </div>
      )}
    </Ctx.Provider>
  );
}

export function useConfirm() {
  return useContext(Ctx);
}
