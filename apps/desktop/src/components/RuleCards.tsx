import { useState, type ComponentProps } from "react";
import type { PreviewReport, RuleCard } from "../ipc/types";
import { FolderTree } from "./FolderTree";
import { destFolder, isAbsoluteDest, relativeSuggestion } from "./destPaths";
import { shortenTo } from "./paths";

const VARS: { v: string; tip: string }[] = [
  { v: "{yyyy}", tip: "수정 연도" },
  { v: "{mm}", tip: "수정 월" },
  { v: "{dd}", tip: "수정 일" },
  { v: "{ext}", tip: "확장자" },
  { v: "{stem}", tip: "확장자 뺀 이름" },
  { v: "{source_dir_name}", tip: "원래 폴더 이름" },
  { v: "{root}", tip: "기준 폴더(감시 폴더 또는 정리 위치)" },
];

const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
const isAbsolute = (p: string) => /^([a-zA-Z]:|\/|\\\\|~)/.test(p.trim());

/** Folder picked in the tree → dest text: relative when inside an anchor. */
function destFor(path: string, anchors: string[]): string {
  const p = norm(path);
  for (const a of anchors.map(norm).sort((x, y) => y.length - x.length)) {
    if (p === a) return "{root}";
    if (p.startsWith(a + "/")) return p.slice(a.length + 1);
  }
  return path;
}

function outside(dest: string, anchors: string[]): boolean {
  const d = dest.trim();
  if (!d || !isAbsolute(d) || d.startsWith("{root}")) return false;
  const n = norm(d);
  return !anchors.some((a) => n === norm(a) || n.startsWith(norm(a) + "/"));
}

/** "pdf · hwp · docx, 이름 '견적서'" — the collapsed line's condition. */
function condSummary(c: RuleCard): string {
  const out: string[] = [];
  if (c.ext.length) out.push(c.ext.join(" · "));
  if (c.nameContains) out.push(`이름에 '${c.nameContains}'`);
  if (c.nameRegex) out.push(`이름 ${c.nameRegex}`);
  if (c.minSize || c.maxSize) out.push(`크기 ${c.minSize || "0"}~${c.maxSize || ""}`);
  if (c.newerThanDays) out.push(`최근 ${c.newerThanDays}일`);
  if (c.olderThanDays) out.push(`${c.olderThanDays}일 넘게 안 씀`);
  return out.join(", ") || "모든 파일";
}

/**
 * Rules (SPEC §14.8 5): each [[rule]] is one collapsed line — ⋮⋮ / name /
 * condition / → destination / 지금 N개 / 펼치기; one card open at a time.
 * Rules whose absolute destination ignores the organize location get an
 * orange border and "상대 경로로".
 */
