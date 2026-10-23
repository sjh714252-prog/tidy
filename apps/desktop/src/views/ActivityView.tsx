import { useState } from "react";
import * as ipc from "../ipc/commands";
import type { ActivityKind } from "../ipc/types";
import { useActivity } from "../store/activity";
import { formatTime } from "../components/format";
import { OrganizeCard, WatchPanel } from "../components/HomeCards";

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

/** 홈 (SPEC §14.6): 감시 현황 패널 + 정리 현황 카드 + live feed of watcher actions. */
export function ActivityView() {
  const { items, clear } = useActivity();
  const [msg, setMsg] = useState<string | null>(null);
  const [hideNoRule, setHideNoRule] = useState(true);

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

  const shown = hideNoRule ? items.filter((a) => a.kind !== "no_rule") : items;
  return (
    <div className="activity">
      <WatchPanel />
      <OrganizeCard />
      <div className="toolbar">
        <button onClick={() => void undo()}>마지막 이동 되돌리기</button>
        <button onClick={() => void ipc.indexNow()}>지금 인덱싱</button>
        <label>
          <input type="checkbox" checked={hideNoRule} onChange={(e) => setHideNoRule(e.target.checked)} /> 규칙 없는 파일 숨기기
        </label>
        <div className="spacer" />
        <button className="link" onClick={clear}>
          목록 지우기
        </button>
      </div>
      {msg && <div className="notice">{msg}</div>}
      {shown.length === 0 ? (
        <p className="muted pad">아직 활동이 없습니다. 감시 폴더에 파일을 만들어 보세요.</p>
      ) : (
        <table className="grid">
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
                <td className="detail">{a.detail}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
