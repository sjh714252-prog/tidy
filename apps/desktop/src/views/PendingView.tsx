import { useEffect } from "react";
import * as ipc from "../ipc/commands";
import { usePending } from "../store/pending";
import { formatSize, formatTime } from "../components/format";
import { LearnPanel } from "../components/LearnPanel";

/** Queue screen (SPEC §9.4): multi-select, bulk move/skip/ignore. */
export function PendingView() {
  const { items, checked, busy, error, lastResult, refresh, toggle, toggleAll, resolve, resolveChecked } =
    usePending();

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const elsewhere = async (id?: number) => {
    const first = items.find((i) => (id ? i.id === id : checked.has(i.id)));
    const dir = await ipc.pickFolder(first?.suggestedDest ?? first?.dir);
    if (!dir) return;
    if (id) await resolve(id, { kind: "move", destDir: dir });
    else await resolveChecked({ kind: "move", destDir: dir });
  };

  return (
    <div className="pending">
      {/* Decisions change the learner's view: refresh it as the queue shrinks. */}
      <LearnPanel refreshKey={items.length} />
      <div className="toolbar">
        <span>{checked.size ? `${checked.size}건 선택` : `대기 ${items.length}건`}</span>
        <button
          className="primary"
          disabled={busy || !checked.size}
          onClick={() => void resolveChecked({ kind: "move", destDir: null })}
        >
          제안 위치로 이동
        </button>
        <button disabled={busy || !checked.size} onClick={() => void elsewhere()}>
          다른 곳으로…
        </button>
        <button disabled={busy || !checked.size} onClick={() => void resolveChecked({ kind: "skip" })}>
          건너뜀
        </button>
        <button disabled={busy || !checked.size} onClick={() => void resolveChecked({ kind: "ignore" })}>
          무시
        </button>
        <div className="spacer" />
        <button className="link" onClick={() => void refresh()}>
          새로고침
        </button>
      </div>
      {error && <div className="error pad">{error}</div>}
      {lastResult && <div className="notice">{lastResult}</div>}
      {items.length === 0 ? (
        <p className="muted pad">확인을 기다리는 파일이 없습니다. (confirm = true 규칙에 걸린 파일이 여기에 쌓입니다)</p>
      ) : (
        <table className="grid">
          <thead>
            <tr>
              <th>
                <input type="checkbox" checked={checked.size === items.length} onChange={toggleAll} />
              </th>
              <th>파일</th>
              <th>제안 위치</th>
              <th>제안</th>
              <th>들어온 시각</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {items.map((p) => (
              <tr key={p.id} className={p.exists ? "" : "gone"}>
                <td>
                  <input type="checkbox" checked={checked.has(p.id)} onChange={() => toggle(p.id)} />
                </td>
                <td className="path" title={p.path}>
                  <div>{p.name}</div>
                  <div className="muted small">
                    {p.dir} {p.size != null && `· ${formatSize(p.size)}`} {!p.exists && "· 파일 없음"}
                  </div>
                </td>
                <td className="path" title={p.suggestedDest ?? ""}>
                  {p.suggestedDest ?? "—"}
                </td>
                <td className="nowrap" title={p.reason ?? ""}>
                  {p.source === "ai" ? (
                    <>
                      <span className="tag ai">AI {p.confidence != null ? `${Math.round(p.confidence * 100)}%` : ""}</span>
                      {p.reason && <div className="muted small reason">{p.reason}</div>}
                    </>
                  ) : (
                    <>규칙: {p.rule}</>
                  )}
                  {p.suggestedName && <div className="muted small">새 이름: {p.suggestedName}</div>}
                </td>
                <td className="nowrap muted">{formatTime(p.createdAt)}</td>
                <td className="nowrap">
                  <button
                    disabled={busy || !p.suggestedDest}
                    onClick={() => void resolve(p.id, { kind: "move", destDir: null })}
                  >
                    이동
                  </button>
                  <button disabled={busy} onClick={() => void elsewhere(p.id)}>
                    다른 곳…
                  </button>
                  <button disabled={busy} onClick={() => void resolve(p.id, { kind: "skip" })}>
                    건너뜀
                  </button>
                  <button disabled={busy} onClick={() => void resolve(p.id, { kind: "ignore" })}>
                    무시
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
