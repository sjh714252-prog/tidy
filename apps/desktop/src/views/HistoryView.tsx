import { Fragment, useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { BatchSummary, MoveLogEntry } from "../ipc/types";
import { formatTime } from "../components/format";
import { PathText } from "../components/PathText";
import { NasHistorySection } from "../components/NasSections";

const BY: Record<string, string> = { rule: "규칙", ai: "AI", user: "확인", restructure: "정리 제안" };

/** Move history (SPEC §9.6): one row per batch, undo per batch, expand for files. */
export function HistoryView() {
  const [batches, setBatches] = useState<BatchSummary[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const [moves, setMoves] = useState<MoveLogEntry[]>([]);
  const [msg, setMsg] = useState<string | null>(null);
  /** (M14) Undo met files in the trash: offer "휴지통 열기". */
  const [inTrash, setInTrash] = useState(0);

  const refresh = () => void ipc.listBatches(200).then(setBatches).catch((e) => setMsg(String(e)));
  useEffect(refresh, []);
  useEffect(() => {
    if (open) void ipc.batchMoves(open).then(setMoves);
  }, [open]);

  // Bulk undo: everything rule/AI moved in the last N hours (one click after
  // an automatic run went wrong, e.g. a project folder split file by file).
  const [bulkHours, setBulkHours] = useState(1);
  const [bulkCount, setBulkCount] = useState<number | null>(null);
  useEffect(() => {
    void ipc.countMovesSince(bulkHours * 3600, true).then(setBulkCount).catch(() => setBulkCount(null));
  }, [bulkHours, batches]);
  const undoBulk = async () => {
    if (!bulkCount) return;
    if (!window.confirm(`최근 ${bulkHours}시간 동안 규칙·AI가 자동으로 옮긴 ${bulkCount}개를 전부 원래 자리로 되돌릴까요? (직접 확인해서 옮긴 것은 그대로 둡니다)`)) return;
    try {
      const r = await ipc.undoSince(bulkHours * 3600, true);
      setInTrash(0);
      setMsg(`${r.restored.length}개 되돌림` + (r.conflicts.length ? ` · 충돌 ${r.conflicts.length}개: ${r.conflicts.slice(0, 3).join("; ")}` : ""));
      refresh();
    } catch (e) {
      setMsg(String(e));
    }
  };

  const undo = async (b: BatchSummary) => {
    const trashOnly = b.trashed > 0 && b.trashed === b.moves;
    const q = trashOnly
      ? `휴지통으로 보낸 ${b.moves - b.undone}개는 Tidy가 되살릴 수 없습니다. 휴지통에서 복원했는지 확인할까요?`
      : `${BY[b.decided_by] ?? b.decided_by} 이동 ${b.moves - b.undone}개를 원래 자리로 되돌릴까요?`;
    if (!window.confirm(q)) return;
    try {
      const r = await ipc.undoBatch(b.batch_id);
      const other = r.conflicts.length - r.inTrash.length;
      setInTrash(r.inTrash.length);
      setMsg(
        `${r.restored.length}개 되돌림` +
          (r.inTrash.length ? ` · 휴지통에 있는 파일 ${r.inTrash.length}개 — 휴지통에서 복원하세요` : "") +
          (other > 0 ? ` · 충돌 ${other}개: ${r.conflicts.filter((c) => !r.inTrash.some((t) => c.startsWith(t))).join("; ")}` : ""),
      );
      refresh();
      if (open === b.batch_id) setMoves(await ipc.batchMoves(b.batch_id));
    } catch (e) {
      setMsg(String(e));
    }
  };

  return (
    <div className="page history">
      <section className="card" data-testid="bulk-undo">
        <div className="card-head">
          <h2>자동 이동 한꺼번에 되돌리기</h2>
          <span className="sub">규칙·AI가 자동으로 옮긴 것만 — 직접 확인해서 옮긴 것은 건드리지 않습니다</span>
        </div>
        <div className="toolbar">
          <select value={bulkHours} onChange={(e) => setBulkHours(Number(e.target.value))} aria-label="되돌릴 기간">
            <option value={1}>최근 1시간</option>
            <option value={3}>최근 3시간</option>
            <option value={24}>최근 24시간</option>
            <option value={24 * 7}>최근 7일</option>
          </select>
          <button className="primary" disabled={!bulkCount} onClick={() => void undoBulk()}>
            {bulkCount == null ? "…" : `규칙·AI 이동 ${bulkCount}개 되돌리기`}
          </button>
        </div>
      </section>
      <div className="toolbar">
        <h2>이동 기록</h2>
        <span className="muted small">묶음마다 되돌릴 수 있습니다 (보관 기간: 설정의 undo_retention_days)</span>
        <div className="spacer" />
        <button className="link" onClick={refresh}>
          새로고침
        </button>
      </div>
      {msg && (
        <div className="notice" data-testid="history-msg">
          {msg}
          {inTrash > 0 && (
            <button className="link" style={{ marginLeft: 8 }} onClick={() => void ipc.openTrash()}>
              휴지통 열기
            </button>
          )}
        </div>
      )}
      {batches.length === 0 ? (
        <p className="muted">아직 이동 기록이 없습니다.</p>
      ) : (
        <section className="table">
          {batches.map((b) => (
            <Fragment key={b.batch_id}>
              <div className={`t-row history-row${b.undone === b.moves ? " gone" : ""}`} data-testid="batch-row">
                <span className="muted small hide-narrow">{formatTime(b.last_at)}</span>
                <span className="hide-narrow">
                  <span className="tag" style={{ marginLeft: 0 }}>
                    {b.trashed > 0 && b.trashed === b.moves ? "중복 정리(휴지통)" : (BY[b.decided_by] ?? b.decided_by)}
                  </span>
                </span>
                <span className="small hide-narrow">
                  {b.moves}개{b.undone > 0 && <span className="muted"> ({b.undone === b.moves ? "되돌림" : `${b.undone}개 되돌림`})</span>}
                </span>
                <div className="cell">
                  {b.label && <span className="name">{b.label}</span>}
                  <PathText path={b.example} file className="sub-path" />
                </div>
                <span className="cell right">
                  <button className="link" onClick={() => setOpen(open === b.batch_id ? null : b.batch_id)}>
                    {open === b.batch_id ? "접기" : "파일 보기"}
                  </button>
                  {b.undone < b.moves && <button onClick={() => void undo(b)}>되돌리기</button>}
                </span>
              </div>
              {open === b.batch_id && (
                <ul className="history-files">
                  {moves.map((m) =>
                    m.op === "trash" ? (
                      <li key={m.id} className={m.undone_at ? "muted" : ""} data-testid="trash-row">
                        🗑 <span className="mono">{m.src_path}</span> → 휴지통{" "}
                        <span className="muted">(남긴 사본: <span className="mono">{m.dst_path}</span>)</span>
                        {m.undone_at ? " (복원됨)" : ""}
                      </li>
                    ) : (
                      <li key={m.id} className={m.undone_at ? "muted" : ""}>
                        <span className="mono">{m.src_path}</span> → <span className="mono">{m.dst_path}</span>
                        {m.undone_at ? " (되돌림)" : m.in_progress ? " (진행 중 / 중단됨)" : ""}
                      </li>
                    ),
                  )}
                </ul>
              )}
            </Fragment>
          ))}
        </section>
      )}
      <NasHistorySection />
    </div>
  );
}
