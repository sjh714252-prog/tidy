import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { StoredPlan } from "../ipc/types";
import { useReport } from "../store/report";
import { useStatus } from "../store/status";
import { formatSize, formatTime } from "./format";

/**
 * Restructure (SPEC §5.7): plan from rules (no AI) or from the AI →
 * before/after diff + move checkboxes → apply the ticked ones as ONE batch
 * (undo in 이동 기록). Never applied without the user's click + confirm.
 */
export function RestructurePanel() {
  const r = useReport();
  const aiState = useStatus((s) => s.status?.aiState);
  const [recursive, setRecursive] = useState(false);
  const [plans, setPlans] = useState<StoredPlan[]>([]);
  useEffect(() => {
    void ipc.listPlans().then(setPlans).catch(() => {});
  }, [r.plan?.id, r.applied]);

  const plan = r.plan;
  const rel = (p: string) => (plan && p.startsWith(plan.root) ? p.slice(plan.root.length + 1) || "." : p);
  const confirmApply = () => {
    if (!plan) return;
    const n = r.selected.size;
    if (window.confirm(`선택한 ${n}개 항목을 옮깁니다. 이동 기록에서 한 번에 되돌릴 수 있습니다. 진행할까요?`))
      void r.apply();
  };

  return (
    <section className="restructure pad">
      <h3>정리 제안 (재구성)</h3>
      <div className="row">
        <button disabled={!r.path || !!r.planning} onClick={() => void r.makePlan("rules", recursive)}>
          {r.planning === "rules" ? "계산 중…" : "규칙으로 정리 미리보기"}
        </button>
        <label className="small">
          <input type="checkbox" checked={recursive} onChange={(e) => setRecursive(e.target.checked)} /> 하위 폴더 포함
        </label>
        <button
          disabled={!r.path || !!r.planning || aiState !== "ready"}
          title={aiState === "ready" ? "" : "설정 > AI 분류에서 AI를 켜고 연결 테스트를 통과해야 합니다"}
          onClick={() => void r.makePlan("ai")}
        >
          {r.planning === "ai" ? "AI가 구조를 보는 중… (최대 몇 분)" : "AI 재구성 제안"}
        </button>
        {plans.length > 0 && (
          <select value="" onChange={(e) => e.target.value && void r.openPlan(Number(e.target.value))}>
            <option value="">지난 제안 열기…</option>
            {plans.map((p) => (
              <option key={p.id} value={p.id}>
                #{p.id} {p.source} {formatTime(p.created_at)} {p.applied_batch ? "(적용됨)" : ""}
              </option>
            ))}
          </select>
        )}
      </div>
      <details>
        <summary className="small">건드리지 말 폴더 (한 줄에 하나)</summary>
        <textarea
          className="protect"
          value={r.protect}
          placeholder={"예: C:\\Users\\나\\바탕 화면\\진행 중"}
          onChange={(e) => r.setProtect(e.target.value)}
        />
        <p className="muted small">config.toml의 [report] protected 목록도 항상 적용됩니다.</p>
      </details>

      {plan && (
        <div className="plan">
          <div className="row">
            <strong>
              제안 #{plan.id} · {plan.source === "ai" ? "AI" : "규칙"} · {plan.moves.length}개 항목
            </strong>
            {plan.applied_batch && <span className="tag">적용됨</span>}
            <div className="spacer" />
            <button className="link" onClick={r.closePlan}>
              닫기
            </button>
          </div>
          {plan.summary && <p>{plan.summary}</p>}
          {plan.folders.length > 0 && (
            <ul className="small">
              {plan.folders.map((f) => (
                <li key={f.path}>
                  <b>{f.path}</b> — {f.reason}
                </li>
              ))}
            </ul>
          )}
          {plan.moves.length === 0 ? (
            <p className="muted">옮길 것이 없습니다.</p>
          ) : (
            <table className="grid">
              <thead>
                <tr>
                  <th>
                    <input
                      type="checkbox"
                      checked={r.selected.size === plan.moves.length}
                      onChange={(e) => r.setAll(e.target.checked)}
                    />
                  </th>
                  <th>#</th>
                  <th>지금</th>
                  <th>옮길 곳</th>
                  <th>이유</th>
                </tr>
              </thead>
              <tbody>
                {plan.moves.map((m) => (
                  <tr key={m.id}>
                    <td>
                      <input
                        type="checkbox"
                        disabled={!!plan.applied_batch}
                        checked={r.selected.has(m.id)}
                        onChange={() => r.toggle(m.id)}
                      />
                    </td>
                    <td className="muted">{m.id}</td>
                    <td className="path" title={m.from}>
                      {rel(m.from)}
                      {m.is_dir && <span className="muted small"> 폴더 · {m.files}개 · {formatSize(m.bytes)}</span>}
                    </td>
                    <td className="path" title={m.to}>
                      {rel(m.to)}
                    </td>
                    <td className="small">
                      {m.reason}
                      {m.warnings.map((w) => (
                        <div key={w} className="warn">
                          ⚠ {w}
                        </div>
                      ))}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {plan.rejected.length > 0 && (
            <details>
              <summary className="small warn">받아들이지 않은 AI 제안 {plan.rejected.length}개</summary>
              <ul className="small">
                {plan.rejected.map((x, i) => (
                  <li key={i}>
                    {x.from} → {x.to}: {x.reason}
                  </li>
                ))}
              </ul>
            </details>
          )}

          {r.preview && (
            <>
              <div className="diff">
                <div>
                  <h4>지금</h4>
                  <pre className="tree">{r.preview.before}</pre>
                </div>
                <div>
                  <h4>적용 후 ({r.preview.files_moved}개 파일 이동)</h4>
                  <pre className="tree">{r.preview.after}</pre>
                </div>
              </div>
              {r.preview.outside.length > 0 && (
                <p className="small warn">이 폴더 밖으로 나가는 파일 {r.preview.outside.length}개</p>
              )}
              {r.preview.emptied.length > 0 && (
                <p className="small muted">
                  비게 되는 폴더(삭제하지 않고 남겨 둠): {r.preview.emptied.join(", ")}
                </p>
              )}
            </>
          )}

          {!plan.applied_batch && plan.moves.length > 0 && (
            <div className="row">
              <button className="primary" disabled={r.applying || !r.selected.size} onClick={confirmApply}>
                {r.applying ? "옮기는 중…" : `선택한 ${r.selected.size}개 적용`}
              </button>
              <span className="muted small">적용 전에는 아무것도 옮겨지지 않습니다.</span>
            </div>
          )}
          {r.applied && (
            <div className="notice">
              {r.applied.moved}개 파일을 옮겼습니다 (batch {r.applied.batchId.slice(0, 8)}).{" "}
              <button
                onClick={() =>
                  void ipc.undoBatch(r.applied!.batchId).then((u) => alert(`${u.restored.length}개 되돌림`))
                }
              >
                이 재구성 되돌리기
              </button>
              {r.applied.skipped.length > 0 && <div className="small">건너뜀 {r.applied.skipped.length}개</div>}
              {r.applied.errors.length > 0 && <div className="small error">{r.applied.errors.join("; ")}</div>}
            </div>
          )}
        </div>
      )}
    </section>
  );
}
