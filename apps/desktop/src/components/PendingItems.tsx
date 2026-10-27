import { useState } from "react";
import * as ipc from "../ipc/commands";
import type { Decision, PendingItem } from "../ipc/types";
import { useWatchNames } from "../store/status";
import { formatSize } from "./format";
import { MoreMenu, type MoreItem } from "./MoreMenu";
import { PathText } from "./PathText";
import { watchOf } from "./paths";

/** (M10) Work-context items carry their question first: "…폴더를 만들까요? — 근거". */
export function splitReason(reason: string | null): { question: string | null; reason: string | null } {
  const q = reason?.match(/^(.*?까요\?)\s+—\s+(.*)$/s);
  return { question: q?.[1] ?? null, reason: q ? (q[2] ?? null) : reason };
}

/** "새 고객사 폴더" · "새 이슈" · "새 주제" · "확신 낮음" (SPEC §14.8 4). */
export function proposalKind(p: PendingItem): string | null {
  if (p.source !== "ai") return p.rule ? `규칙: ${p.rule}` : null;
  if (p.confidence != null && p.confidence < 0.8) return "확신 낮음";
  const q = splitReason(p.reason).question ?? "";
  if (q.startsWith("새 고객사")) return "새 고객사 폴더";
  if (q.includes("새 이슈")) return "새 이슈";
  if (q.startsWith("새 주제")) return "새 주제";
  return null;
}

export function Confidence({ value }: { value: number | null }) {
  if (value == null) return <span className="conf">AI</span>;
  const pct = Math.round(value * 100);
  return (
    <span className={pct >= 80 ? "conf" : "conf low"} data-testid="confidence">
      AI {pct}%
    </span>
  );
}

const two = (n: number) => String(n).padStart(2, "0");
export function shortTime(t: number): string {
  const d = new Date(t * 1000);
  return `${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}`;
}

export interface ItemActions {
  busy: boolean;
  resolve: (id: number, d: Decision) => Promise<unknown>;
  /** "대기열에 두기" (popup: hide the window; queue tab: nothing). */
  defer?: () => void;
}

/** The ⋯ menu (5 items, same in the queue tab and the popup). */
function useItemMenu(p: PendingItem, a: ItemActions, startRename: () => void): MoreItem[] {
  const elsewhere = async () => {
    const dir = await ipc.pickFolder(p.suggestedDest ?? p.dir);
    if (dir) await a.resolve(p.id, { kind: "move", destDir: dir });
  };
  return [
    { label: "다른 곳으로…", onClick: () => void elsewhere(), disabled: a.busy },
    { label: "이름 바꿔 이동", onClick: startRename, disabled: a.busy || !p.suggestedDest },
    {
      label: "건너뜀(이번만)",
      title: "이번만 그대로 둡니다. 파일이 다시 바뀌거나 '지금 정리'를 하면 또 묻습니다.",
      onClick: () => void a.resolve(p.id, { kind: "skip" }),
      disabled: a.busy,
    },
    {
      label: "무시(앞으로 묻지 않음)",
      title: "이 파일은 앞으로 묻지도, 옮기지도 않습니다. 설정 → 고급의 '무시한 파일'에서 풀 수 있습니다.",
      onClick: () => void a.resolve(p.id, { kind: "ignore" }),
      disabled: a.busy,
    },
    {
      label: "대기열에 두기",
      title: "결정을 미룹니다. 확인 대기 탭에 남습니다.",
      onClick: () => a.defer?.(),
    },
  ];
}

function Rename({ p, a, onDone }: { p: PendingItem; a: ItemActions; onDone: () => void }) {
  const [name, setName] = useState(p.suggestedName ?? p.name);
  const go = () => {
    if (name.trim()) void a.resolve(p.id, { kind: "move", destDir: null, newName: name.trim() }).then(onDone);
  };
  return (
    <div className="rename-row">
      <input
        aria-label="새 이름"
        value={name}
        autoFocus
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") go();
          if (e.key === "Escape") onDone();
        }}
      />
      <button className="primary sm" disabled={a.busy || !name.trim()} onClick={go}>
        확인
      </button>
      <button className="sm" onClick={onDone}>
        취소
      </button>
    </div>
  );
}

