import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { PlannedMove, StoredPlan, StructDir } from "../ipc/types";
import { ALL_WATCHES, guardTarget, useReport } from "../store/report";
import { useCurate } from "../store/curate";
import { useStatus } from "../store/status";
import { useNav } from "../store/nav";
import { formatTime } from "./format";
import { AiNeeded, useAiOn } from "./AiNeeded";
import { ContextTable, Suggestions } from "./ContextPlan";
import { PlanMoves } from "./PlanMoves";

const SOURCE: Record<string, string> = { ai: "AI", rules: "규칙", context: "업무 맥락", relocate: "정리 위치로 옮기기", imported: "제안 파일", compact: "정리 위치 다듬기", review: "정리 위치 점검" };

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
  const all = r.path === ALL_WATCHES;
  const reviewMode = r.reviewRoots.includes(r.path);
  const rel = (p: string) => {
    if (!plan) return p;
    if (p.startsWith(plan.root)) return p.slice(plan.root.length + 1) || ".";
    // (0.12.0) A "모든 감시 폴더" plan: sources by their folder name.
    const src = (plan.sources ?? []).find((s) => p.startsWith(s));
    if (src) return `${src.split(/[\\/]/).pop()}/${p.slice(src.length + 1)}`;
    return p;
  };
  const plan_ = (kind: "rules" | "ai") => {
    if (guardTarget(r.target)) void r.makePlan(kind, recursive);
  };
  const confirmApply = () => {
    if (!plan) return;
    const n = r.selected.size;
    if (window.confirm(`선택한 ${n}개 항목을 옮깁니다. 되돌리기 탭에서 한 번에 되돌릴 수 있습니다. 진행할까요?`))
      void r.apply();
  };
  const step = !plan ? 1 : plan.applied_batch ? 3 : 2;
  const unsure = plan?.source === "context" ? plan.moves.filter((m) => m.context?.unclassified) : [];
  const movable = plan ? plan.moves.filter((m) => m.from !== m.to).length : 0;

  return (
    <section className="card restructure">
      <div className="card-head">
        <h2>정리하기</h2>
        <span className="sub">미리보기를 보고 고른 것만 옮깁니다</span>
      </div>
      <ol className="steps" data-testid="organize-steps" aria-label="정리 순서">
        {["미리보기 만들기", "확인하고 고르기", "옮기기"].map((t, i) => (
          <li key={t} className={i + 1 === step ? "now" : i + 1 < step ? "done" : undefined}>
            <span className="num">{i + 1 < step ? "✓" : i + 1}</span> {t}
          </li>
        ))}
      </ol>

      {!plan && reviewMode && <ReviewStart />}
      {!plan && !reviewMode && (
        <div className="start" data-testid="organize-start-box">
          <button
            className="primary big"
            data-testid="organize-start"
            disabled={!r.path || !!r.planning}
            onClick={() => void r.start()}
          >
            {r.planning === "context"
              ? "AI가 업무 맥락을 보는 중…"
              : r.planning
                ? "미리보기 만드는 중…"
                : all
                  ? "전체 훑어 폴더 구성안 만들기"
                  : "정리 미리보기 만들기"}
          </button>
          {all ? (
            <AllWatchesNote />
          ) : (
            <p className="muted small">
              규칙에 맞는 파일부터 보여 줍니다.{" "}
              {aiOn
                ? "규칙으로 옮길 게 없으면 AI가 고객사·이슈별로 묶습니다(분석 전에 비용을 먼저 알려 줍니다)."
                : "규칙에 안 맞는 파일은 그대로 둡니다."}{" "}
              미리보기만 만들고 아무것도 옮기지 않습니다.
            </p>
          )}
        </div>
      )}

      {!r.planning && plan?.source === "rules" && !plan.applied_batch && <Leftover moves={plan.moves.length} />}
      {plan && (
        <div className="plan">
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
          {!plan.applied_batch && (
            <p className="plan-headline" data-testid="plan-headline">
              {movable > 0 ? (
                <>
                  <b>{movable}개</b>는 옮길 곳이 정해져 있어 미리 골라 두었습니다.
                </>
              ) : (
                <>아직 옮길 곳이 정해진 항목이 없습니다.</>
              )}
              {unsure.length > 0 && <> AI가 확신하지 못한 <b>{unsure.length}개</b>는 그대로 둡니다.</>}
            </p>
          )}
          <p className={`notice small no-move${plan.applied_batch ? "" : " warn"}`} data-testid="no-move-notice">
            {plan.applied_batch ? (
              "이 제안은 적용되었습니다. 되돌리기 탭에서 되돌릴 수 있습니다."
            ) : (
              <>
                <b>미리보기입니다.</b> 아래 [선택한 N개 옮기기]를 눌러야 정리 위치에 폴더가 만들어지고 파일이 옮겨집니다.
                옮긴 뒤에도 되돌리기 탭에서 되돌릴 수 있습니다.
              </>
            )}
          </p>
          {!plan.applied_batch && unsure.length > 0 && <Unsure items={unsure} first={movable === 0} />}
          {plan.summary && <p className="muted small">{plan.summary}</p>}
          {plan.source === "review" && !plan.applied_batch && <ReviewAi />}
          {plan.folders.length > 0 && (
            <ul className="small">
              {plan.folders.map((f) => (
                <li key={f.path}>
                  <b>{f.path}</b> — {f.reason}
                </li>
              ))}
            </ul>
          )}
          {plan.source === "context" && (r.preview?.structure?.length ?? 0) > 0 && (
            <Structure dirs={r.preview!.structure!} applied={!!plan.applied_batch} />
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
            <details className="diff-box" open={r.preview.files_moved > 0 && plan.source !== "context"}>
              <summary className="small">
                {plan.applied_batch ? "적용 전후 폴더 모양" : "옮기면 폴더가 어떻게 바뀌는지 보기"} ({r.preview.files_moved}개
                파일 이동)
              </summary>
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
            </details>
          )}

          {!plan.applied_batch && plan.moves.length > 0 && (
            <div className="row apply-bar" data-testid="apply-bar">
              <button className="primary" disabled={r.applying || !r.selected.size} onClick={confirmApply}>
                {r.applying ? "옮기는 중…" : `선택한 ${r.selected.size}개 옮기기`}
              </button>
              {r.selected.size ? (
                <span className="muted small">누르기 전에는 아무것도 옮겨지지 않습니다.</span>
              ) : unsure.length > 0 ? (
                <>
                  <span className="small warn" data-testid="apply-empty-hint">
                    옮길 항목이 없습니다. 확신하지 못한 {unsure.length}개를 어디로 보낼지 정하세요:
                  </span>
                  <SettleButtons items={unsure} small />
                </>
              ) : (
                <span className="small warn" data-testid="apply-empty-hint">
                  옮길 항목을 표에서 하나 이상 체크하세요.
                </span>
              )}
              <div className="spacer" />
              <span className="small">{r.preview && r.selected.size ? `${r.preview.files_moved}개 파일 이동 예정` : ""}</span>
            </div>
          )}
          {r.applied && (
            <div className="notice done-box" data-testid="applied-box">
              <div>
                <b>{r.applied.moved}개 파일을 옮겼습니다.</b>{" "}
                {plan.moves.length - r.selected.size > 0 &&
                  `고르지 않은 ${plan.moves.length - r.selected.size}개는 그대로 있습니다.`}
              </div>
              <div className="row" style={{ marginTop: 6 }}>
                <button
                  onClick={() =>
                    void ipc.undoBatch(r.applied!.batchId).then((u) => alert(`${u.restored.length}개 되돌림`))
                  }
                >
                  이 정리 되돌리기
                </button>
                <button
                  className="primary"
                  data-testid="organize-again"
                  onClick={() => {
                    r.closePlan();
                    void r.start();
                  }}
                >
                  남은 파일로 다시 미리보기
                </button>
              </div>
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

      <details className="more-ways" data-testid="more-ways">
        <summary>다른 방법으로 정리하기 · 지난 제안 열기</summary>
        <div className="row">
          <button disabled={!r.path || all || !!r.planning} onClick={() => plan_("rules")}>
            {r.planning === "rules" ? "계산 중…" : "규칙으로 정리 미리보기"}
          </button>
          <label className="small" title="규칙 정리에만 적용됩니다. 업무 맥락 정리는 하위 폴더를 통째로 한 항목으로 봅니다.">
            <input type="checkbox" checked={recursive} onChange={(e) => setRecursive(e.target.checked)} /> 하위 폴더 포함
          </label>
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
          <button
            disabled={!r.path || all || !!r.planning || aiState !== "ready"}
            title={aiState === "ready" ? "AI가 폴더 구조 전체를 보고 새 구조를 제안합니다" : "설정 → AI 연결에서 AI를 켜고 연결 테스트를 통과해야 합니다"}
            onClick={() => plan_("ai")}
          >
            {r.planning === "ai" ? "AI가 구조를 보는 중… (최대 몇 분)" : "AI 정리 제안"}
          </button>
          <AiNeeded />
        </div>
        <div className="row">
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
      </details>
    </section>
  );
}

/**
 * (M19, SPEC §14.20) The organize root itself was chosen: instead of
 * "organizing" it again, look inside it — merge split customer / issue
 * folders, fold small folders, place loose files — as a preview.
 */
function ReviewStart() {
  const r = useReport();
  const info = r.reviewInfo;
  return (
    <div className="start" data-testid="review-start-box">
      <button
        className="primary big"
        data-testid="review-start"
        disabled={!!r.reviewing || !!r.planning}
        onClick={() => void r.review(false)}
      >
        {r.reviewing ? "정리 위치를 살펴보는 중…" : "정리 위치 점검 미리보기"}
      </button>
      <p className="muted small">
        정리 위치 안을 살펴보고 <b>이름만 다른 같은 고객사 폴더</b>, <b>며칠 차이로 나뉜 같은 이슈 폴더</b>,{" "}
        <b>기타 아래 들어간 고객사 폴더</b>를 합치고, 파일이 적은 작은 폴더는 한 단계 위로 접고, 맨 위에 흩어진
        파일은 규칙대로 넣는 미리보기를 만듭니다. 폴더는 통째로 옮기고(안은 그대로), 코드 프로젝트 폴더는 건드리지
        않습니다. AI는 쓰지 않고, 적용 전에는 아무것도 옮기지 않습니다.
      </p>
      {info && (
        <div className="notice ok" data-testid="review-clean">
          {info.summary}
          <ReviewAi />
        </div>
      )}
    </div>
  );
}

/** (M19) Files a review left for the AI (a customer's file outside the
 * customer folders, loose files no rule takes): one click, after a
 * confirmation, re-runs the review with the AI placing them. */
function ReviewAi() {
  const r = useReport();
  const aiOn = useAiOn();
  const info = r.reviewInfo;
  if (!info) return null;
  if (info.aiError)
    return (
      <p className="small warn" data-testid="review-ai-error">
        AI가 답하지 못했습니다({info.aiError}). 나머지 제안은 그대로 쓸 수 있습니다.
      </p>
    );
  if (!info.aiWaiting) return null;
  const ask = () => {
    if (
      window.confirm(
        `고객사 폴더 밖에 있거나 맨 위에 흩어진 파일 ${info.aiWaiting}개를 AI에 보내 고객사·이슈 폴더를 정합니다(이미 분석한 파일은 다시 묻지 않음). 미리보기만 다시 만들고 아무것도 옮기지 않습니다. 진행할까요?`,
      )
    )
      void r.review(true);
  };
  return (
    <div className="row" style={{ marginTop: 6 }} data-testid="review-ai">
      <span className="small">
        AI가 옮길 곳을 정할 수 있는 파일 <b>{info.aiWaiting}개</b>가 더 있습니다.
      </span>
      {aiOn ? (
        <button className="sm" disabled={!!r.reviewing} onClick={ask} data-testid="review-ai-run">
          {r.reviewing === "ai" ? "AI가 보는 중…" : "AI로 이것도 보기"}
        </button>
      ) : (
        <span className="muted small">(AI를 켜면 쓸 수 있습니다)</span>
      )}
    </div>
  );
}

/** (0.12.0) What "모든 감시 폴더" does: where files come from and where the
 * folders go (with how many are already there). */
function AllWatchesNote() {
  const d = useCurate((s) => s.loaded ?? s.draft);
  const load = useCurate((s) => s.load);
  useEffect(() => {
    void load();
  }, [load]);
  const sources = d?.sources ?? [];
  const targets = d?.targets ?? [];
  return (
    <div className="muted small" data-testid="all-watches-note">
      감시 폴더 {sources.length ? `${sources.length}곳(${sources.map((s) => s.alias || s.path.split(/[\\/]/).pop()).join(", ")})` : ""}의
      파일과 하위 폴더를 한 번에 보고, 고객사·이슈별 <b>폴더 구성안</b>을 먼저 보여 줍니다.{" "}
      {targets.length > 0 &&
        `정리 위치(${targets.map((t) => `${t.path} — 이미 있는 폴더 ${t.folders}개`).join(", ")})의 같은 고객사·이슈 폴더는 새로 만들지 않고 다시 씁니다. `}
      분석 전에 비용을 먼저 알려 주고, 구성안을 확인하고 옮기기 전에는 아무것도 옮기지 않습니다.
    </div>
  );
}

/**
 * (0.12.0) The folder layout a work-context plan proposes (customer →
 * issue → kind): which folders already exist and are reused, which are
 * made — the thing to approve before anything moves.
 */
function Structure({ dirs, applied }: { dirs: StructDir[]; applied: boolean }) {
  const anchors = [...new Set(dirs.map((d) => d.anchor))];
  const fresh = dirs.filter((d) => !d.exists);
  // Count only the top-most new folders (a new customer brings its issues).
  const newTop = fresh.filter((d) => !fresh.some((o) => o !== d && o.anchor === d.anchor && d.path.startsWith(`${o.path}/`)));
  const reused = dirs.filter((d) => d.exists).length;
  return (
    <details className="structure-box" open data-testid="structure">
      <summary>
        <b>폴더 구성안</b> — 이미 있는 폴더 {reused}곳을 다시 쓰고, 새 폴더 {newTop.length}곳을{" "}
        {applied ? "만들었습니다" : "만듭니다"}
      </summary>
      {anchors.map((a) => (
        <div key={a} className="structure">
          <div className="muted small" title={a}>
            📂 {a}
          </div>
          <ul>
            {dirs
              .filter((d) => d.anchor === a)
              .map((d) => (
                <li
                  key={d.path}
                  style={{ paddingLeft: `${(d.depth - 1) * 18}px` }}
                  data-testid="structure-dir"
                  data-exists={d.exists ? "1" : "0"}
                >
                  📁 {d.path.split("/").pop()}{" "}
                  {d.exists ? (
                    <span className="tag ok">기존</span>
                  ) : (
                    <span className="tag new">새 폴더</span>
                  )}{" "}
                  <span className="muted small">
                    {d.items}개 항목 · 파일 {d.files.toLocaleString()}개
                  </span>
                </li>
              ))}
          </ul>
        </div>
      ))}
      <p className="muted small">
        고객사·이슈를 표에서 고치면 구성안도 바로 바뀝니다. 체크한 항목만 옮기고, 새 폴더는 옮길 때 만들어집니다.
      </p>
    </details>
  );
}

/**
 * (0.11.3) The work-context items the AI was unsure about (below
 * min_confidence). Before this they silently stayed, so a plan of only
 * 미분류 items offered "선택한 0개 적용" and nothing else.
 */
function Unsure({ items, first }: { items: PlannedMove[]; first: boolean }) {
  return (
    <div className={`notice unsure${first ? " warn" : ""}`} data-testid="unsure-box">
      <div>
        <b>AI가 확신하지 못한 항목 {items.length}개</b> — 지금은 옮기지 않고 그대로 둡니다. 한 번에 정할 수 있습니다(아직
        옮기지 않음):
      </div>
      <SettleButtons items={items} primary={first} />
      <div className="muted small">
        하나씩 정하려면 표에서 고객사나 이슈/주제를 고치세요. 그대로 두려면 아무것도 누르지 않아도 됩니다.
      </div>
    </div>
  );
}

function SettleButtons({ items, primary, small }: { items: PlannedMove[]; primary?: boolean; small?: boolean }) {
  const r = useReport();
  const guessable = items.filter((m) => m.context?.entity || m.context?.topic).length;
  const cls = (main: boolean) => [main && primary ? "primary" : "", small ? "sm" : ""].join(" ").trim() || undefined;
  return (
    <span className="row settle">
      {guessable > 0 && (
        <button
          className={cls(true)}
          data-testid="settle-guess"
          disabled={r.settling}
          title="확신은 낮지만 AI가 고른 고객사·주제 폴더로 넣습니다. 표에서 결과를 보고 고칠 수 있습니다"
          onClick={() => void r.settle("guess")}
        >
          AI 추측대로 넣기{guessable < items.length ? ` (${guessable}개, 나머지는 종류별)` : ""}
        </button>
      )}
      <button
        className={cls(guessable === 0)}
        data-testid="settle-kind"
        disabled={r.settling}
        title="고객사 없이 기타/미분류/종류(문서·데이터·사진…) 폴더에 모읍니다"
        onClick={() => void r.settle("by_kind")}
      >
        기타/미분류에 종류별로 모으기
      </button>
    </span>
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
