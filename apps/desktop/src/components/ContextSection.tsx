import { useEffect, useState } from "react";
import type { ContextCandidates, ContextDict } from "../ipc/types";
import { useCurate } from "../store/curate";
import { useNav } from "../store/nav";
import { useReport } from "../store/report";
import { useAiOn } from "./AiNeeded";
import { Help } from "./Help";

const csv = (s: string) =>
  s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);

/**
 * Settings → 업무 맥락 (SPEC §14.3, reworked in 0.12.0 §14.18) as the order
 * it is used in: where files come from → where folders go (and what is
 * already there), ① the customer / issue dictionary from every watch
 * folder, ② first run: survey all scattered files → folder layout → approve,
 * ③ new files later: existing folder at once, a new folder asks. Folder
 * layouts and thresholds are under "세부 설정". Needs the AI.
 */
export function ContextSection() {
  const { draft, extracting, candidates, message, error, load, patch, setDict, extract, takeCandidates, dropCandidates, dirty } =
    useCurate();
  const aiOn = useAiOn();
  const setTab = useNav((s) => s.setTab);
  const surveyAll = useReport((s) => s.surveyAll);

  useEffect(() => {
    void load();
  }, [load]);

  if (!draft) return <p className="muted small">{error ?? "업무 맥락 설정 불러오는 중…"}</p>;
  const d = draft.dict;
  const s = draft.settings;
  const sources = draft.sources ?? [];
  const targets = draft.targets ?? [];
  const upd = (f: (x: ContextDict) => void) => {
    const next = structuredClone(d);
    f(next);
    setDict(next);
  };
  const unsaved = dirty();
  const survey = () => {
    setTab("report");
    void surveyAll();
  };
  const name = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

  return (
    <div className="context-section" data-testid="context-section">
      <p className="muted small">
        감시 폴더의 파일이 "어느 고객사의 어느 이슈" 자료인지 AI가 판단해, 정리 위치 아래{" "}
        <code>고객사/날짜_이슈/종류</code> 폴더로 모읍니다. 미리보기를 보고 승인해야 옮깁니다. 지금까지 분석한 파일{" "}
        {draft.analysed}개(다시 묻지 않음).
      </p>
      <div className="context-route" data-testid="context-route">
        <span className="small">가져올 곳</span>
        {sources.length === 0 && <span className="muted small">감시 폴더 없음</span>}
        {sources.map((x) => (
          <span key={x.path} className="tag" title={x.path}>
            {x.alias || name(x.path)}
            {x.inPlace ? " (그 폴더 안에 정리)" : ""}
          </span>
        ))}
        <span className="small">→ 만들 곳</span>
        {targets.length === 0 && <span className="muted small">각 감시 폴더 안</span>}
        {targets.map((t) => (
          <span key={t.path} className="tag ok" title={t.names.length ? `이미 있는 폴더: ${t.names.join(", ")}${t.folders > t.names.length ? " …" : ""}` : t.path}>
            {t.path} · 이미 있는 폴더 {t.folders}개
          </span>
        ))}
      </div>
      <fieldset disabled={!aiOn}>
        <ol className="context-flow">
          <li>
            <h3>
              ① 고객사·이슈 사전 <Help id="context" />
            </h3>
            <div className="row">
              <button onClick={() => void extract()} disabled={extracting} data-testid="extract-context">
                {extracting ? "AI가 감시 폴더를 보는 중…" : `감시 폴더 ${sources.length || ""}${sources.length ? "곳" : ""}에서 뽑기`}
              </button>
              <span className="muted small">
                모든 감시 폴더의 폴더·파일 이름(과 본문 앞부분)에서 고객사·이슈 후보를 뽑습니다. 정리 위치에 이미 있는 고객사
                폴더 이름은 그 철자를 그대로 씁니다. 고른 것만 사전에 넣습니다.
              </span>
            </div>
            {candidates && <Candidates c={candidates} onTake={takeCandidates} onCancel={dropCandidates} />}

            <table className="grid dict">
              <thead>
                <tr>
                  <th>고객사·프로젝트</th>
                  <th>별칭 (쉼표로 구분: 영문·약칭)</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {d.entity.map((e, i) => (
                  <tr key={i} data-testid="dict-entity">
                    <td>
                      <input value={e.name} onChange={(ev) => upd((x) => (x.entity[i]!.name = ev.target.value))} />
                    </td>
                    <td>
                      <input
                        className="wide"
                        value={e.aliases.join(", ")}
                        onChange={(ev) => upd((x) => (x.entity[i]!.aliases = csv(ev.target.value)))}
                      />
                    </td>
                    <td>
                      <button className="link" onClick={() => upd((x) => x.entity.splice(i, 1))}>
                        삭제
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <button onClick={() => upd((x) => x.entity.push({ name: "", aliases: [] }))}>+ 고객사 추가</button>
            <label className="row">
              <span>이슈 유형</span>
              <input
                className="wide"
                value={d.issue_type.map((i) => i.name).join(", ")}
                onChange={(ev) => upd((x) => (x.issue_type = csv(ev.target.value).map((name) => ({ name }))))}
              />
            </label>
          </li>

          <li data-testid="context-step-survey">
            <h3>② 처음 한 번: 흩어진 파일 전체 훑기</h3>
            <p className="small">
              감시 폴더 전체의 파일·하위 폴더를 한 번에 분석해 <b>폴더 구성안</b>(고객사 → 날짜_이슈 → 종류)을 먼저
              보여 줍니다. 정리 위치에 이미 있는 같은 고객사·이슈 폴더는 다시 쓰고(띄어쓰기·대소문자 차이, 14일 안의 같은
              이슈), 새로 만들 폴더는 "새 폴더"로 표시합니다. 구성안을 확인하고 고른 것만 옮깁니다.
            </p>
            <div className="row">
              <button
                className="primary"
                data-testid="survey-all"
                disabled={unsaved}
                title={unsaved ? "저장하지 않은 사전 변경이 있습니다 — 먼저 저장하세요" : "정리하기 탭에서 비용 안내 → 분석 → 폴더 구성안"}
                onClick={survey}
              >
                전체 훑어 폴더 구성안 만들기
              </button>
              {unsaved && <span className="small warn">사전을 먼저 저장하세요(아래 저장).</span>}
            </div>
          </li>

          <li>
            <h3>③ 그 뒤 새로 생기는 파일</h3>
            <label className="row">
              <input
                type="checkbox"
                data-testid="curate-enabled"
                checked={s.enabled}
                onChange={(e) => patch({ enabled: e.target.checked })}
              />
              <span>
                규칙에 안 걸린 새 파일도 업무 맥락으로 정리 — <b>이미 있는 폴더</b>로 갈 파일은 확신이 높으면 바로 옮기고,{" "}
                <b>새 폴더</b>(새 고객사·새 이슈)가 필요하면 확인 대기에서 "폴더를 만들까요?"로 묻습니다
              </span>
            </label>
            <label className="row">
              <input
                type="checkbox"
                disabled={!s.enabled}
                checked={s.prefer}
                onChange={(e) => patch({ prefer: e.target.checked })}
              />
              <span>맥락 정리 우선 (규칙보다 먼저)</span>
              <Help id="curatePrefer" />
            </label>
            <p className="muted small">감시 폴더 바로 아래의 새 파일만 이렇게 정리합니다. 새 하위 폴더는 ②처럼 미리보기로 정리합니다.</p>
          </li>
        </ol>

        <details className="context-advanced" data-testid="context-advanced">
          <summary>세부 설정 — 자료 종류 · 폴더 틀 · 미분류 기준</summary>
          <label className="row">
            <span>자료 종류</span>
            <input
              className="wide"
              title="이름(확장자 확장자) 형식: 로그(log txt), 캡처(png jpg)"
              value={d.doc_kind.map((k) => (k.ext.length ? `${k.name}(${k.ext.join(" ")})` : k.name)).join(", ")}
              onChange={(ev) =>
                upd(
                  (x) =>
                    (x.doc_kind = csv(ev.target.value).map((t) => {
                      const m = t.match(/^(.*?)\s*\(([^)]*)\)\s*$/);
                      return m ? { name: (m[1] ?? "").trim(), ext: (m[2] ?? "").split(/\s+/).filter(Boolean) } : { name: t, ext: [] };
                    })),
                )
              }
            />
          </label>
          <h3>
            폴더 틀 <Help id="curateTemplate" />
          </h3>
          <label className="row">
            <span>고객사 있음</span>
            <input className="wide" value={s.template} onChange={(e) => patch({ template: e.target.value })} />
          </label>
          <label className="row">
            <span>고객사 없음</span>
            <input
              className="wide"
              value={s.other_template}
              onChange={(e) => patch({ other_template: e.target.value })}
            />
          </label>
          <p className="muted small">
            감시 폴더의 정리 방식을 따릅니다(이 폴더 안 / 정리 위치 아래). 같은 고객사·이슈는 14일 안이면 가장 이른 날짜
            폴더로 묶고, 이미 있는 폴더가 있으면 그 폴더를 씁니다.
          </p>
          <label className="row">
            <span>작은 폴더 만들지 않기</span>
            <input
              type="number"
              min={1}
              max={50}
              step={1}
              className="narrow"
              data-testid="curate-min-folder"
              value={s.min_folder_items ?? 3}
              onChange={(e) => patch({ min_folder_items: Math.max(1, Number(e.target.value) || 1) })}
            />
            <span className="muted small">
              개 미만이 들어갈 자료 종류·작은 주제 폴더(<code>로그/</code>, <code>정책/문자 정책</code>)는 새로 만들지 않고 한
              단계 위 폴더에 둡니다. 이미 있는 폴더는 그대로 씁니다.
            </span>
          </label>
          <label className="row">
            <span>미분류 기준</span>
            <input
              type="number"
              min={0}
              max={1}
              step={0.05}
              className="narrow"
              value={s.min_confidence}
              onChange={(e) => patch({ min_confidence: Number(e.target.value) })}
            />
            <Help id="curateMin" />
          </label>
        </details>
      </fieldset>
      {message && <p className="ok small">{message}</p>}
      {error && <p className="error small">{error}</p>}
    </div>
  );
}

/** Tick / rename the AI's candidates before they go into the dictionary. */
function Candidates({
  c,
  onTake,
  onCancel,
}: {
  c: ContextCandidates;
  onTake: (c: ContextCandidates) => void;
  onCancel: () => void;
}) {
  const [ents, setEnts] = useState(c.entities.map((e) => ({ ...e, on: !e.known })));
  const [issues, setIssues] = useState(c.issue_types.map((i) => ({ ...i, on: !i.known })));
  return (
    <div className="candidates" data-testid="candidates">
      <p className="small">
        {c.folders?.length ? `감시 폴더 ${c.folders.length}곳(${c.folders.join(", ")})의 ` : ""}파일 {c.files}개에서 뽑은 후보입니다. 넣을 것만 체크하고 이름을 고치세요(아직 저장되지 않음).
      </p>
      <ul>
        {ents.map((e, i) => (
          <li key={i} data-testid="candidate-entity">
            <input
              type="checkbox"
              checked={e.on}
              onChange={(ev) => setEnts(ents.map((x, j) => (j === i ? { ...x, on: ev.target.checked } : x)))}
            />
            <input
              value={e.name}
              onChange={(ev) => setEnts(ents.map((x, j) => (j === i ? { ...x, name: ev.target.value } : x)))}
            />
            <span className="muted small">
              별칭 {e.aliases.join(", ") || "없음"} · 파일 {e.files}개{e.known ? " · 이미 사전에 있음" : ""} — {e.reason}
            </span>
          </li>
        ))}
        {issues.map((x, i) => (
          <li key={`i${i}`}>
            <input
              type="checkbox"
              checked={x.on}
              onChange={(ev) => setIssues(issues.map((y, j) => (j === i ? { ...y, on: ev.target.checked } : y)))}
            />
            <span>이슈 유형: {x.name}</span>
            {x.known && <span className="muted small"> (이미 있음)</span>}
          </li>
        ))}
      </ul>
      <div className="row">
        <button
          className="primary"
          onClick={() =>
            onTake({
              ...c,
              entities: ents.filter((e) => e.on && e.name.trim()),
              issue_types: issues.filter((i) => i.on),
              doc_kinds: c.doc_kinds.filter((k) => !k.known),
            })
          }
        >
          고른 것 사전에 넣기
        </button>
        <button onClick={onCancel}>닫기</button>
      </div>
    </div>
  );
}
