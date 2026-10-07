import { useEffect, useId, useRef, type ReactNode } from "react";

export function ConfirmDialog({
  title,
  children,
  confirmLabel,
  cancelLabel,
  confirmDisabled = false,
  onConfirm,
  onCancel,
}: {
  title: string;
  children: ReactNode;
  confirmLabel: string;
  cancelLabel: string;
  confirmDisabled?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const titleId = useId();
  const descriptionId = useId();
  const cancel = useRef<HTMLButtonElement>(null);
  const cancelAction = useRef(onCancel);
  cancelAction.current = onCancel;

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    cancel.current?.focus();
    function keydown(event: KeyboardEvent) {
      if (event.key === "Escape") cancelAction.current();
    }
    document.addEventListener("keydown", keydown);
    return () => {
      document.removeEventListener("keydown", keydown);
      previous?.focus();
    };
  }, []);

  return (
    <div
      className="modal-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
    >
      <section
        className="modal-dialog modal-dialog-sm"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
      >
        <div className="modal-header">
          <h2 id={titleId}>{title}</h2>
        </div>
        <div className="modal-body">
          <p id={descriptionId}>{children}</p>
          <div className="form-actions">
            <button type="button" className="primary" disabled={confirmDisabled} onClick={onConfirm}>
              {confirmLabel}
            </button>
            <button ref={cancel} type="button" onClick={onCancel}>
              {cancelLabel}
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
