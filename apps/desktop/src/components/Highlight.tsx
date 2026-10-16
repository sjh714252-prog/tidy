import { Fragment } from "react";
import { MARK_END, MARK_START } from "../ipc/types";

/** Render backend highlight markers as <mark> — no innerHTML involved. */
export function Highlight({ text }: { text: string }) {
  const parts: { t: string; on: boolean }[] = [];
  let on = false;
  let buf = "";
  for (const ch of text) {
    if (ch === MARK_START || ch === MARK_END) {
      if (buf) parts.push({ t: buf, on });
      buf = "";
      on = ch === MARK_START;
    } else {
      buf += ch;
    }
  }
  if (buf) parts.push({ t: buf, on });
  return (
    <>
      {parts.map((p, i) => (p.on ? <mark key={i}>{p.t}</mark> : <Fragment key={i}>{p.t}</Fragment>))}
    </>
  );
}
