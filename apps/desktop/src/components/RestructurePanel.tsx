import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { StoredPlan } from "../ipc/types";
import { guardTarget, useReport } from "../store/report";
import { useStatus } from "../store/status";
import { useNav } from "../store/nav";
import { formatTime } from "./format";
import { AiNeeded, useAiOn } from "./AiNeeded";
import { ContextTable, Suggestions } from "./ContextPlan";
import { PlanMoves } from "./PlanMoves";

const SOURCE: Record<string, string> = { ai: "AI", rules: "규칙", context: "업무 맥락", relocate: "정리 위치로 옮기기", imported: "제안 파일" };

/**
 * Restructure (SPEC §5.7): plan from rules (no AI) or from the AI →
 * before/after diff + move checkboxes → apply the ticked ones as ONE batch
 * (undo in 되돌리기). Never applied without the user's click + confirm.
 */
export function RestructurePanel() {
  const r = useReport();
  const aiState = useStatus((s) => s.status?.aiState);
  const aiOn = useAiOn();
  const recursive = r.recursive;
  const setRecursive = r.setRecursive;
  const [plans, setPlans] = useState<StoredPlan[]>([]);
  useEffect(() => {
    void ipc.listPlans().then(setPlans).catch(() => {});
  }, [r.plan?.id, r.applied]);

  const plan = r.plan;
  const rel = (p: string) => (plan && p.startsWith(plan.root) ? p.slice(plan.root.length + 1) || "." : p);
  const plan_ = (kind: "rules" | "ai") => {
    if (guardTarget(r.target)) void r.makePlan(kind, recursive);
  };
  const confirmApply = () => {
    if (!plan) return;
    const n = r.selected.size;
    if (window.confirm(`선택한 ${n}개 항목을 옮깁니다. 되돌리기 탭에서 한 번에 되돌릴 수 있습니다. 진행할까요?`))
      void r.apply();
  };

  return (
    <section className="card restructure">
      <div className="card-head">
        <h2>정리 제안</h2>
        <span className="sub">미리보기를 보고 고른 것만 옮깁니다</span>
      </div>
      <div className="row">
        <button disabled={!r.path || !!r.planning} onClick={() => plan_("rules")}>
          {r.planning === "rules" ? "계산 중…" : "규칙으로 정리 미리보기"}
        </button>
        <label className="small" title="규칙 정리에만 적용됩니다. 업무 맥락 정리는 하위 폴더를 통째로 한 항목으로 봅니다.">
          <input type="checkbox" checked={recursive} onChange={(e) => setRecursive(e.target.checked)} /> 하위 폴더 포함
        </label>
        <button
          disabled={!r.path || !!r.planning || aiState !== "ready"}
          title={aiState === "ready" ? "" : "설정 → AI 연결에서 AI를 켜고 연결 테스트를 통과해야 합니다"}
          onClick={() => plan_("ai")}
        >
          {r.planning === "ai" ? "AI가 구조를 보는 중… (최대 몇 분)" : "AI 정리 제안"}
        </button>
        <button
          data-testid="curate-plan"
          className={r.suggestCurate && aiOn ? "primary" : undefined}
          disabled={!r.path || !!r.planning || !aiOn}
          title={
            aiOn
              ? "파일 이름·경로·날짜·본문 앞부분으로 고객사/날짜_이슈/종류 폴더를 제안합니다. 하위 폴더는 통째로 한 항목입니다(분석 전 비용 안내)"
              : "AI를 켜면 쓸 수 있습니다"
          }
          onClick={() => void r.curate()}
        >
          {r.planning === "context" ? "AI가 업무 맥락을 보는 중…" : "업무 맥락으로 정리"}
        </button>
        <AiNeeded />
        <button
          data-testid="import-plan"
          disabled={!!r.planning}
          title="다른 곳에서 만든 정리 제안(.json)을 엽니다. 미리보기 후 적용해야 옮깁니다"
          onClick={() => void r.importPlan()}
        >
          제안 파일 열기…
        </button>
        {plans.length > 0 && (
          <select value="" onChange={(e) => e.target.value && void r.openPlan(Number(e.target.value))}>
            <option value="">지난 제안 열기…</option>
            {plans.map((p) => (
              <option key={p.id} value={p.id}>
                #{p.id} {SOURCE[p.source] ?? p.source} {formatTime(p.created_at)} {p.applied_batch ? "(적용됨)" : ""}
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

      {!r.planning && plan?.source === "rules" && !plan.applied_batch && <Leftover moves={plan.moves.length} />}
      {plan && (
        <div className="plan">
          <p className={`notice small no-move${plan.applied_batch ? "" : " warn"}`} data-testid="no-move-notice">
            {plan.applied_batch ? (
              "이 제안은 적용되었습니다. 되돌리기 탭에서 되돌릴 수 있습니다."
            ) : (
              <>
                <b>미리보기입니다.</b> 적용을 누르기 전에는 아무것도 옮기지 않습니다. 아래 [선택한 N개 적용]을 눌러야
                정리 위치에 폴더가 만들어지고 파일이 옮겨집니다. 적용 후에도 되돌리기 탭에서 되돌릴 수 있습니다.
              </>
            )}
          </p>
          <div className="row">
            <strong>
              제안 #{plan.id} · {SOURCE[plan.source] ?? plan.source} · {plan.moves.length}개 항목
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
          ) : plan.source === "context" ? (
            <ContextTable plan={plan} rel={rel} />
          ) : (
            <PlanMoves plan={plan} rel={rel} />
          )}
          {r.suggestions.length > 0 && <Suggestions />}
          {r.learnt && <p className="ok small">{r.learnt}</p>}
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
                  <h4>{plan.applied_batch ? "적용 후" : "적용하면 (미리보기)"} ({r.preview.files_moved}개 파일 이동)</h4>
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
            <div className="row apply-bar" data-testid="apply-bar">
              <button className="primary" disabled={r.applying || !r.selected.size} onClick={confirmApply}>
                {r.applying ? "옮기는 중…" : `선택한 ${r.selected.size}개 적용`}
              </button>
              <span className="muted small">적용 전에는 아무것도 옮겨지지 않습니다.</span>
              <div className="spacer" />
              <span className="small">{r.preview ? `${r.preview.files_moved}개 파일 이동 예정` : ""}</span>
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
                이 정리 되돌리기
              </button>
              {r.applied.skipped.length > 0 && <div className="small">건너뜀 {r.applied.skipped.length}개</div>}
              {(r.applied.duplicates ?? []).map((d) => (
                <div key={d.path} className="small" data-testid="applied-duplicate">
                  {d.path.split(/[\\/]/).pop()}: {d.message}{" "}
                  <button className="link small" onClick={() => void ipc.revealPath(d.existing)}>
                    기존 파일 열기
                  </button>
                </div>
              ))}
              {r.applied.errors.length > 0 && <div className="small error">{r.applied.errors.join("; ")}</div>}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

/**
 * (0.11.1) What a rules preview leaves where it is: files no rule takes and
 * sub-folders (only "업무 맥락으로 정리" moves those). Without this a
 * dictionary-only setup saw "옮길 것이 없습니다" and nothing ever moved.
 */
function Leftover({ moves }: { moves: number }) {
  const r = useReport();
  const aiOn = useAiOn();
  const goSettings = useNav((s) => s.goSettings);
  const left = r.leftover;
  const files = left?.files ?? 0;
  const folders = left?.folders ?? 0;
  if (!files && !folders && !(moves === 0 && aiOn)) return null;
  const what = [files ? `파일 ${files.toLocaleString()}개` : "", folders ? `폴더 ${folders.toLocaleString()}개` : ""]
    .filter(Boolean)
    .join("와 ");
  return (
    <div className="notice warn" data-testid="curate-hint">
      <div>
        {moves === 0 ? "규칙으로 옮길 파일이 없습니다." : `규칙에 맞는 ${moves}개만 옮깁니다.`}{" "}
        {what && `규칙에 안 맞는 ${what}는 그대로 남습니다.`}
      </div>
      {aiOn ? (
        <div className="row" style={{ marginTop: 6 }}>
          <button className="primary sm" data-testid="curate-rest" onClick={() => void r.curate()}>
            업무 맥락으로 {moves ? "나머지도 " : ""}정리 미리보기
          </button>
          <span className="muted small">고객사·이슈별로 묶습니다(분석 전 비용 안내, 적용 전엔 무이동).</span>
        </div>
      ) : (
        <div className="row" style={{ marginTop: 6 }}>
          <span className="small">남은 파일을 정리하려면 규칙을 추가하거나 AI를 켜세요.</span>
          <button className="link" onClick={() => goSettings("rules")}>
            규칙 만들기
          </button>
          <button className="link" onClick={() => goSettings("ai")}>
            AI 켜기
          </button>
        </div>
      )}
    </div>
  );
}