/**
 * (M14) "똑같은 파일이 이미 있습니다" — shown on that item only, with the
 * three answers as buttons (a decision is needed, so not in the ⋯ menu).
 */
export function DuplicateNotice({ p, a }: { p: PendingItem; a: ItemActions }) {
  if (!p.duplicateOf) return null;
  const go = (onDuplicate: "skip" | "rename" | "replace") =>
    void a.resolve(p.id, { kind: "move", destDir: null, onDuplicate });
  return (
    <div className="dup-notice" data-testid="dup-notice">
      <span className="dup-line">
        ⚠ 똑같은 파일이 이미 있습니다 <PathText path={p.duplicateOf} className="sub-path" />
      </span>
      <span className="dup-actions">
        <button className="sm" disabled={a.busy} onClick={() => go("skip")}>
          그대로 두기
        </button>
        <button className="sm" disabled={a.busy} onClick={() => go("rename")}>
          번호 붙여 이동
        </button>
        <button className="sm" disabled={a.busy} onClick={() => go("replace")}>
          기존 것 휴지통 후 이동
        </button>
      </span>
    </div>
  );
}

function Source({ p }: { p: PendingItem }) {
  const w = watchOf(p.dir, useWatchNames());
  return <PathText path={p.dir} alias={w?.alias} root={w?.path} className="sub-path" />;
}

/** Wide window: one 5-column row (SPEC §14.8 4). */
export function PendingRow({
  p,
  a,
  checked,
  onCheck,
}: {
  p: PendingItem;
  a: ItemActions;
  checked: boolean;
  onCheck: () => void;
}) {
  const [renaming, setRenaming] = useState(false);
  const menu = useItemMenu(p, a, () => setRenaming(true));
  const kind = proposalKind(p);
  const { reason } = splitReason(p.reason);
  return (
    <div className={`t-row cols-pending${p.exists ? "" : " gone"}`} data-testid="pending-row" data-id={p.id}>
      <input type="checkbox" aria-label="선택" checked={checked} onChange={onCheck} />
      <div className="cell">
        <span className="name" data-testid="pending-name">
          {p.name} <span className="sub">{p.size != null ? formatSize(p.size) : ""}</span>
          {!p.exists && <span className="sub"> · 파일 없음</span>}
        </span>
        <Source p={p} />
        {p.pii && (
          <span className="pii-badge" data-testid="pii-badge">
            개인정보: {p.pii}
          </span>
        )}
        {renaming && <Rename p={p} a={a} onDone={() => setRenaming(false)} />}
      </div>
      <div className="cell">
        {p.suggestedDest ? (
          <PathText path={p.suggestedDest} clickable={false} />
        ) : (
          <span className="muted">제안 없음</span>
        )}
        {reason && (
          <span className="reason clamp2" title={reason}>
            {reason}
          </span>
        )}
        {p.suggestedName && <span className="reason">새 이름: {p.suggestedName}</span>}
        {p.encrypt && <span className="tag lock">🔒 암호화해서 보관</span>}
        <DuplicateNotice p={p} a={a} />
      </div>
      <div className="cell" style={{ gap: 4 }}>
        <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12, flexWrap: "wrap" }}>
          {p.source === "ai" && <Confidence value={p.confidence} />}
          {kind && <span className="muted">{kind}</span>}
        </span>
        <span className="muted small">{shortTime(p.createdAt)}</span>
      </div>
      <div className="cell right">
        {!p.duplicateOf && (
          <button
            className="primary"
            disabled={a.busy || !p.suggestedDest}
            onClick={() => void a.resolve(p.id, { kind: "move", destDir: null })}
          >
            {p.encrypt ? "암호화 이동" : "이동"}
          </button>
        )}
        <MoreMenu label="다른 선택" items={menu} />
      </div>
    </div>
  );
}

