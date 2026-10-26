import { useEffect } from "react";
import * as ipc from "../ipc/commands";
import { onOrganize, onStatus } from "../ipc/events";
import type { CustomerStat, FolderOverview, Overview } from "../ipc/types";
import { organizeFolders, organizeNow, shortPath, useHome } from "../store/home";
import { AiNeeded, useAiOn } from "./AiNeeded";
import { formatTime } from "./format";

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
  const aiOn = useAiOn();
  if (!o || !o.folders.length) return null;
  const counted = o.folders.filter((f) => !f.network);
  const waiting = counted.filter((f) => f.backlog && (f.backlog.total > 0 || f.backlog.newFolders > 0));
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
              {!!f.backlog?.total && (
                <>
                  <b>{count(f, o)}</b>
                  <span className="muted small">
                    {" "}
                    (규칙에 맞음 {f.backlog.toMove.toLocaleString()} · 규칙 없음 {f.backlog.noRule.toLocaleString()})
                  </span>{" "}
                  <button className="primary" onClick={() => void organizeNow(f, o.canCurate)}>
                    지금 정리
                  </button>
                </>
              )}
              {!!f.backlog?.newFolders && (
                <span className="new-folders" data-testid="new-folders">
                  {" "}
                  <b>새 폴더 {f.backlog.newFolders}개</b> —{" "}
                  <button
                    className={f.backlog.total ? undefined : "primary"}
                    disabled={!aiOn}
                    title={
                      aiOn
                        ? "하위 폴더를 통째로 업무 맥락(고객사/날짜_이슈/종류)으로 정리합니다. 미리보기 후 승인해야 옮깁니다."
                        : "폴더를 통째로 정리하려면 AI가 필요합니다"
                    }
                    onClick={() => void organizeFolders(f)}
                  >
                    지금 정리
                  </button>
                  {!aiOn && <AiNeeded />}
                </span>
              )}
              {!!f.backlog?.settling && (
                <span className="muted small" title="복사 중이거나 방금 바뀐 폴더는 조용해진 뒤 셉니다">
                  {" "}
                  (복사 중 {f.backlog.settling}개)
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * (M12) 고객사별 보기: per customer how many issue folders and files there
 * are and when they last changed; a click opens the customer's folder.
 */
export function CustomerCard() {
  const { customers, loadCustomers } = useHome();
  useEffect(() => {
    void loadCustomers();
    const sub = onOrganize(() => void loadCustomers());
    return () => void sub.then((f) => f());
  }, [loadCustomers]);
  if (!customers || !customers.length) return null;
  return (
    <section className="home-card" data-testid="customer-card">
      <details open={customers.length <= 8}>
        <summary>
          <b>고객사별 보기</b> <span className="muted small">({customers.length}곳)</span>
        </summary>
        <ul className="customers">
          {customers.map((c) => (
            <CustomerRow key={c.name} c={c} />
          ))}
        </ul>
      </details>
    </section>
  );
}

function CustomerRow({ c }: { c: CustomerStat }) {
  const open = () => void ipc.openFolder(c.dirs[0] ?? "").catch((e) => alert(String(e)));
  return (
    <li data-testid="customer-row" data-name={c.name}>
      <button className="link" title={c.dirs.join("\n")} onClick={open}>
        📁 {c.name}
      </button>
      <span className="small">
        이슈 폴더 {c.issues}개 · 파일 {c.files.toLocaleString()}
        {c.capped ? "+" : ""}개
      </span>
      <span className="muted small">{c.last ? `마지막 작업 ${formatTime(c.last)}` : ""}</span>
      {c.dirs.length > 1 && (
        <span className="muted small" title={c.dirs.join("\n")}>
          (폴더 {c.dirs.length}곳)
        </span>
      )}
    </li>
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
                <td>{f.exclude ? `${f.exclude}개` : "—"}</td>
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
