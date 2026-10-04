import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Language } from "./i18n";

export type ToastKind = "success" | "info" | "error";
export type ToastOperation = {
  finish: (message: string, kind?: ToastKind) => void;
  dismiss: () => void;
  /** Retain a failed request without ending the spinner before independent readback. */
  recordError: (message: string) => void;
};
type Notify = ((message: string, kind?: ToastKind) => void) & {
  loading: (message: string) => ToastOperation;
};
const ToastContext = createContext<Notify>(Object.assign(() => {}, {
  loading: () => ({ finish: () => {}, dismiss: () => {}, recordError: () => {} }),
}));

export function ToastProvider({ children, language }: { children: ReactNode; language: Language }) {
  const [toasts, setToasts] = useState<{ id: number; message: string; kind: ToastKind | "loading"; detail?: string }[]>([]);
  const nextId = useRef(0);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());
  const visible = useRef(new Set<number>());
  const dismiss = useCallback((id: number) => {
    clearTimeout(timers.current.get(id));
    timers.current.delete(id);
    visible.current.delete(id);
    setToasts(previous => previous.filter(toast => toast.id !== id));
  }, []);
  const finish = useCallback((id: number, message: string, kind: ToastKind = "success", detail?: string) => {
    if (!visible.current.has(id)) return;
    clearTimeout(timers.current.get(id));
    setToasts(previous => previous.map(toast => toast.id === id ? { id, message, kind, detail } : toast));
    timers.current.set(id, setTimeout(() => dismiss(id), 8000));
  }, [dismiss]);
  const notify = useMemo<Notify>(() => Object.assign((message: string, kind: ToastKind = "success") => {
    if (!message) return;
    const id = ++nextId.current;
    visible.current.add(id);
    setToasts(previous => [...previous, { id, message, kind }]);
    timers.current.set(id, setTimeout(() => dismiss(id), 8000));
  }, {
    loading: (message: string): ToastOperation => {
      const id = ++nextId.current;
      visible.current.add(id);
      setToasts(previous => [...previous, { id, message, kind: "loading" }]);
      let detail: string | undefined;
      return {
        finish: (message, kind) => finish(id, message, kind, detail),
        dismiss: () => dismiss(id),
        recordError: message => { detail = message; },
      };
    },
  }), [dismiss, finish]);
  useEffect(() => () => {
    timers.current.forEach(clearTimeout);
    timers.current.clear();
    visible.current.clear();
  }, []);
  return <ToastContext.Provider value={notify}>
    {children}
    <div className="toast-stack" aria-label={language === "en" ? "Notifications" : "通知"}>
      {toasts.map(toast => <div key={toast.id} data-toast-id={toast.id} className={`toast toast-${toast.kind}`}>
        <span className="toast-icon" aria-hidden="true">{toast.kind === "loading" ? <span className="toast-spinner" /> : toast.kind === "success" ? "✓" : toast.kind === "error" ? "!" : "i"}</span>
        <div className="toast-content" role={toast.kind === "error" || toast.detail ? "alert" : "status"}>
          <span>{toast.message}</span>
          {toast.detail && toast.detail !== toast.message && <small className="toast-detail">{toast.detail}</small>}
        </div>
        {toast.kind !== "loading" && <button type="button" className="toast-close" aria-label={`${language === "en" ? "Dismiss" : language === "zhtw" ? "關閉" : "关闭"} ${toast.message}`} onClick={() => dismiss(toast.id)}>×</button>}
      </div>)}
    </div>
  </ToastContext.Provider>;
}

/** Empty strings leave existing notifications unchanged. */
export function useToast() { return useContext(ToastContext); }

/** Bridge result reports to the shared notification layer, once per message. */
export function ToastMessage({ message, kind = "info", operation }: { message: string; kind?: ToastKind; operation?: ToastOperation }) {
  const notify = useToast();
  useEffect(() => {
    if (operation) operation.finish(message, kind);
    else notify(message, kind);
  }, [message, kind, notify, operation]);
  return null;
}
