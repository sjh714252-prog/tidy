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

  const elsewhere = async () => {
    const dir = await ipc.pickFolder(item.suggestedDest ?? item.dir);
    if (dir) await resolve(item.id, { kind: "move", destDir: dir });
  };

  return (
    <div className="confirm">
      <header>
        <strong>정리할까요?</strong>
        {items.length > 1 && <span className="muted"> · 대기 {items.length}건</span>}
      </header>
      <div className="c-file" title={item.path}>
        {item.name}
        {item.size != null && <span className="muted"> · {formatSize(item.size)}</span>}
      </div>
      <div className="muted c-from">{item.dir}</div>
      <div className="c-dest">
        <span className="muted">제안 위치</span>
        <div title={item.suggestedDest ?? ""}>{item.suggestedDest ?? "(제안 없음)"}</div>
        {item.rule && <span className="tag">규칙: {item.rule}</span>}
        {item.source === "ai" && (
          <span className="tag ai" title="AI 제안 — 확신도가 자동 이동 기준보다 낮거나 자동 이동이 허용되지 않은 폴더">
            AI {item.confidence != null ? `${Math.round(item.confidence * 100)}%` : ""}
          </span>
        )}
        {item.reason && <div className="muted small">이유: {item.reason}</div>}
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
          이동
        </button>
        <button disabled={busy} onClick={() => void elsewhere()}>
          다른 곳…
        </button>
        <button disabled={busy || !item.suggestedDest} onClick={() => setRenaming((r) => !r)}>
          이름 바꿔 이동
        </button>
        <button disabled={busy} onClick={() => void resolve(item.id, { kind: "skip" })}>
          건너뜀
        </button>
        <button
          disabled={busy}
          title="이 파일은 앞으로 정리하지 않습니다"
          onClick={() => void resolve(item.id, { kind: "ignore" })}
        >
          무시
        </button>
      </div>
      <footer>
        <button className="link" onClick={() => void ipc.showWindow("main:pending")}>
          대기열 전체 보기
        </button>
        <button className="link" onClick={() => void hideCurrentWindow()}>
          나중에
        </button>
      </footer>
    </div>
  );
}
