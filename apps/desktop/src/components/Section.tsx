import { type ReactNode, useEffect, useRef, useState } from "react";
import { useNav } from "../store/nav";

const KEY = "tidy.settings.sections";
/** ① 감시 폴더 and ② 규칙 start open (SPEC §14.4); the rest folded. */
const DEFAULT_OPEN: Record<string, boolean> = { watch: true, rules: true };

function readState(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? { ...DEFAULT_OPEN, ...JSON.parse(raw) } : { ...DEFAULT_OPEN };
  } catch {
    return { ...DEFAULT_OPEN };
  }
}

function writeState(s: Record<string, boolean>) {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    // Private mode / blocked storage: the state is just not remembered.
  }
}

/**
 * A foldable settings section (SPEC §14.4). Open/closed is remembered per
 * section; "AI 연결에서 켜기" links open and scroll to their section.
 */
export function Section({
  id,
  title,
  badge,
  children,
}: {
  id: string;
  title: string;
  /** Shown next to the title (e.g. the "AI 필요" badge). */
  badge?: ReactNode;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(() => readState()[id] ?? false);
  const focus = useNav((s) => s.focus);
  const clearFocus = useNav((s) => s.clearFocus);
  const ref = useRef<HTMLElement>(null);
  const toggle = (on: boolean) => {
    setOpen(on);
    writeState({ ...readState(), [id]: on });
  };
  useEffect(() => {
    if (focus !== id) return;
    toggle(true);
    clearFocus();
    setTimeout(() => ref.current?.scrollIntoView({ block: "start" }), 50);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focus, id]);
  return (
    <section ref={ref} className={open ? "fold open" : "fold"} data-section={id}>
      <h2>
        <button className="fold-head" aria-expanded={open} onClick={() => toggle(!open)}>
          <span className="chev">{open ? "▾" : "▸"}</span> {title}
        </button>
        {badge}
      </h2>
      {open && <div className="fold-body">{children}</div>}
    </section>
  );
}
