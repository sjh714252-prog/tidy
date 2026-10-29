import { useRef, useState } from "react";
import * as ipc from "../ipc/commands";
import type { ActivityKind } from "../ipc/types";
import { useActivity } from "../store/activity";
import { formatTime } from "../components/format";
import { CommandCard } from "../components/CommandCard";
import { CustomerCard, OrganizeCard, TodayCard, WatchPanel } from "../components/HomeCards";

const LABEL: Record<ActivityKind, string> = {
  moved: "이동",
  pending: "확인 대기",
  duplicate: "중복",
  no_rule: "규칙 없음",
  skipped: "건너뜀",
  error: "오류",
  warning: "주의",
  info: "알림",
};

/**
 * 홈 (SPEC §14.8 3): 감시 현황 패널(상태바에서) → 말로 시키기(M16) → 아직 정리 안 된 기존 파일
 * → 오늘 한 일 + 고객사별 보기 → 접힌 전체 활동(실시간 피드).
 */
export function ActivityView() {
  const { items, clear } = useActivity();
  const [msg, setMsg] = useState<string | null>(null);
  const [hideNoRule, setHideNoRule] = useState(true);
  const [allOpen, setAllOpen] = useState(false);
  const allRef = useRef<HTMLDetailsElement>(null);

  const undo = async () => {
    try {
      const r = await ipc.undoLast();
      if (!r) setMsg("되돌릴 이동이 없습니다.");
      else
        setMsg(
          `${r.restored.length}건 되돌림` + (r.conflicts.length ? ` · 충돌 ${r.conflicts.length}건: ${r.conflicts.join("; ")}` : ""),
        );
    } catch (e) {
      setMsg(String(e));
    }
  };
  const showAll = () => {
    setAllOpen(true);
    setTimeout(() => allRef.current?.scrollIntoView({ block: "start", behavior: "smooth" }), 50);
  };

  const shown = hideNoRule ? items.filter((a) => a.kind !== "no_rule") : items;
  return (
    <div className="page activity">
      <WatchPanel />
      <CommandCard />
      <OrganizeCard />
      <div className="home-cols">
        <TodayCard onShowAll={showAll} />
        <CustomerCard />
      </div>
      <details
        ref={allRef}
        className="card activity-all"
        data-testid="activity-all"
        open={allOpen}
        onToggle={(e) => setAllOpen((e.target as HTMLDetailsElement).open)}
      >
        <summary>
          전체 활동 <span className="muted small">({shown.length})</span>
        </summary>
        <div className="toolbar" style={{ marginTop: 10 }}>
          <button onClick={() => void undo()}>마지막 이동 되돌리기</button>
          <button onClick={() => void ipc.indexNow()}>지금 인덱싱</button>
          <label className="check">
            <input type="checkbox" checked={hideNoRule} onChange={(e) => setHideNoRule(e.target.checked)} /> 규칙 없는 파일
            숨기기
          </label>
          <div className="spacer" />
          <button className="link" onClick={clear}>
            목록 지우기
          </button>
        </div>
        {msg && <div className="notice">{msg}</div>}
        {shown.length === 0 ? (
          <p className="muted">아직 활동이 없습니다. 감시 폴더에 파일을 만들어 보세요.</p>
        ) : (
          <table className="feed" data-testid="activity-feed">
            <tbody>
              {shown.map((a, i) => (
                <tr key={i} className={a.kind}>
                  <td className="nowrap muted">{formatTime(a.at)}</td>
                  <td className="nowrap">
                    <span className={`tag ${a.kind}`}>{LABEL[a.kind]}</span>
                  </td>
                  <td className="path" title={a.path}>
                    {a.path}
                  </td>
                  <td className="detail">
                    {a.detail}
                    {a.existing && (
                      <button
                        className="link small"
                        style={{ marginLeft: 6 }}
                        data-testid="open-existing"
                        onClick={() => void ipc.revealPath(a.existing as string)}
                      >
                        기존 파일 열기
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </details>
    </div>
  );
}
