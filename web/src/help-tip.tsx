import { useContext, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { LanguageContext, t } from "./i18n";

/** Shared help for mouse, keyboard and touch; stays inside the viewport. */
export function HelpTip({ children, id, label }: { children: ReactNode; id?: string; label?: string }) {
  const language = useContext(LanguageContext);
  const generated = useId();
  const tooltipId = `${id ?? generated}-tooltip`;
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ left: 16, top: 16 });
  const trigger = useRef<HTMLButtonElement>(null);
  const popup = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    if (!open) return;
    function place() {
      const anchor = trigger.current?.getBoundingClientRect();
      const box = popup.current?.getBoundingClientRect();
      if (!anchor || !box) return;
      setPosition({
        left: Math.max(16, Math.min(anchor.left, window.innerWidth - box.width - 16)),
        top: anchor.bottom + box.height + 12 < window.innerHeight
          ? anchor.bottom + 8 : Math.max(16, anchor.top - box.height - 8),
      });
    }
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => { window.removeEventListener("resize", place); window.removeEventListener("scroll", place, true); };
  }, [open]);
  return <span className="help-tip" id={id} onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)}>
    <button ref={trigger} type="button" className="help-trigger" aria-label={label ?? t(language, "help")} aria-describedby={open ? tooltipId : undefined}
      onFocus={() => setOpen(true)} onBlur={() => setOpen(false)}
      onClick={event => { event.preventDefault(); setOpen(true); }} onKeyDown={event => { if (event.key === "Escape") setOpen(false); }}>?</button>
    {open && createPortal(<span ref={popup} id={tooltipId} role="tooltip" className="help-popup" style={position}>{children}</span>, document.body)}
  </span>;
}
