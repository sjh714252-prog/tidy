import { useState } from "react";
import type { PreviewReport, RuleCard } from "../ipc/types";
import { FolderTree } from "./FolderTree";
import { destFolder, isAbsoluteDest, relativeSuggestion } from "./destPaths";

const EXT_PRESETS: { label: string; ext: string[] }[] = [
  { label: "문서", ext: ["pdf", "hwp", "hwpx", "doc", "docx", "xls", "xlsx", "ppt", "pptx"] },
  { label: "이미지", ext: ["png", "jpg", "jpeg", "gif", "heic", "webp"] },
  { label: "영상", ext: ["mp4", "mov", "avi", "mkv"] },
  { label: "음악", ext: ["mp3", "wav", "flac", "m4a"] },
  { label: "압축", ext: ["zip", "7z", "rar", "alz", "egg"] },
  { label: "설치", ext: ["exe", "msi", "dmg", "pkg"] },
];

const VARS: { v: string; tip: string }[] = [
  { v: "{yyyy}", tip: "수정 연도" },
  { v: "{mm}", tip: "수정 월" },
  { v: "{dd}", tip: "수정 일" },
  { v: "{ext}", tip: "확장자" },
  { v: "{stem}", tip: "확장자 뺀 이름" },
  { v: "{source_dir_name}", tip: "원래 폴더 이름" },
  { v: "{root}", tip: "기준 폴더(감시 폴더 또는 모을 위치)" },
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

/**
 * Visual rule editor (SPEC §14.1): one card per [[rule]], top = first.
 * "어떤 파일" (extension chips, name contains / regex, size, date) →
 * "어디로" (folder tree + variable chips) → options. Each card shows how
 * many files in the watch folders it would take right now.
 */
export function RuleCards({
  cards,
  preview,
  anchors,
  collectRoots = [],
  readOnly,
  aiMarked,
  onChange,
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
  onChange: (cards: RuleCard[]) => void;
}) {
  const [drag, setDrag] = useState<number | null>(null);
  const [treeFor, setTreeFor] = useState<number | null>(null);
  const [extDraft, setExtDraft] = useState<Record<number, string>>({});

  const set = (i: number, p: Partial<RuleCard>) => onChange(cards.map((c, j) => (j === i ? { ...c, ...p } : c)));
  const move = (from: number, to: number) => {
    if (to < 0 || to >= cards.length || from === to) return;
    const next = [...cards];
    const [c] = next.splice(from, 1);
    if (!c) return;
    next.splice(to, 0, c);
    onChange(next);
  };
  const addExt = (i: number, list: string[]) => {
    const cur = cards[i]?.ext ?? [];
    const add = list.map((e) => e.trim().replace(/^\./, "").toLowerCase()).filter((e) => e && !cur.includes(e));
    if (add.length) set(i, { ext: [...cur, ...add] });
  };

  return (
    <ol className="rule-cards">
      {cards.map((c, i) => {
        const pv = preview?.rules.find((r) => r.index === i);
        return (
          <li
            key={i}
            className={drag === i ? "rule-card dragging" : "rule-card"}
            data-testid="rule-card"
            onDragOver={(e) => {
              if (drag !== null) e.preventDefault();
            }}
            onDrop={(e) => {
              e.preventDefault();
              if (drag !== null) move(drag, i);
              setDrag(null);
            }}
          >
            <header>
              <span
                className="handle"
                draggable={!readOnly}
                onDragStart={() => setDrag(i)}
                onDragEnd={() => setDrag(null)}
                title="끌어서 순서 바꾸기 (위가 우선)"
              >
                ⠿ {i + 1}
              </span>
              <input
                className="rule-name"
                placeholder="규칙 이름"
                value={c.name}
                disabled={readOnly}
                onChange={(e) => set(i, { name: e.target.value })}
              />
              {aiMarked?.has(i) && <span className="tag ai">AI 제안</span>}
              <span className="match-count" data-testid="match-count">
                {!c.enabled
                  ? "사용 안 함"
                  : pv
                    ? `지금 ${pv.count}개${pv.examples.length ? ` · 예: ${pv.examples.join(", ")}` : ""}`
                    : ""}
              </span>
              <div className="spacer" />
              <button className="link" disabled={readOnly || i === 0} onClick={() => move(i, i - 1)} title="위로">
                ↑
              </button>
              <button
                className="link"
                disabled={readOnly || i === cards.length - 1}
                onClick={() => move(i, i + 1)}
                title="아래로"
              >
                ↓
              </button>
              <button className="link" disabled={readOnly} onClick={() => onChange(cards.filter((_, j) => j !== i))}>
                삭제
              </button>
            </header>

            <div className="rule-body">
              <fieldset disabled={readOnly}>
                <legend>어떤 파일</legend>
                <div className="cond">
                  <span className="cond-label">확장자</span>
                  <div>
                    <div className="chips">
                      {c.ext.map((e) => (
                        <span key={e} className="chip">
                          .{e}
                          <button className="link" onClick={() => set(i, { ext: c.ext.filter((x) => x !== e) })}>
                            ×
                          </button>
                        </span>
                      ))}
                      <input
                        className="narrow"
                        placeholder="확장자 + Enter"
                        value={extDraft[i] ?? ""}
                        onChange={(e) => setExtDraft({ ...extDraft, [i]: e.target.value })}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" || e.key === ",") {
                            e.preventDefault();
                            addExt(i, (extDraft[i] ?? "").split(/[,\s]+/));
                            setExtDraft({ ...extDraft, [i]: "" });
                          }
                        }}
                      />
                    </div>
                    <div className="chips presets">
                      {EXT_PRESETS.map((p) => (
                        <button key={p.label} className="chip-btn" onClick={() => addExt(i, p.ext)}>
                          + {p.label}
                        </button>
                      ))}
                    </div>
                  </div>
                  <label className="cond-label" htmlFor={`name-${i}`}>
                    이름에 포함
                  </label>
                  <input
                    id={`name-${i}`}
                    className="wide"
                    placeholder="예: 스크린샷, 견적서"
                    value={c.nameContains}
                    onChange={(e) => set(i, { nameContains: e.target.value })}
                  />
                </div>
                <details
                  className="more-conds"
                  open={!!(c.nameRegex || c.minSize || c.maxSize || c.newerThanDays || c.olderThanDays)}
                >
                  <summary>조건 더 보기 (정규식 · 크기 · 수정일)</summary>
                  <div className="cond">
                    <label className="cond-label" htmlFor={`regex-${i}`}>
                      이름 정규식
                    </label>
                    <input
                      id={`regex-${i}`}
                      className="wide"
                      placeholder="예: ^스크린샷|Screenshot"
                      value={c.nameRegex}
                      onChange={(e) => set(i, { nameRegex: e.target.value })}
                    />
                    <span className="cond-label">크기</span>
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
                    <span className="cond-label">수정일</span>
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
                  </div>
                </details>
              </fieldset>

              <fieldset disabled={readOnly}>
                <legend>어디로</legend>
                <label className="row">
                  <input
                    className="wide dest"
                    placeholder="문서/{yyyy}-{mm}"
                    value={c.dest}
                    onChange={(e) => set(i, { dest: e.target.value })}
                  />
                  <button onClick={() => setTreeFor(treeFor === i ? null : i)}>폴더 선택…</button>
                </label>
                <div className="chips">
                  {VARS.map((v) => (
                    <button key={v.v} className="chip-btn" title={v.tip} onClick={() => set(i, { dest: c.dest + v.v })}>
                      {v.v}
                    </button>
                  ))}
                </div>
                {pv?.sample_dest && <p className="muted small">→ 예: {pv.sample_dest}</p>}
                {collectRoots.length > 0 && isAbsoluteDest(c.dest) ? (
                  <div className="warn small abs-dest" data-testid="abs-dest-warning">
                    <span className="tag warn">정리 위치 무시</span> 이 규칙은 기준 폴더({collectRoots.join(", ")})를 무시하고{" "}
                    <code>{destFolder(c.dest)}</code> 로 보냅니다.
                    {relativeSuggestion(c.dest) && (
                      <>
                        {" "}
                        상대 경로(<code>{relativeSuggestion(c.dest)}</code>)로 바꾸면 기준 폴더 아래로 갑니다.{" "}
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
                  outside(c.dest, anchors) && <p className="warn small">⚠ 감시 폴더(기준 폴더) 밖으로 옮깁니다</p>
                )}
                {treeFor === i && (
                  <FolderTree
                    selected={null}
                    onSelect={(p) => {
                      set(i, { dest: destFor(p, anchors) });
                    }}
                  />
                )}
              </fieldset>

              <fieldset disabled={readOnly}>
                <legend>옵션</legend>
                <label className="row">
                  <input type="checkbox" checked={c.confirm} onChange={(e) => set(i, { confirm: e.target.checked })} />
                  <span>옮기기 전 확인</span>
                </label>
                <label className="row">
                  <input type="checkbox" checked={!c.enabled} onChange={(e) => set(i, { enabled: !e.target.checked })} />
                  <span>사용 안 함</span>
                </label>
                <label
                  className="row"
                  title="본문·파일 이름에 개인정보가 있으면 목적지에 암호화 zip(AES-256)으로 보관합니다 (설정 → 보안)"
                >
                  <input
                    type="checkbox"
                    data-testid="encrypt-toggle"
                    checked={c.encrypt === "pii" || c.encrypt === "always"}
                    onChange={(e) => set(i, { encrypt: e.target.checked ? "pii" : "" })}
                  />
                  <span>🔒 개인정보면 암호화</span>
                </label>
                {(c.encrypt === "pii" || c.encrypt === "always") && (
                  <label className="row sub">
                    <input
                      type="checkbox"
                      checked={c.encrypt === "always"}
                      onChange={(e) => set(i, { encrypt: e.target.checked ? "always" : "pii" })}
                    />
                    <span>개인정보가 아니어도 항상 암호화</span>
                  </label>
                )}
                {c.otherKeys.length > 0 && (
                  <p className="muted small">그 밖의 설정(그대로 유지, TOML에서 편집): {c.otherKeys.join(", ")}</p>
                )}
              </fieldset>
            </div>
          </li>
        );
      })}
    </ol>
  );
}
