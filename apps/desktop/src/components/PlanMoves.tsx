import type { Plan } from "../ipc/types";
import { useReport } from "../store/report";
import { formatSize } from "./format";
import { PathText } from "./PathText";
import { useNarrow } from "./useNarrow";
import { useWatchNames } from "../store/status";
import { watchOf } from "./paths";

/**
 * Rules / AI plan items in the queue's row design (SPEC §14.8 6):
 * ☐ / 지금 (📁 폴더 · 파일 N) / 옮길 곳 + 이유; cards below 900px.
 */
/** Where the item is now: "바탕화면 › 하위 폴더" (its parent folder). */
function Source({ p }: { p: string }) {
  const names = useWatchNames();
  const dir = p.replace(/[\\/][^\\/]*$/, "") || p;
  const w = watchOf(dir, names);
  return <PathText path={dir} alias={w?.alias} root={w?.path} className="sub-path" />;
}

export function PlanMoves({ plan }: { plan: Plan; rel?: (p: string) => string }) {
  const r = useReport();
  const narrow = useNarrow();
  const locked = !!plan.applied_batch;
  const name = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;
  const all = (
    <label className="check">
      <input
        type="checkbox"
        aria-label="모두 선택"
        disabled={locked}
        checked={r.selected.size === plan.moves.length}
        onChange={(e) => r.setAll(e.target.checked)}
      />{" "}
      모두 <span className="muted">· 선택 {r.selected.size} / {plan.moves.length}</span>
    </label>
  );
  if (narrow)
    return (
      <div className="stack" data-testid="plan-cards">
        {all}
        <div className="item-cards">
          {plan.moves.map((m) => (
            <article key={m.id} className="item-card" data-testid="plan-item">
              <div className="ic-top">
                <input
                  type="checkbox"
                  aria-label="선택"
                  disabled={locked}
                  checked={r.selected.has(m.id)}
                  onChange={() => r.toggle(m.id)}
                />
                <div className="ic-main">
                  <span className="name" title={m.from}>
                    {m.is_dir ? "📁 " : ""}
                    {name(m.from)}
                  </span>
                  <span className="sub-path" style={{ display: "flex", gap: 4, minWidth: 0 }}>
                    <Source p={m.from} />
                    {m.is_dir && <span className="nowrap">· 폴더 · 파일 {m.files.toLocaleString()}개 · {formatSize(m.bytes)}</span>}
                  </span>
                </div>
              </div>
              <div className="ic-body">
                <div className="ic-dest">
                  <span className="muted">→ </span>
                  <PathText path={m.to} clickable={false} />
                </div>
                {m.reason && <div className="reason clamp2">{m.reason}</div>}
                {m.warnings.map((w) => (
                  <div key={w} className="warn small">
                    ⚠ {w}
                  </div>
                ))}
              </div>
            </article>
          ))}
        </div>
      </div>
    );
  return (
    <div className="stack">
      {all}
      <section className="table" data-testid="plan-table">
        <div className="t-head cols-plan">
          <span />
          <span>지금</span>
          <span>옮길 곳</span>
        </div>
        {plan.moves.map((m) => (
          <div key={m.id} className="t-row cols-plan" data-testid="plan-item">
            <input
              type="checkbox"
              aria-label="선택"
              disabled={locked}
              checked={r.selected.has(m.id)}
              onChange={() => r.toggle(m.id)}
            />
            <div className="cell">
              <span className="name" title={m.from}>
                {m.is_dir ? "📁 " : ""}
                {name(m.from)}{" "}
                {m.is_dir && (
                  <span className="sub">
                    폴더 · 파일 {m.files.toLocaleString()} · {formatSize(m.bytes)}
                  </span>
                )}
              </span>
              <Source p={m.from} />
            </div>
            <div className="cell">
              <PathText path={m.to} clickable={false} />
              {m.reason && (
                <span className="reason clamp2" title={m.reason}>
                  {m.reason}
                </span>
              )}
              {m.warnings.map((w) => (
                <span key={w} className="warn small">
                  ⚠ {w}
                </span>
              ))}
            </div>
          </div>
        ))}
      </section>
    </div>
  );
}
