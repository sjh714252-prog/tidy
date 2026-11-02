import { useEffect } from "react";
import * as ipc from "../ipc/commands";
import { usePending } from "../store/pending";
import { LearnPanel } from "../components/LearnPanel";
import { MoreMenu } from "../components/MoreMenu";
import { PendingCard, PendingRow, type ItemActions } from "../components/PendingItems";
import { useNarrow } from "../components/useNarrow";
import { NasPendingSection } from "../components/NasSections";

/**
 * 확인 대기 (SPEC §14.8 4): 모두 · 대기 N건 · 선택 M · [선택한 항목 제안
 * 위치로 이동] [건너뜀] [⋯]; a 5-column table, cards below 900px.
 */
export function PendingView() {
  const { items, checked, busy, error, lastResult, lastExisting, refresh, toggle, toggleAll, resolve, resolveChecked } =
    usePending();
  const narrow = useNarrow();

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const elsewhere = async () => {
    const first = items.find((i) => checked.has(i.id));
    const dir = await ipc.pickFolder(first?.suggestedDest ?? first?.dir);
    if (dir) await resolveChecked({ kind: "move", destDir: dir });
  };
  const a: ItemActions = { busy, resolve, defer: () => usePending.setState({ lastResult: "확인 대기에 그대로 둡니다" }) };
  const all = items.length > 0 && checked.size === items.length;

  return (
    <div className="page pending-page pending">
      {/* Decisions change the learner's view: refresh it as the queue shrinks. */}
      <LearnPanel refreshKey={items.length} />
      <div className="toolbar" data-testid="pending-toolbar">
        <label className="check">
          <input type="checkbox" aria-label="모두 선택" checked={all} disabled={!items.length} onChange={toggleAll} /> 모두
        </label>
        <span className="muted" style={{ fontSize: 13 }} data-testid="pending-count">
          {narrow ? `${items.length}건` : `대기 ${items.length}건 · 선택 ${checked.size}`}
        </span>
        <div className="spacer" />
        <button
          className="primary"
          disabled={busy || !checked.size}
          onClick={() => void resolveChecked({ kind: "move", destDir: null })}
        >
          {narrow ? "선택 이동" : "선택한 항목 제안 위치로 이동"}
        </button>
        {!narrow && (
          <button disabled={busy || !checked.size} onClick={() => void resolveChecked({ kind: "skip" })}>
            건너뜀
          </button>
        )}
        <MoreMenu
          items={[
            ...(narrow
              ? [{ label: "건너뜀(이번만)", disabled: busy || !checked.size, onClick: () => void resolveChecked({ kind: "skip" }) }]
              : []),
            { label: "선택한 항목 다른 곳으로…", disabled: busy || !checked.size, onClick: () => void elsewhere() },
            {
              label: "선택한 항목 무시(앞으로 묻지 않음)",
              disabled: busy || !checked.size,
              onClick: () => void resolveChecked({ kind: "ignore" }),
            },
            { label: "새로고침", onClick: () => void refresh(), sep: true },
          ]}
        />
      </div>
      {error && <div className="notice warn error">{error}</div>}
      {lastResult && (
        <div className="notice" data-testid="pending-result">
          {lastResult}
          {lastExisting && (
            <button className="link" style={{ marginLeft: 8 }} onClick={() => void ipc.revealPath(lastExisting)}>
              기존 파일 열기
            </button>
          )}
        </div>
      )}
      {items.length === 0 ? (
        <p className="muted">확인을 기다리는 파일이 없습니다. ("옮기기 전에 묻기" 규칙이나 확신이 낮은 AI 제안이 여기에 쌓입니다)</p>
      ) : narrow ? (
        <div className="item-cards" data-testid="pending-cards">
          {items.map((p) => (
            <PendingCard key={p.id} p={p} a={a} checked={checked.has(p.id)} onCheck={() => toggle(p.id)} />
          ))}
        </div>
      ) : (
        <section className="table" data-testid="pending-table">
          <div className="t-head cols-pending">
            <span />
            <span>파일</span>
            <span>제안 위치</span>
            <span>제안 · 확신도</span>
            <span />
          </div>
          {items.map((p) => (
            <PendingRow key={p.id} p={p} a={a} checked={checked.has(p.id)} onCheck={() => toggle(p.id)} />
          ))}
          <div className="t-foot">⋯ 메뉴: 다른 곳으로… · 이름 바꿔 이동 · 건너뜀(이번만) · 무시(앞으로 묻지 않음) · 대기열에 두기</div>
        </section>
      )}
      <NasPendingSection />
    </div>
  );
}
