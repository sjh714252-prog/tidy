import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { ContextSuggestion, Plan, PlannedMove } from "../ipc/types";
import { useCurate } from "../store/curate";
import { useReport } from "../store/report";
import { formatSize } from "./format";

const NONE = "";

/**
 * Work-context plan rows (SPEC §14.3): per item the AI's judgement and its
 * reason, with customer / issue / kind / date editable. An edit re-lays out
 * the plan (nothing moves) and may suggest dictionary additions. (M12,
 * §14.7) A sub-folder is one row (📁, moved as a whole): its inner tree
 * opens below it, and "파일별로 나누기" re-analyzes its files instead.
 */
export function ContextTable({ plan, rel }: { plan: Plan; rel: (p: string) => string }) {
  const r = useReport();
  const dict = useCurate((s) => s.loaded?.dict ?? s.draft?.dict);
  const loadDict = useCurate((s) => s.load);
  const [groups, setGroups] = useState<[string, string][]>([]);
  useEffect(() => {
    void loadDict();
    void ipc.contextGroups().then(setGroups, () => {});
  }, [loadDict, plan.id]);

  const movable = plan.moves.filter((m) => m.from !== m.to);
  const entities = new Set<string>(dict?.entity.map((e) => e.name) ?? []);
  for (const m of plan.moves) if (m.context?.entity) entities.add(m.context.entity);
  for (const [e] of groups) entities.add(e);
  const issues = new Set<string>(dict?.issue_type.map((i) => i.name) ?? []);
  for (const m of plan.moves) if (m.context?.issue) issues.add(m.context.issue);
  for (const [, i] of groups) issues.add(i);
  const kinds = new Set<string>(dict?.doc_kind.map((k) => k.name) ?? []);
  for (const m of plan.moves) if (m.context?.doc_kind) kinds.add(m.context.doc_kind);
  const locked = !!plan.applied_batch;

  return (
    <>
      <datalist id="ctx-issues">
        {[...issues].map((i) => (
          <option key={i} value={i} />
        ))}
      </datalist>
      <table className="grid context-plan" data-testid="context-plan">
        <thead>
          <tr>
            <th>
              <input
                type="checkbox"
                aria-label="모두 선택"
                checked={movable.length > 0 && r.selected.size === movable.length}
                onChange={(e) => r.setAll(e.target.checked)}
              />
            </th>
            <th>#</th>
            <th>지금</th>
            <th>고객사</th>
            <th>이슈 / 주제</th>
            <th>날짜</th>
            <th>종류</th>
            <th>옮길 곳</th>
            <th>AI 판단</th>
          </tr>
        </thead>
        <tbody>
          {plan.moves.map((m) => (
            <Row
              key={m.id}
              m={m}
              rel={rel}
              locked={locked}
              entities={[...entities]}
              kinds={[...kinds]}
            />
          ))}
        </tbody>
      </table>
    </>
  );
}

