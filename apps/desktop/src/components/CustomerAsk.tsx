import { useState } from "react";
import * as ipc from "../ipc/commands";
import type { PendingItem } from "../ipc/types";
import type { ItemActions } from "./PendingItems";

const SHOWN = 10;

/**
 * (0.18.0, SPEC §14.24) "어느 고객사 로그인가요?" on a queue item / the
 * popup: the customer folders as buttons (known customers first), a box to
 * search them or type a new one, "그대로 두기", and "next time don't ask
 * for logs named like this" (the log's name becomes the customer's alias).
 */
export function CustomerAsk({ p, a }: { p: PendingItem; a: ItemActions }) {
  const ask = p.askCustomer;
  const [q, setQ] = useState("");
  const [remember, setRemember] = useState(true);
  const [all, setAll] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  if (!ask) return null;
  const key = (s: string) => s.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
  const k = key(q);
  const hits = ask.choices.filter((c) => !k || key(c.name).includes(k));
  const shown = all || k ? hits : hits.slice(0, SHOWN);
  const exact = hits.some((c) => key(c.name) === k);

  const pick = async (name: string) => {
    setErr(null);
    try {
      const dest = await ipc.answerCustomer(p.id, name, remember && !!ask.prefix);
      await a.resolve(p.id, { kind: "move", destDir: dest });
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <div className="customer-ask" data-testid="customer-ask">
      <div className="ca-q">
        <strong>어느 고객사 로그인가요?</strong>{" "}
        <span className="muted small">고르면 그 고객사 폴더의 로그/{ask.what}로 옮깁니다</span>
      </div>
      <input
        className="ca-search"
        data-testid="customer-search"
        placeholder={`고객사 찾기 또는 새 이름 (${ask.choices.length}곳)`}
        value={q}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && q.trim()) {
            e.preventDefault();
            void pick(hits.length === 1 && hits[0] ? hits[0].name : q.trim());
          }
        }}
      />
      <div className="ca-list">
        {shown.map((c) => (
          <button
            key={c.dir}
            className="sm"
            data-testid="customer-choice"
            disabled={a.busy}
            title={c.exists ? c.dir : `${c.dir} (새로 만듦)`}
            onClick={() => void pick(c.name)}
          >
            {c.name}
            {!c.exists && <span className="muted"> · 새 폴더</span>}
          </button>
        ))}
        {!all && !k && hits.length > SHOWN && (
          <button className="link small" onClick={() => setAll(true)}>
            {hits.length - SHOWN}곳 더 보기
          </button>
        )}
        {k && !exact && (
          <button className="sm" data-testid="customer-new" disabled={a.busy} onClick={() => void pick(q.trim())}>
            ‘{q.trim()}’ 새 고객사 폴더로
          </button>
        )}
        {k && hits.length === 0 && <span className="muted small">맞는 고객사 폴더가 없습니다</span>}
      </div>
      <div className="ca-foot">
        {ask.prefix && (
          <label className="small">
            <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} /> 다음부터 이름이 ‘
            {ask.prefix}’인 로그는 묻지 않고 이 고객사로
          </label>
        )}
        <span className="spacer" />
        <button className="sm" data-testid="customer-skip" disabled={a.busy} onClick={() => void a.resolve(p.id, { kind: "skip" })}>
          그대로 두기
        </button>
      </div>
      {err && <div className="error small">{err}</div>}
    </div>
  );
}
