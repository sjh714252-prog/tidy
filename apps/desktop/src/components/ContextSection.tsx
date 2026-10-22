import { useEffect, useState } from "react";
import type { ContextCandidates, ContextDict } from "../ipc/types";
import { useCurate } from "../store/curate";
import { useAiOn } from "./AiNeeded";
import { Help } from "./Help";

const csv = (s: string) =>
  s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);

/**
 * Settings → 업무 맥락 (SPEC §14.3, M10): the context dictionary
 * (customers + aliases, issue types, kinds of material), "바탕화면에서 뽑기",
 * folder layouts and the watch options. Needs the AI: disabled without it.
 */
export function ContextSection() {
  const { draft, extracting, candidates, message, error, load, patch, setDict, extract, takeCandidates, dropCandidates } =
    useCurate();
  const aiOn = useAiOn();

  useEffect(() => {
    void load();
  }, [load]);

  if (!draft) return <p className="muted small">{error ?? "업무 맥락 설정 불러오는 중…"}</p>;
  const d = draft.dict;
  const s = draft.settings;
  const upd = (f: (x: ContextDict) => void) => {
    const next = structuredClone(d);
    f(next);
    setDict(next);
  };

  return (
    <div className="context-section" data-testid="context-section">
      <p className="muted small">
        파일 이름·날짜·본문 앞부분을 AI에 보내 "어느 고객사의 어느 이슈" 자료인지 판단해{" "}
        <code>고객사/날짜_이슈/종류</code>로 묶습니다. 리포트 탭의 "업무 맥락으로 정리"에서 미리보기를 보고 승인해야
        옮깁니다. 지금까지 분석한 파일 {draft.analysed}개(다시 묻지 않음).
      </p>
      <fieldset disabled={!aiOn}>
        <h3>
          맥락 사전 <Help id="context" />
        </h3>
        <div className="row">
          <button onClick={() => void extract()} disabled={extracting} data-testid="extract-context">
            {extracting ? "AI가 감시 폴더를 보는 중…" : "바탕화면에서 뽑기"}
          </button>
          <span className="muted small">감시 폴더의 파일 이름(과 본문 앞부분)에서 고객사·이슈 후보를 뽑습니다. 고른 것만 사전에 넣습니다.</span>
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
          감시 폴더의 정리 방식을 따릅니다(이 폴더 안 / 기준 폴더 아래). 같은 고객사·이슈는 14일 안이면 가장 이른 날짜
          폴더로 묶습니다.
        </p>
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

        <h3>감시 중 새 파일</h3>
        <label className="row">
          <input
            type="checkbox"
            data-testid="curate-enabled"
            checked={s.enabled}
            onChange={(e) => patch({ enabled: e.target.checked })}
          />
          <span>
            규칙에 안 걸린 새 파일을 업무 맥락으로 정리 — 확신이 높고 폴더가 이미 있을 때만 바로 옮기고, 새 고객사·이슈는
            확인 팝업으로 묻습니다
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
        파일 {c.files}개에서 뽑은 후보입니다. 넣을 것만 체크하고 이름을 고치세요(아직 저장되지 않음).
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
