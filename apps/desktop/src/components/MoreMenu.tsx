import { type ReactNode, useEffect, useRef, useState } from "react";

export interface MoreItem {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  title?: string;
  /** A divider above this item. */
  sep?: boolean;
}

/** "⋯" button + menu (SPEC §14.8: one main button, the rest in ⋯). */
export function MoreMenu({
  items,
  label = "더 보기",
  up = false,
  testid,
  children,
}: {
  testid?: string;
  items: MoreItem[];
  /** aria-label of the ⋯ button. */
  label?: string;
  up?: boolean;
  children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);
  return (
    <div className="more" ref={ref}>
      <button className="icon" aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(!open)}>
        {children ?? "⋯"}
      </button>
      {open && (
        <div className={up ? "more-menu up" : "more-menu"} role="menu" data-testid={testid}>
          {items.map((it) => (
            <span key={it.label} style={{ display: "contents" }}>
              {it.sep && <hr />}
              <button
                role="menuitem"
                disabled={it.disabled}
                title={it.title}
                onClick={() => {
                  setOpen(false);
                  it.onClick();
                }}
              >
                {it.label}
              </button>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
