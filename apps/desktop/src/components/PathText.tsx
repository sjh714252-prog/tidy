import { useLayoutEffect, useRef, useState } from "react";
import * as ipc from "../ipc/commands";
import { aliasPath, pathForms, shortenTo } from "./paths";

let canvas: HTMLCanvasElement | null = null;
function textWidth(text: string, font: string): number {
  canvas ??= document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) return text.length * 8;
  ctx.font = font;
  return ctx.measureText(text).width;
}

/**
 * One way to show a path everywhere (SPEC §14.8 0.1): monospace, middle
 * ellipsis that keeps the drive + first part and the last 1–2 parts, the
 * full path in the title, a click opens it in the file manager. Inside a
 * watch folder (`alias` + `root`) it reads "바탕화면 › 중앙선관위 로그".
 * `max` = a character budget (summary spots); without it the text fits the
 * element's width.
 */
export function PathText({
  path,
  alias,
  root,
  max,
  file = false,
  clickable = true,
  inline = false,
  prefix = "",
  className = "",
}: {
  path: string;
  alias?: string | null;
  root?: string | null;
  max?: number;
  /** A file: the click selects it in its folder. */
  file?: boolean;
  /** false for proposed destinations that may not exist yet, or inside buttons. */
  clickable?: boolean;
  inline?: boolean;
  /** e.g. "→ " (kept outside the measured path). */
  prefix?: string;
  className?: string;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const viaAlias = alias && root ? aliasPath(path, alias, root) : null;
  const [shown, setShown] = useState(() => viaAlias ?? (max ? shortenTo(path, max) : path));

  useLayoutEffect(() => {
    if (viaAlias) {
      setShown(viaAlias);
      return;
    }
    if (max) {
      setShown(shortenTo(path, max));
      return;
    }
    const el = ref.current;
    if (!el) return;
    const fit = () => {
      const avail = el.clientWidth;
      if (!avail) {
        setShown(path);
        return;
      }
      const font = getComputedStyle(el).font;
      const pre = prefix ? textWidth(prefix, font) : 0;
      const forms = pathForms(path);
      setShown(forms.find((f) => pre + textWidth(f, font) <= avail + 0.5) ?? forms[forms.length - 1] ?? path);
    };
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, [path, max, viaAlias, prefix]);

  const open = (e: React.MouseEvent) => {
    if (!clickable) return;
    e.stopPropagation();
    void (file ? ipc.revealPath(path) : ipc.openFolder(path)).catch(() => {});
  };
  return (
    <span
      ref={ref}
      className={`path-text${inline ? " inline" : ""}${clickable ? " clickable" : ""} ${className}`}
      title={path}
      data-testid="path-text"
      data-path={path}
      role={clickable ? "link" : undefined}
      onClick={open}
    >
      {prefix}
      {shown}
    </span>
  );
}
