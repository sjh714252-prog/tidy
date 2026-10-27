import { useEffect } from "react";
import { hideCurrentWindow } from "../ipc/transport";
import * as ipc from "../ipc/commands";
import { onPending } from "../ipc/events";
import { usePending } from "../store/pending";
import { useStatus } from "../store/status";
import { PendingCard, splitReason } from "../components/PendingItems";

/**
 * Confirm popup (SPEC §5.3, §14.8 4): one pending file at a time, as the
 * same card and ⋯ menu as the queue tab. Hides itself when the queue is empty.
 */
export function ConfirmPopup() {
  const { items, busy, error, lastResult, lastExisting, refresh, resolve } = usePending();
  const refreshStatus = useStatus((s) => s.refresh);

  useEffect(() => {
    void refresh();
    void refreshStatus();
    const un = onPending(() => void refresh());
    return () => {
      void un.then((f) => f());
    };
  }, [refresh, refreshStatus]);

  const item = items[0];
  useEffect(() => {
    if (!item && !busy) void hideCurrentWindow();
  }, [item, busy]);

  if (!item) return <div className="confirm muted">대기 중인 파일이 없습니다.</div>;
  const { question } = splitReason(item.reason);

  return (
    <div className="confirm">
      <header>
        <strong data-testid="confirm-question">{question ?? "정리할까요?"}</strong>
        {items.length > 1 && <span className="muted"> · 대기 {items.length}건</span>}
      </header>
      <PendingCard key={item.id} p={item} a={{ busy, resolve, defer: () => void hideCurrentWindow() }} popup />
      {error && <div className="error small">{error}</div>}
      {lastResult && (
        <div className="muted small">
          방금: {lastResult}
          {lastExisting && (
            <button className="link small" style={{ marginLeft: 6 }} onClick={() => void ipc.revealPath(lastExisting)}>
              기존 파일 열기
            </button>
          )}
        </div>
      )}
      <p className="muted small hint">건너뜀 = 이번만 그대로 둠 · 무시 = 이 파일은 앞으로 묻지 않음 (설정 → 고급에서 해제)</p>
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
