import { useEffect } from "react";
import { onOrganize, onStatus } from "../ipc/events";
import type { FolderOverview, Overview } from "../ipc/types";
import { organizeNow, shortPath, useHome } from "../store/home";

const STATE: Record<FolderOverview["state"], { label: string; cls: string }> = {
  ok: { label: "정상", cls: "ok" },
  offline: { label: "연결 끊김", cls: "warn" },
  paused: { label: "일시정지", cls: "muted" },
  idle: { label: "감시 전", cls: "muted" },
};

function count(f: FolderOverview, o: Overview): string {
  if (f.network) return "네트워크 폴더 — 세지 않음";
  if (!f.backlog) return o.computing ? "세는 중…" : "—";
  const n = f.backlog.capped ? `${o.cap.toLocaleString()}+` : f.backlog.total.toLocaleString();
  return `${n}개`;
}

function useOverview() {
  const { overview, load } = useHome();
  useEffect(() => {
    void load();
    const subs = [onOrganize(() => void load()), onStatus(() => void load())];
    return () => subs.forEach((p) => void p.then((f) => f()));
  }, [load]);
  return overview;
}

/**
 * 정리 현황 (SPEC §14.6): new files are sorted automatically; files that
 * were there before watching started wait for "지금 정리" (a preview —
 * nothing moves until applied).
 */
export function OrganizeCard() {
  const o = useOverview();
  if (!o || !o.folders.length) return null;
  const counted = o.folders.filter((f) => !f.network);
  const waiting = counted.filter((f) => f.backlog && f.backlog.total > 0);
  const total = o.capped ? `${o.total.toLocaleString()}+` : o.total.toLocaleString();
  return (
    <section className="home-card" data-testid="organize-card">
      <p>
        <b>새 파일은 자동으로 정리됩니다.</b>{" "}
        {o.computing && !counted.some((f) => f.backlog) ? (
          <span className="muted">감시 전부터 있던 파일을 세는 중…</span>
        ) : o.total > 0 ? (
          <>
            감시를 시작하기 전부터 있던 파일 <b data-testid="backlog-total">{total}개</b>는 아직 정리되지 않았습니다.
          </>
        ) : (
          <span className="ok">감시 폴더에 정리할 기존 파일이 없습니다.</span>
        )}
      </p>
      {waiting.length > 0 && (
        <ul className="backlog-list">
          {waiting.map((f) => (
            <li key={f.path} data-testid="backlog-folder">
              <span className="path" title={f.path}>
                {shortPath(f.path)}
              </span>{" "}
              <b>{count(f, o)}</b>
              {f.backlog && (
                <span className="muted small">
                  {" "}
                  (규칙에 맞음 {f.backlog.toMove.toLocaleString()} · 규칙 없음 {f.backlog.noRule.toLocaleString()})
                </span>
              )}{" "}
              <button className="primary" onClick={() => void organizeNow(f, o.canCurate)}>
                지금 정리
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** 감시 현황 패널: one row per watch folder (opened from the status bar). */
export function WatchPanel() {
  const open = useHome((s) => s.panelOpen);
  const toggle = useHome((s) => s.togglePanel);
  const o = useOverview();
  if (!open || !o) return null;
  return (
    <section className="home-card watch-panel" data-testid="watch-panel">
      <div className="row">
        <b>감시 현황</b>
        <div className="spacer" />
        <button className="link" onClick={toggle}>
          접기
        </button>
      </div>
      {o.folders.length === 0 ? (
        <p className="muted">감시 중인 폴더가 없습니다. 설정 → ① 감시 폴더에서 추가하세요.</p>
      ) : (
        <table className="grid">
          <thead>
            <tr>
              <th>폴더</th>
              <th>정리 방식</th>
              <th>상태</th>
              <th>제외</th>
              <th>기존 파일 미정리</th>
            </tr>
          </thead>
          <tbody>
            {o.folders.map((f) => (
              <tr key={f.path} data-testid="watch-row">
                <td className="path" title={f.path}>
                  {shortPath(f.path)}
                  {f.network && <span className="tag">NAS</span>}
                </td>
                <td title={f.organizeRoot ?? f.path}>
                  {f.mode === "collect" && f.organizeRoot ? `→ ${shortPath(f.organizeRoot)}` : "이 폴더 안"}
                </td>
                <td className={STATE[f.state].cls}>{STATE[f.state].label}</td>
                <td className="num">{f.exclude ? `${f.exclude}개` : "—"}</td>
                <td>
                  {count(f, o)}{" "}
                  {!!f.backlog?.total && (
                    <button onClick={() => void organizeNow(f, o.canCurate)}>지금 정리</button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
