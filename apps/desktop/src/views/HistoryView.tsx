import { Fragment, useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { BatchSummary, MoveLogEntry } from "../ipc/types";
import { formatTime } from "../components/format";

const BY: Record<string, string> = { rule: "규칙", ai: "AI", user: "확인", restructure: "재구성" };

/** Move history (SPEC §9.6): one row per batch, undo per batch, expand for files. */
export function HistoryView() {
  const [batches, setBatches] = useState<BatchSummary[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const [moves, setMoves] = useState<MoveLogEntry[]>([]);
  const [msg, setMsg] = useState<string | null>(null);

  const refresh = () => void ipc.listBatches(200).then(setBatches).catch((e) => setMsg(String(e)));
  useEffect(refresh, []);
  useEffect(() => {
    if (open) void ipc.batchMoves(open).then(setMoves);
  }, [open]);

  const undo = async (b: BatchSummary) => {
    if (!window.confirm(`${BY[b.decided_by] ?? b.decided_by} 이동 ${b.moves - b.undone}개를 원래 자리로 되돌릴까요?`)) return;
    try {
      const r = await ipc.undoBatch(b.batch_id);
      setMsg(
        `${r.restored.length}개 되돌림` + (r.conflicts.length ? ` · 충돌 ${r.conflicts.length}개: ${r.conflicts.join("; ")}` : ""),
      );
      refresh();
      if (open === b.batch_id) setMoves(await ipc.batchMoves(b.batch_id));
    } catch (e) {
      setMsg(String(e));
    }
  };

  return (
    <div className="history">
      <div className="toolbar">
        <span>이동 기록 (보관 기간: 설정의 undo_retention_days)</span>
        <div className="spacer" />
        <button className="link" onClick={refresh}>
          새로고침
        </button>
      </div>
      {msg && <div className="notice">{msg}</div>}
      {batches.length === 0 ? (
        <p className="muted pad">아직 이동 기록이 없습니다.</p>
      ) : (
        <table className="grid">
          <tbody>
            {batches.map((b) => (
              <Fragment key={b.batch_id}>
                <tr className={b.undone === b.moves ? "gone" : ""}>
                  <td className="nowrap muted">{formatTime(b.last_at)}</td>
                  <td className="nowrap">
                    <span className="tag">{BY[b.decided_by] ?? b.decided_by}</span>
                  </td>
                  <td className="nowrap">
                    {b.moves}개{b.undone > 0 && <span className="muted"> ({b.undone === b.moves ? "되돌림" : `${b.undone}개 되돌림`})</span>}
                  </td>
                  <td className="path" title={b.example}>
                    {b.label && <b>{b.label} </b>}
                    <span className="muted">{b.example}</span>
                  </td>
                  <td className="nowrap">
                    <button className="link" onClick={() => setOpen(open === b.batch_id ? null : b.batch_id)}>
                      {open === b.batch_id ? "접기" : "파일 보기"}
                    </button>
                    {b.undone < b.moves && <button onClick={() => void undo(b)}>되돌리기</button>}
                  </td>
                </tr>
                {open === b.batch_id && (
                  <tr>
                    <td colSpan={5}>
                      <ul className="small">
                        {moves.map((m) => (
                          <li key={m.id} className={m.undone_at ? "muted" : ""}>
                            {m.src_path} → {m.dst_path}
                            {m.undone_at ? " (되돌림)" : m.in_progress ? " (진행 중 / 중단됨)" : ""}
                          </li>
                        ))}
                      </ul>
                    </td>
                  </tr>
                )}
              </Fragment>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
