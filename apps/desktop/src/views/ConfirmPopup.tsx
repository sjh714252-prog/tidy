import { useEffect, useState } from "react";
import { hideCurrentWindow } from "../ipc/transport";
import * as ipc from "../ipc/commands";
import { onPending } from "../ipc/events";
import { usePending } from "../store/pending";
import { formatSize } from "../components/format";

/**
 * Confirm popup (SPEC §5.3): one pending file at a time.
 * 이동 / 다른 곳… / 건너뜀(이번만) / 무시(영구). Hides itself when the queue is empty.
 */
export function ConfirmPopup() {
  const { items, busy, error, lastResult, refresh, resolve } = usePending();

  useEffect(() => {
    void refresh();
    const un = onPending(() => void refresh());
    return () => {
      void un.then((f) => f());
    };
  }, [refresh]);

  const item = items[0];
  const [renaming, setRenaming] = useState(false);
  const [newName, setNewName] = useState("");
  useEffect(() => {
    setRenaming(false);
    setNewName(item?.suggestedName ?? item?.name ?? "");
  }, [item?.id, item?.suggestedName, item?.name]);
  useEffect(() => {
    if (!item && !busy) void hideCurrentWindow();
  }, [item, busy]);

  if (!item) return <div className="confirm muted">대기 중인 파일이 없습니다.</div>;

  // (M10) Work-context items carry their question first: "…폴더를 만들까요? — 근거".
  const q = item.reason?.match(/^(.*?까요\?)\s+—\s+(.*)$/s);
  const question = q?.[1] ?? null;
  const reason = q ? q[2] : item.reason;

  const elsewhere = async () => {
    const dir = await ipc.pickFolder(item.suggestedDest ?? item.dir);
    if (dir) await resolve(item.id, { kind: "move", destDir: dir });
  };

  return (
    <div className="confirm">
      <header>
        <strong data-testid="confirm-question">{question ?? "정리할까요?"}</strong>
        {items.length > 1 && <span className="muted"> · 대기 {items.length}건</span>}
      </header>
      <div className="c-file" title={item.path}>
        {item.name}
        {item.size != null && <span className="muted"> · {formatSize(item.size)}</span>}
      </div>
      <div className="muted c-from">{item.dir}</div>
      {item.pii && (
        <div className="pii-badge" data-testid="pii-badge" title="본문·파일 이름에서 찾은 개인정보 (항목 · 건수)">
          개인정보: {item.pii}
        </div>
      )}
      <div className="c-dest">
        <span className="muted">제안 위치</span>
        <div title={item.suggestedDest ?? ""}>{item.suggestedDest ?? "(제안 없음)"}</div>
        {item.encrypt && (
          <span className="tag lock" data-testid="encrypt-tag" title="승인하면 암호화 zip(AES-256)으로 만들어 이 위치에 두고, 확인한 뒤 원본을 정리합니다">
            🔒 암호화해서 보관
          </span>
        )}
        {item.rule && <span className="tag">규칙: {item.rule}</span>}
        {item.source === "ai" && (
          <span className="tag ai" title="AI 제안 — 자동으로 옮길 조건(확신도 기준, 허용된·이미 있는 폴더)에 맞지 않아 묻습니다">
            AI {item.confidence != null ? `${Math.round(item.confidence * 100)}%` : ""}
          </span>
        )}
        {reason && <div className="muted small">이유: {reason}</div>}
        {item.suggestedName && <div className="muted small">새 이름 제안: {item.suggestedName}</div>}
      </div>
      {renaming && (
        <div className="row">
          <input
            className="wide"
            value={newName}
            autoFocus
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && newName.trim())
                void resolve(item.id, { kind: "move", destDir: null, newName: newName.trim() });
              if (e.key === "Escape") setRenaming(false);
            }}
          />
          <button
            className="primary"
            disabled={busy || !newName.trim() || !item.suggestedDest}
            onClick={() => void resolve(item.id, { kind: "move", destDir: null, newName: newName.trim() })}
          >
            확인
          </button>
        </div>
      )}
      {!item.exists && <div className="warn">파일이 더 이상 없습니다.</div>}
      {error && <div className="error">{error}</div>}
      {lastResult && <div className="muted small">방금: {lastResult}</div>}
      <div className="actions">
        <button
          className="primary"
          disabled={busy || !item.suggestedDest}
          onClick={() => void resolve(item.id, { kind: "move", destDir: null })}
        >
          {item.encrypt ? "암호화해서 이동" : "이동"}
        </button>
        <button disabled={busy} onClick={() => void elsewhere()}>
          다른 곳…
        </button>
        <button disabled={busy || !item.suggestedDest} onClick={() => setRenaming((r) => !r)}>
          이름 바꿔 이동
        </button>
        <button
          disabled={busy}
          title="이번만 그대로 둡니다. 파일이 다시 바뀌거나 '지금 정리'를 하면 또 묻습니다."
          onClick={() => void resolve(item.id, { kind: "skip" })}
        >
          건너뜀
        </button>
        <button
          disabled={busy}
          title="이 파일은 앞으로 묻지도, 옮기지도 않습니다. 설정 ⑥ 고급의 '무시한 파일'에서 풀 수 있습니다."
          onClick={() => void resolve(item.id, { kind: "ignore" })}
        >
          무시
        </button>
      </div>
      <p className="muted small hint">건너뜀 = 이번만 그대로 둠 · 무시 = 이 파일은 앞으로 묻지 않음 (설정 ⑥에서 해제)</p>
      <footer>
        <button className="link" onClick={() => void ipc.showWindow("main:pending")}>
          확인 대기 전체 보기
        </button>
        <button
          className="link"
          data-testid="popup-defer"
          title="결정을 미룹니다. 확인 대기 탭에 남습니다."
          onClick={() => void hideCurrentWindow()}
        >
          대기열에 두기
        </button>
      </footer>
    </div>
  );
}
