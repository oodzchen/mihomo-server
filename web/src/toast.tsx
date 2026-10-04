import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import type { Language } from "./i18n";

type Notify = (message: string, kind?: "success" | "info") => void;
const ToastContext = createContext<Notify>(() => {});

export function ToastProvider({ children, language }: { children: ReactNode; language: Language }) {
  const [toasts, setToasts] = useState<{ id: number; message: string; kind: "success" | "info" }[]>([]);
  const nextId = useRef(0);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());
  const dismiss = useCallback((id: number) => {
    clearTimeout(timers.current.get(id));
    timers.current.delete(id);
    setToasts(previous => previous.filter(toast => toast.id !== id));
  }, []);
  const notify = useCallback<Notify>((message, kind = "success") => {
    if (!message) return;
    const id = ++nextId.current;
    setToasts(previous => [...previous, { id, message, kind }]);
    timers.current.set(id, setTimeout(() => dismiss(id), 8000));
  }, [dismiss]);
  useEffect(() => () => { timers.current.forEach(clearTimeout); timers.current.clear(); }, []);
  return <ToastContext.Provider value={notify}>
    {children}
    <div className="toast-stack" aria-label={language === "en" ? "Notifications" : "通知"}>
      {toasts.map(toast => <div key={toast.id} className={`toast toast-${toast.kind}`}>
        <span className="toast-icon" aria-hidden="true">{toast.kind === "success" ? "✓" : "i"}</span>
        <span role="status">{toast.message}</span>
        <button type="button" className="toast-close" aria-label={`${language === "en" ? "Dismiss" : language === "zhtw" ? "關閉" : "关闭"} ${toast.message}`} onClick={() => dismiss(toast.id)}>×</button>
      </div>)}
    </div>
  </ToastContext.Provider>;
}

/** Empty strings clear old inline notices without removing stacked notifications. */
export function useToast() { return useContext(ToastContext); }

/** Bridge result reports to the shared notification layer, once per message. */
export function ToastMessage({ message, kind = "info" }: { message: string; kind?: "success" | "info" }) {
  const notify = useToast();
  useEffect(() => { notify(message, kind); }, [message, kind, notify]);
  return null;
}