export function RuleCards({
  cards,
  preview,
  anchors,
  collectRoots = [],
  readOnly,
  aiMarked,
  open,
  onOpen,
  onChange,
  treeSource,
}: {
  cards: RuleCard[];
  preview: PreviewReport | null;
  /** Watch folders' anchors (relative destinations resolve against them). */
  anchors: string[];
  /** (M11) "지정 위치로 모으기" roots: absolute destinations ignore them. */
  collectRoots?: string[];
  readOnly: boolean;
  /** Card indexes that came from "AI로 규칙 만들기" (badge only). */
  aiMarked?: Set<number>;
  /** Index of the open card (accordion). */
  open: number | null;
  onOpen: (i: number | null) => void;
  onChange: (cards: RuleCard[]) => void;
  /** (M16) "폴더 선택…" browses this machine instead (a NAS). */
  treeSource?: ComponentProps<typeof FolderTree>["source"];
}) {
  const [drag, setDrag] = useState<number | null>(null);
  const [treeFor, setTreeFor] = useState<number | null>(null);
  const [extDraft, setExtDraft] = useState("");
  const [moreFor, setMoreFor] = useState<number | null>(null);

  const set = (i: number, p: Partial<RuleCard>) => onChange(cards.map((c, j) => (j === i ? { ...c, ...p } : c)));
  const move = (from: number, to: number) => {
    if (to < 0 || to >= cards.length || from === to) return;
    const next = [...cards];
    const [c] = next.splice(from, 1);
    if (!c) return;
    next.splice(to, 0, c);
    onChange(next);
    if (open === from) onOpen(to);
  };
  const addExt = (i: number, list: string[]) => {
    const cur = cards[i]?.ext ?? [];
    const add = list.map((e) => e.trim().replace(/^\./, "").toLowerCase()).filter((e) => e && !cur.includes(e));
    if (add.length) set(i, { ext: [...cur, ...add] });
  };
  const anchor = collectRoots[0] ?? anchors[0] ?? "";
  const sep = anchor.includes("\\") ? "\\" : "/";
  const where = (c: RuleCard, sample: string | null | undefined) => {
    if (sample) return sample;
    const d = destFolder(c.dest);
    if (!d) return anchor;
    if (isAbsolute(d)) return d;
    return anchor ? `${anchor.replace(/[\\/]+$/, "")}${sep}${d.replace(/^\{root\}[\\/]?/, "").replace(/[\\/]/g, sep)}` : d;
  };

  return (
    <ol className="rule-cards">
      {cards.map((c, i) => {
        const pv = preview?.rules.find((r) => r.index === i);
        const abs = collectRoots.length > 0 && isAbsoluteDest(c.dest);
        const isOpen = open === i;
        const countText = !c.enabled ? "사용 안 함" : pv ? `지금 ${pv.count.toLocaleString()}개` : "";
        return (
          <li
            key={i}
            className={`rule-card${abs ? " abs" : ""}${isOpen ? " open" : ""}${drag === i ? " dragging" : ""}`}
            data-testid="rule-card"
            data-open={isOpen ? "true" : "false"}
            onDragOver={(e) => {
              if (drag !== null) e.preventDefault();
            }}
            onDrop={(e) => {
              e.preventDefault();
              if (drag !== null) move(drag, i);
              setDrag(null);
            }}
          >
            {!isOpen ? (
              <div className="rule-line" data-testid="rule-line">
                <span
                  className="handle"
                  draggable={!readOnly}
                  onDragStart={() => setDrag(i)}
                  onDragEnd={() => setDrag(null)}
                  title="끌어서 순서 바꾸기 (위가 우선)"
                  aria-hidden
                >
                  ⋮⋮
                </span>
                <span className="rname" data-testid="rule-name" title={c.name}>
                  {c.name || "(이름 없음)"}
                  {c.confirm && <small>확인 후</small>}
                  {!c.enabled && <small className="off">사용 안 함</small>}
                  {aiMarked?.has(i) && <span className="tag ai">AI 제안</span>}
                </span>
                <span className="cond" title={condSummary(c)}>
                  {condSummary(c)}
                </span>
                {abs ? (
                  <span
                    className="dest abs"
                    data-testid="abs-dest-line"
                    title={`이 규칙은 정리 위치를 무시하고 ${destFolder(c.dest)} 로 보냅니다`}
                  >
                    ⚠ {shortenTo(destFolder(c.dest), 28)}
                  </span>
                ) : (
                  <span className="dest" title={c.dest}>
                    → {c.dest || "(목적지 없음)"}
                  </span>
                )}
                {abs && relativeSuggestion(c.dest) && !readOnly ? (
                  <button
                    className="link small nowrap"
                    data-testid="make-relative"
                    title={`상대 경로(${relativeSuggestion(c.dest)})로 바꾸면 정리 위치 아래로 갑니다`}
                    onClick={() => set(i, { dest: relativeSuggestion(c.dest) })}
                  >
                    상대 경로로
                  </button>
                ) : (
                  <span className={pv?.count ? "count some" : "count"} data-testid="match-count">
                    {countText}
                  </span>
                )}
                <button className="link toggle" onClick={() => onOpen(i)}>
                  펼치기
                </button>
              </div>
            ) : (
              <>
                <div className="rule-open-head">
                  <span className="handle" aria-hidden style={{ color: "var(--faint)" }}>
                    ⋮⋮
                  </span>
                  <input
                    className="rule-name"
                    aria-label="규칙 이름"
                    placeholder="규칙 이름"
                    value={c.name}
                    disabled={readOnly}
                    onChange={(e) => set(i, { name: e.target.value })}
                  />
                  <span className="count" data-testid="match-count" title={pv?.examples.join(", ")}>
                    {countText}
                    {c.enabled && pv?.examples.length ? ` · 예: ${pv.examples.join(", ")}` : ""}
                    {aiMarked?.has(i) && <span className="tag ai">AI 제안</span>}
                  </span>
                  <button className="link toggle" style={{ textAlign: "right" }} onClick={() => onOpen(null)}>
                    접기
                  </button>
                </div>
                <fieldset disabled={readOnly} className="rule-form">
                  <span className="label">확장자</span>
                  <div className="chips">
                    {c.ext.map((e) => (
                      <span key={e} className="chip">
                        .{e}
                        <button aria-label={`확장자 빼기 ${e}`} onClick={() => set(i, { ext: c.ext.filter((x) => x !== e) })}>
                          ×
                        </button>
                      </span>
                    ))}
                    <input
                      aria-label="확장자 추가"
                      placeholder="확장자 + Enter"
                      style={{ width: 120, height: 26, fontSize: 12 }}
                      value={extDraft}
                      onChange={(e) => setExtDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === ",") {
                          e.preventDefault();
                          addExt(i, extDraft.split(/[,\s]+/));
                          setExtDraft("");
                        }
                      }}
                    />
                  </div>
                  <label className="label" htmlFor={`name-${i}`}>
                    이름에 포함
                  </label>
                  <input
                    id={`name-${i}`}
                    aria-label="이름에 포함"
                    placeholder="예: 로그, 견적서"
                    style={{ maxWidth: 360 }}
                    value={c.nameContains}
                    onChange={(e) => set(i, { nameContains: e.target.value })}
                  />
                  <span className="label">어디로</span>
                  <div className="stack" style={{ gap: 6 }}>
                    <div className="dest-row">
                      <input
                        aria-label="목적지"
                        className="dest"
                        placeholder="문서/{yyyy}-{mm}"
                        value={c.dest}
                        onChange={(e) => set(i, { dest: e.target.value })}
                      />
                      <span className="small muted mono" data-testid="dest-preview" title={pv?.sample_dest ?? ""}>
                        → {where(c, pv?.sample_dest)}
                      </span>
                      <button className="sm" onClick={() => setTreeFor(treeFor === i ? null : i)}>
                        폴더 선택…
                      </button>
                    </div>
                    <div className="chips">
                      {VARS.map((v) => (
                        <button key={v.v} className="chip-btn" title={v.tip} onClick={() => set(i, { dest: c.dest + v.v })}>
                          {v.v}
                        </button>
                      ))}
                    </div>
                  </div>
                  {abs ? (
                    <div className="abs-dest" data-testid="abs-dest-warning">
                      ⚠ 이 규칙은 정리 위치({collectRoots.join(", ")})를 무시하고 <code>{destFolder(c.dest)}</code> 로 보냅니다.
                      {relativeSuggestion(c.dest) && (
                        <>
                          {" "}
                          상대 경로(<code>{relativeSuggestion(c.dest)}</code>)로 바꾸면 정리 위치 아래로 갑니다.{" "}
                          <button
                            className="link"
                            data-testid="make-relative"
                            onClick={() => set(i, { dest: relativeSuggestion(c.dest) })}
                          >
                            상대 경로로 바꾸기
                          </button>
                        </>
                      )}
                    </div>
                  ) : (
                    outside(c.dest, anchors) && <p className="abs-dest">⚠ 감시 폴더(정리 위치) 밖으로 옮깁니다</p>
                  )}
                  {treeFor === i && (
                    <FolderTree
                      source={treeSource}
                      selected={null}
                      onSelect={(p) => {
                        set(i, { dest: destFor(p, anchors) });
                      }}
                    />
                  )}
                  <span className="label">옵션</span>
                  <div className="opts">
                    <label>
                      <input type="checkbox" checked={c.confirm} onChange={(e) => set(i, { confirm: e.target.checked })} /> 옮기기
                      전에 묻기
                    </label>
                    <label title="본문·파일 이름에 개인정보가 있으면 목적지에 암호화 zip(AES-256)으로 보관합니다 (설정 → 보안 · 개인정보)">
                      <input
                        type="checkbox"
                        data-testid="encrypt-toggle"
                        checked={c.encrypt === "pii" || c.encrypt === "always"}
                        onChange={(e) => set(i, { encrypt: e.target.checked ? "pii" : "" })}
                      />{" "}
                      개인정보면 암호화
                    </label>
                    <button
                      className="link"
                      aria-expanded={moreFor === i}
                      data-testid="more-conds"
                      onClick={() => setMoreFor(moreFor === i ? null : i)}
                    >
                      조건 더 보기 (정규식 · 크기 · 수정일)
                    </button>
                  </div>
                  {(moreFor === i || !!(c.nameRegex || c.minSize || c.maxSize || c.newerThanDays || c.olderThanDays)) &&
                    moreFor !== -1 && (
                      <div className="rule-more" data-testid="rule-more">
                        <label className="label" htmlFor={`regex-${i}`}>
                          이름 정규식
                        </label>
                        <input
                          id={`regex-${i}`}
                          style={{ maxWidth: 420 }}
                          placeholder="예: ^스크린샷|Screenshot"
                          value={c.nameRegex}
                          onChange={(e) => set(i, { nameRegex: e.target.value })}
                        />
                        <span className="label">크기</span>
                        <div className="inline">
                          <input
                            className="narrow"
                            placeholder="최소 (예: 10MB)"
                            value={c.minSize}
                            onChange={(e) => set(i, { minSize: e.target.value })}
                          />
                          <span>~</span>
                          <input
                            className="narrow"
                            placeholder="최대"
                            value={c.maxSize}
                            onChange={(e) => set(i, { maxSize: e.target.value })}
                          />
                        </div>
                        <span className="label">수정일</span>
                        <div className="inline">
                          <span>최근</span>
                          <input
                            type="number"
                            min={1}
                            className="narrow"
                            placeholder="N"
                            value={c.newerThanDays ?? ""}
                            onChange={(e) => set(i, { newerThanDays: e.target.value ? Number(e.target.value) : null })}
                          />
                          <span>일 이내</span>
                          <span className="muted">또는</span>
                          <input
                            type="number"
                            min={1}
                            className="narrow"
                            placeholder="N"
                            value={c.olderThanDays ?? ""}
                            onChange={(e) => set(i, { olderThanDays: e.target.value ? Number(e.target.value) : null })}
                          />
                          <span>일 넘게 안 쓴 파일</span>
                        </div>
                        <span className="label">그 밖에</span>
                        <div className="inline">
                          {(c.encrypt === "pii" || c.encrypt === "always") && (
                            <label className="check">
                              <input
                                type="checkbox"
                                checked={c.encrypt === "always"}
                                onChange={(e) => set(i, { encrypt: e.target.checked ? "always" : "pii" })}
                              />
                              개인정보가 아니어도 항상 암호화
                            </label>
                          )}
                          <label className="check">
                            <input type="checkbox" checked={!c.enabled} onChange={(e) => set(i, { enabled: !e.target.checked })} />
                            사용 안 함
                          </label>
                          <button className="sm" disabled={i === 0} onClick={() => move(i, i - 1)} title="위로 (먼저 검사)">
                            ↑ 위로
                          </button>
                          <button className="sm" disabled={i === cards.length - 1} onClick={() => move(i, i + 1)} title="아래로">
                            ↓ 아래로
                          </button>
                        </div>
                        {c.otherKeys.length > 0 && (
                          <p className="muted small" style={{ gridColumn: "1 / -1", margin: 0 }}>
                            그 밖의 설정(그대로 유지, TOML에서 편집): {c.otherKeys.join(", ")}
                          </p>
                        )}
                      </div>
                    )}
                  <span />
                  <button
                    className="link danger"
                    style={{ justifySelf: "start" }}
                    onClick={() => {
                      if (!window.confirm(`규칙 '${c.name || "(이름 없음)"}'을 지울까요? (저장하기 전에는 바뀌지 않습니다)`)) return;
                      onChange(cards.filter((_, j) => j !== i));
                      onOpen(null);
                    }}
                  >
                    이 규칙 삭제
                  </button>
                </fieldset>
              </>
            )}
          </li>
        );
      })}
    </ol>
  );
}
