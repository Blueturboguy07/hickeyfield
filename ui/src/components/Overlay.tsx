import { useEffect, useRef, type ReactNode } from "react";
import { CloseIcon } from "./Icons";

/**
 * Modal shell for the preset picker and the model sheet.
 *
 * Focus is moved into the dialog on open and returned to the invoking element
 * on close — without that, dismissing a full-screen picker drops keyboard
 * focus onto <body> and the next Tab starts from the top of the app.
 */
export function Overlay({
  open,
  onClose,
  title,
  variant = "full",
  children,
  header,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  variant?: "full" | "sheet";
  children: ReactNode;
  header?: ReactNode;
}) {
  const panelRef = useRef<HTMLDivElement>(null);
  const restoreRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;
    restoreRef.current = document.activeElement as HTMLElement | null;
    panelRef.current?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      restoreRef.current?.focus?.();
    };
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div className="overlay-root" data-variant={variant}>
      <button
        type="button"
        className="overlay-backdrop"
        aria-label={`Close ${title}`}
        onClick={onClose}
      />
      <div
        ref={panelRef}
        className="overlay-panel"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
      >
        <header className="overlay-header">
          <h2 className="overlay-title">{title.toUpperCase()}</h2>
          {header}
          <button
            type="button"
            className="btn btn-icon btn-ghost overlay-close"
            onClick={onClose}
            aria-label={`Close ${title}`}
          >
            <CloseIcon size={18} />
          </button>
        </header>
        <div className="overlay-body">{children}</div>
      </div>
    </div>
  );
}
