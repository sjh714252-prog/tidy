import { Fragment, useCallback, useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { Decision, MoveLogEntry, NasBatch, NasPendingItem } from "../ipc/types";
import { formatTime } from "./format";
import { useNas } from "../store/nas";

const BY: Record<string, string> = { rule: "규칙", ai: "AI", user: "확인", restructure: "지금 정리" };

/** Shown only once a NAS is connected (설정 › NAS). */
function useHasNas(): boolean {
  const { list, loaded, load } = useNas();
  useEffect(() => {
    if (!loaded) void load();
  }, [loaded, load]);
  return list.length > 0;
}

/**
 * (M18) 확인 대기 › NAS 확인 대기: files a NAS rule asked about. Answers go
 * to the NAS, which moves the file there (and logs it for undo there).
 */
export function NasPendingSection() {
  const has = useHasNas();
  const [items, setItems] = useState<NasPendingItem[]>([]);
  const [errors, setErrors] = useState<string[]>([]);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const key = (p: NasPendingItem) => `${p.nas}:${p.id}`;

  const refresh = useCallback(async () => {
    try {
      const r = await ipc.nasPending();
      setItems(r.items);
      setErrors(r.errors);
      setChecked((c) => new Set([...c].filter((k) => r.items.some((p) => key(p) === k))));
    } catch (e) {
      setErrors([String(e)]);
    }
  }, []);
  useEffect(() => {
    if (has) void refresh();
  }, [has, refresh]);
  if (!has) return null;

  const answer = async (list: NasPendingItem[], decision: Decision) => {
    if (!list.length) return;
    setBusy(true);
    setMsg(null);
    try {
      const byNas = new Map<string, number[]>();
      for (const p of list) byNas.set(p.nas, [...(byNas.get(p.nas) ?? []), p.id]);
      let moved = 0;
      let other = 0;
      const notes: string[] = [];
      for (const [name, ids] of byNas) {
        const res = await ipc.nasResolve(name, ids, decision);
        for (const r of res) {
          if (r.kind === "moved") moved++;
          else other++;
          if (r.message) notes.push(r.message);
        }
      }
      setMsg(
        decision.kind === "move"
          ? `NAS에서 ${moved}개 옮김${other ? ` · ${other}개는 그대로` : ""}${notes.length ? ` — ${notes.slice(0, 2).join("; ")}` : ""}`
          : decision.kind === "skip"
            ? `${list.length}개 건너뜀 (NAS)`
            : `${list.length}개 무시 — 앞으로 묻지 않음 (NAS)`,
      );
      await refresh();
      void useNas.getState().load();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
    }
  };

  const sel = items.filter((p) => checked.has(key(p)));
  const toggle = (k: string) =>
    setChecked((c) => {
      const n = new Set(c);
      if (n.has(k)) n.delete(k);
      else n.add(k);
      return n;
    });

  return (
    <section className="card nas-pending" data-testid="nas-pending">
      <div className="card-head">
        <h2>NAS 확인 대기</h2>
        <span className="sub">NAS 규칙이 옮기기 전에 묻는 파일 — 답하면 NAS가 직접 옮깁니다</span>
        <span className="spacer" />
        <button className="link" onClick={() => void refresh()}>
          새로고침
        </button>
      </div>
      {errors.map((e) => (
        <p key={e} className="warn small">
          {e}
        </p>
      ))}
      {msg && (
        <div className="notice" data-testid="nas-pending-result">
          {msg}
        </div>
      )}
      {!items.length ? (
        <p className="muted small">NAS에서 확인을 기다리는 파일이 없습니다.</p>
      ) : (
        <>
          <div className="toolbar">
            <label className="check">
              <input
                type="checkbox"
                aria-label="NAS 모두 선택"
                checked={sel.length === items.length}
                onChange={() => setChecked(sel.length === items.length ? new Set() : new Set(items.map(key)))}
              />{" "}
              모두
            </label>
            <span className="muted small">선택 {sel.length}</span>
            <div className="spacer" />
            <button
              className="primary"
              disabled={busy || !sel.length}
              onClick={() => void answer(sel, { kind: "move", destDir: null })}
            >
              선택한 항목 제안 위치로 이동
            </button>
            <button disabled={busy || !sel.length} onClick={() => void answer(sel, { kind: "skip" })}>
              건너뜀
            </button>
            <button disabled={busy || !sel.length} onClick={() => void answer(sel, { kind: "ignore" })}>
              무시
            </button>
          </div>
          <div className="list">
            {items.map((p) => (
              <div key={key(p)} className="list-row nas-pending-row" data-testid="nas-pending-row" data-name={p.name}>
                <input type="checkbox" aria-label={p.name} checked={checked.has(key(p))} onChange={() => toggle(key(p))} />
                <div className="cell">
                  <span className="name">
                    {p.name} <span className="tag nas">NAS · {p.nas}</span>
                  </span>
                  <span className="muted small mono ellipsis" title={p.localPath ?? p.path}>
                    {p.dir}
                  </span>
                </div>
                <div className="cell">
                  <span className="small ellipsis" title={p.suggestedDest ?? ""}>
                    {p.suggestedDest ? `→ ${p.suggestedDest}` : "제안 위치 없음"}
                  </span>
                  <span className="muted small">
                    {p.rule ? `규칙: ${p.rule}` : p.source === "ai" ? "AI 제안" : ""}
                    {p.duplicateOf && " · 똑같은 파일이 이미 있음"}
                  </span>
                </div>
                <span className="cell right">
                  <button
                    disabled={busy || !p.suggestedDest}
                    data-testid="nas-pending-move"
                    onClick={() => void answer([p], { kind: "move", destDir: null })}
                  >
                    이동
                  </button>
                  <button className="link" disabled={busy} onClick={() => void answer([p], { kind: "skip" })}>
                    건너뜀
                  </button>
                </span>
              </div>
            ))}
          </div>
        </>
      )}
    </section>
  );
}

/** (M18) 되돌리기 › NAS에서 한 일: the NAS's own move batches, undone there. */
export function NasHistorySection() {
  const has = useHasNas();
  const [batches, setBatches] = useState<NasBatch[]>([]);
  const [errors, setErrors] = useState<string[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const [moves, setMoves] = useState<MoveLogEntry[]>([]);
  const [msg, setMsg] = useState<string | null>(null);
  const key = (b: NasBatch) => `${b.nas}:${b.batch_id}`;

  const refresh = useCallback(async () => {
    try {
      const r = await ipc.nasBatches(50);
      setBatches(r.batches);
      setErrors(r.errors);
    } catch (e) {
      setErrors([String(e)]);
    }
  }, []);
  useEffect(() => {
    if (has) void refresh();
  }, [has, refresh]);
  if (!has) return null;

  const show = async (b: NasBatch) => {
    if (open === key(b)) return setOpen(null);
    setOpen(key(b));
    setMoves([]);
    try {
      setMoves(await ipc.nasBatchMoves(b.nas, b.batch_id));
    } catch (e) {
      setMsg(String(e));
    }
  };
  const undo = async (b: NasBatch) => {
    if (!window.confirm(`NAS(${b.nas})에서 ${BY[b.decided_by] ?? b.decided_by} 이동 ${b.moves - b.undone}개를 원래 자리로 되돌릴까요?`)) return;
    try {
      const r = await ipc.nasUndo(b.nas, b.batch_id);
      setMsg(`NAS에서 ${r.restored.length}개 되돌림` + (r.conflicts.length ? ` · 충돌 ${r.conflicts.length}개: ${r.conflicts.slice(0, 3).join("; ")}` : ""));
      await refresh();
      if (open === key(b)) setMoves(await ipc.nasBatchMoves(b.nas, b.batch_id));
    } catch (e) {
      setMsg(String(e));
    }
  };

  return (
    <section className="card nas-history" data-testid="nas-history">
      <div className="card-head">
        <h2>NAS에서 한 일</h2>
        <span className="sub">NAS가 직접 옮긴 묶음 — 되돌리기도 NAS에서 합니다</span>
        <span className="spacer" />
        <button className="link" onClick={() => void refresh()}>
          새로고침
        </button>
      </div>
      {errors.map((e) => (
        <p key={e} className="warn small">
          {e}
        </p>
      ))}
      {msg && (
        <div className="notice" data-testid="nas-history-msg">
          {msg}
        </div>
      )}
      {!batches.length ? (
        <p className="muted small">NAS 이동 기록이 아직 없습니다.</p>
      ) : (
        <div className="list">
          {batches.map((b) => (
            <Fragment key={key(b)}>
              <div className={`list-row nas-batch-row${b.undone === b.moves ? " muted" : ""}`} data-testid="nas-batch-row">
                <span className="muted small">{formatTime(b.last_at)}</span>
                <span className="small">
                  <span className="tag nas">NAS · {b.nas}</span> {BY[b.decided_by] ?? b.decided_by} {b.moves}개
                  {b.undone > 0 && <span className="muted"> ({b.undone === b.moves ? "되돌림" : `${b.undone}개 되돌림`})</span>}
                </span>
                <span className="mono small ellipsis" title={b.example}>
                  {b.label ?? b.example}
                </span>
                <span className="cell right">
                  <button className="link" onClick={() => void show(b)}>
                    {open === key(b) ? "접기" : "파일 보기"}
                  </button>
                  {b.undone < b.moves && (
                    <button data-testid="nas-undo" onClick={() => void undo(b)}>
                      되돌리기
                    </button>
                  )}
                </span>
              </div>
              {open === key(b) && (
                <ul className="history-files">
                  {moves.map((m) => (
                    <li key={m.id} className={m.undone_at ? "muted" : ""}>
                      <span className="mono">{m.src_path}</span> → <span className="mono">{m.dst_path}</span>
                      {m.undone_at ? " (되돌림)" : ""}
                    </li>
                  ))}
                </ul>
              )}
            </Fragment>
          ))}
        </div>
      )}
    </section>
  );
}