function Row({
  m,
  rel,
  locked,
  entities,
  kinds,
}: {
  m: PlannedMove;
  rel: (p: string) => string;
  locked: boolean;
  entities: string[];
  kinds: string[];
}) {
  const r = useReport();
  const c = m.context;
  const stays = m.from === m.to;
  const [issue, setIssue] = useState(c?.entity ? c.issue : (c?.topic ?? ""));
  useEffect(() => setIssue(c?.entity ? c.issue : (c?.topic ?? "")), [c?.entity, c?.issue, c?.topic]);
  const [open, setOpen] = useState(false);
  if (!c) return null;
  const busy = r.editing === m.id || r.splitting === m.id;
  const name = m.from.split(/[\\/]/).pop() ?? m.from;
  const base = { entity: c.entity, issue: c.issue, docKind: c.doc_kind, topic: c.topic };
  const commitIssue = () => {
    const v = issue.trim();
    if (!v || v === (c.entity ? c.issue : c.topic)) return;
    void r.editMove(m.id, c.entity ? { ...base, issue: v } : { ...base, topic: v });
  };
  const pct = Math.round(c.confidence * 100);
  const merged = !!c.date && !!c.own_date && c.own_date !== c.date;
  const split = () => {
    if (
      window.confirm(
        `"${name}" 폴더를 통째로 옮기지 않고, 안의 파일 ${m.files}개를 하나씩 다시 분석합니다(AI 요청). 진행할까요?`,
      )
    )
      void r.splitFolder(m.id);
  };

  const row = (
    <tr
      className={[stays ? "stays" : "", m.is_dir ? "folder-item" : ""].join(" ").trim() || undefined}
      data-testid="context-row"
      data-name={name}
      data-kind={m.is_dir ? "folder" : "file"}
    >
      <td>
        <input
          type="checkbox"
          disabled={locked || stays}
          checked={r.selected.has(m.id)}
          onChange={() => r.toggle(m.id)}
        />
      </td>
      <td className="muted">{m.id}</td>
      <td className="path" title={m.from}>
        {m.is_dir ? (
          <>
            <button
              className="link"
              data-testid="folder-toggle"
              title="안의 폴더 구조 보기"
              aria-expanded={open}
              onClick={() => setOpen(!open)}
            >
              {open ? "▾" : "▸"} 📁 {rel(m.from)}
            </button>
            <div className="muted small" data-testid="folder-count">
              파일 {m.files.toLocaleString()}개 · {formatSize(m.bytes)}
            </div>
            {!locked && (
              <button
                className="link small"
                data-testid="folder-split"
                disabled={busy}
                title="폴더를 통째로 옮기지 않고 안의 파일을 하나씩 분석합니다"
                onClick={split}
              >
                {r.splitting === m.id ? "파일별로 분석 중…" : "이 폴더는 파일별로 나눠서"}
              </button>
            )}
          </>
        ) : (
          rel(m.from)
        )}
      </td>
      <td>
        <select
          aria-label="고객사"
          data-testid="ctx-entity"
          disabled={locked || busy}
          value={c.entity ?? NONE}
          onChange={(e) => void r.editMove(m.id, { ...base, entity: e.target.value || null })}
        >
          <option value={NONE}>(고객사 없음)</option>
          {entities.map((e) => (
            <option key={e} value={e}>
              {e}
            </option>
          ))}
        </select>
      </td>
      <td>
        <input
          aria-label={c.entity ? "이슈" : "주제"}
          data-testid="ctx-issue"
          list={c.entity ? "ctx-issues" : undefined}
          placeholder={c.entity ? "이슈" : "큰 주제/작은 주제"}
          disabled={locked || busy}
          value={issue}
          onChange={(e) => setIssue(e.target.value)}
          onBlur={commitIssue}
          onKeyDown={(e) => e.key === "Enter" && commitIssue()}
        />
      </td>
      <td
        className="small"
        title={
          (merged ? `${m.is_dir ? "폴더" : "파일"} 날짜 ${c.own_date} → 가까운 날짜의 같은 이슈로 묶음. ` : "") +
          (m.is_dir ? "폴더 날짜 = 안의 가장 최근 파일 날짜. 고칠 수 있습니다." : "날짜를 고칠 수 있습니다.")
        }
      >
        {locked ? (
          (c.date || c.own_date).replaceAll("-", "")
        ) : (
          <input
            type="date"
            className="date-edit"
            aria-label="날짜"
            data-testid="ctx-date"
            disabled={busy}
            value={c.own_date}
            onChange={(e) => e.target.value && void r.editMove(m.id, { ...base, date: e.target.value })}
          />
        )}
        {merged && <div className="muted">→ {c.date.replaceAll("-", "")} (묶음)</div>}
      </td>
      <td>
        <select
          aria-label="종류"
          disabled={locked || busy}
          value={c.doc_kind}
          onChange={(e) => void r.editMove(m.id, { ...base, docKind: e.target.value })}
        >
          {[...new Set([c.doc_kind, ...kinds])].map((k) => (
            <option key={k} value={k}>
              {k}
            </option>
          ))}
        </select>
      </td>
      <td className="path" title={m.to}>
        {stays ? <span className="muted">그대로 둠</span> : rel(m.to)}
      </td>
      <td className="small">
        {c.unclassified ? (
          <span className="tag warn">미분류</span>
        ) : c.edited ? (
          <span className="tag">직접 고침</span>
        ) : (
          <span className={pct >= 90 ? "tag ok" : "tag"}>확신 {pct}%</span>
        )}{" "}
        <span className="reason">{c.reason}</span>
        {m.warnings.map((w) => (
          <div key={w} className="warn">
            ⚠ {w}
          </div>
        ))}
      </td>
    </tr>
  );
  if (!m.is_dir || !open) return row;
  return (
    <>
      {row}
      <tr className="folder-inner" data-testid="folder-inner">
        <td />
        <td colSpan={8}>
          <pre className="item-tree">{c.tree || "(안의 구조를 아직 모릅니다)"}</pre>
        </td>
      </tr>
    </>
  );
}

/** "사전에 반영": what the dictionary could learn from the user's edits. */
export function Suggestions() {
  const r = useReport();
  const [off, setOff] = useState<Set<string>>(new Set());
  const picked: ContextSuggestion[] = r.suggestions.filter((s) => !off.has(s.label)).map((s) => s.suggestion);
  return (
    <div className="notice suggestions" data-testid="context-suggestions">
      <p className="small">고친 내용을 맥락 사전에 넣어 두면 다음 분석부터 같은 이름으로 묶습니다.</p>
      <ul className="small">
        {r.suggestions.map((s) => (
          <li key={s.label}>
            <label>
              <input
                type="checkbox"
                checked={!off.has(s.label)}
                onChange={(e) => {
                  const n = new Set(off);
                  if (e.target.checked) n.delete(s.label);
                  else n.add(s.label);
                  setOff(n);
                }}
              />{" "}
              {s.label}
            </label>
          </li>
        ))}
      </ul>
      <div className="row">
        <button
          className="primary"
          disabled={!picked.length}
          onClick={() =>
            void r.learn(picked).then(() => {
              setOff(new Set());
              void useCurate.getState().load();
            })
          }
        >
          사전에 반영
        </button>
        <button onClick={r.dropSuggestions}>이번만 (사전은 그대로)</button>
      </div>
    </div>
  );
}