/** Narrow window and the confirm popup: the same item as a card. */
export function PendingCard({
  p,
  a,
  checked,
  onCheck,
  popup = false,
}: {
  p: PendingItem;
  a: ItemActions;
  checked?: boolean;
  onCheck?: () => void;
  popup?: boolean;
}) {
  const [renaming, setRenaming] = useState(false);
  const [more, setMore] = useState(false);
  const menu = useItemMenu(p, a, () => setRenaming(true));
  const elsewhere = async () => {
    const dir = await ipc.pickFolder(p.suggestedDest ?? p.dir);
    if (dir) await a.resolve(p.id, { kind: "move", destDir: dir });
  };
  const { reason } = splitReason(p.reason);
  const kind = proposalKind(p);
  const flush = popup || !onCheck;
  return (
    <article className={`item-card${p.exists ? "" : " gone"}`} data-testid="pending-card" data-id={p.id}>
      <div className="ic-top">
        {onCheck && <input type="checkbox" aria-label="선택" checked={!!checked} onChange={onCheck} />}
        <div className="ic-main">
          <span className="name" title={p.path} data-testid="pending-name">
            {p.name}
          </span>
          <span className="sub-path" style={{ display: "flex", gap: 4, minWidth: 0 }}>
            <Source p={p} />
            {p.size != null && <span className="nowrap">· {formatSize(p.size)}</span>}
          </span>
        </div>
        {p.source === "ai" ? (
          <Confidence value={p.confidence} />
        ) : (
          kind && <span className="muted small nowrap">{kind}</span>
        )}
      </div>
      <div className={flush ? "ic-body flush" : "ic-body"}>
        {p.pii && (
          <span className="pii-badge" data-testid="pii-badge" title="본문·파일 이름에서 찾은 개인정보 (항목 · 건수)">
            개인정보: {p.pii}
          </span>
        )}
        <div className="ic-dest">
          <span className="muted">→ </span>
          {p.suggestedDest ? (
            <PathText path={p.suggestedDest} clickable={false} />
          ) : (
            <span className="muted">(제안 없음)</span>
          )}
        </div>
        {p.encrypt && (
          <span
            className="tag lock"
            style={{ marginLeft: 0, alignSelf: "flex-start" }}
            data-testid="encrypt-tag"
            title="승인하면 암호화 zip(AES-256)으로 만들어 이 위치에 두고, 확인한 뒤 원본을 정리합니다"
          >
            🔒 암호화해서 보관
          </span>
        )}
        {reason && (
          <>
            <div className={more ? "reason" : "reason clamp2"}>
              {reason}
            </div>
            {reason.length > 60 && (
              <button className="link small" style={{ alignSelf: "flex-start" }} onClick={() => setMore(!more)}>
                {more ? "접기" : "더보기"}
              </button>
            )}
          </>
        )}
        {p.suggestedName && <div className="reason">새 이름 제안: {p.suggestedName}</div>}
        {!p.exists && <div className="warn small">파일이 더 이상 없습니다.</div>}
        <DuplicateNotice p={p} a={a} />
        {renaming && <Rename p={p} a={a} onDone={() => setRenaming(false)} />}
      </div>
      <div className={flush ? "ic-actions flush" : "ic-actions"}>
        {!p.duplicateOf && (
          <button
            className="primary"
            disabled={a.busy || !p.suggestedDest}
            onClick={() => void a.resolve(p.id, { kind: "move", destDir: null })}
          >
            {p.encrypt ? "암호화해서 이동" : "이동"}
          </button>
        )}
        <button disabled={a.busy} onClick={() => void elsewhere()}>
          다른 곳…
        </button>
        <MoreMenu label="다른 선택" items={menu} up={popup} />
        <span className="spacer" />
        <span className="muted small">{shortTime(p.createdAt)}</span>
      </div>
    </article>
  );
}
