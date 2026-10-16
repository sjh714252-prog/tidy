import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import { onActivity, onNavigate, onPending, onStatus } from "../ipc/events";
import type { MainTab } from "../ipc/types";
import { useActivity } from "../store/activity";
import { usePending } from "../store/pending";
import { useStatus } from "../store/status";
import { ActivityView } from "./ActivityView";
import { HistoryView } from "./HistoryView";
import { PendingView } from "./PendingView";
import { ReportView } from "./ReportView";
import { SettingsView } from "./SettingsView";

const TABS: { id: MainTab; label: string }[] = [
  { id: "activity", label: "활동" },
  { id: "pending", label: "대기열" },
  { id: "report", label: "리포트" },
  { id: "history", label: "이동 기록" },
  { id: "settings", label: "설정" },
];

/** Main window: status bar + tabs (활동 / 대기열 / 설정). Hidden to tray on close. */
export function MainWindow() {
  const [tab, setTab] = useState<MainTab>("activity");
  const { status, refresh, setPaused } = useStatus();
  const refreshPending = usePending((s) => s.refresh);
  const push = useActivity((s) => s.push);

  useEffect(() => {
    void refresh();
    void refreshPending();
    const subs = [
      onNavigate(setTab),
      onStatus(() => void refresh()),
      onPending(() => {
        void refresh();
        void refreshPending();
      }),
      onActivity((a) => {
        push(a);
        if (a.kind === "info" || a.kind === "error") void refresh();
      }),
    ];
    // Indexing progress and counts are cheap to poll while the window is open.
    const t = setInterval(() => void refresh(), 5000);
    return () => {
      clearInterval(t);
      subs.forEach((p) => void p.then((f) => f()));
    };
  }, [refresh, refreshPending, push]);

  return (
    <div className="main">
      <header className="topbar">
        <div className="brand">Tidy</div>
        <nav>
          {TABS.map((t) => (
            <button key={t.id} className={tab === t.id ? "tab on" : "tab"} onClick={() => setTab(t.id)}>
              {t.label}
              {t.id === "pending" && status?.pendingCount ? <span className="badge">{status.pendingCount}</span> : null}
            </button>
          ))}
        </nav>
        <div className="spacer" />
        <button onClick={() => void ipc.showWindow("search")} title={status?.hotkey}>
          검색 ({status?.hotkey ?? "…"})
        </button>
        {status && (
          <button onClick={() => void setPaused(!status.paused)}>{status.paused ? "감시 재개" : "일시정지"}</button>
        )}
      </header>
      <StatusBar />
      <section className="content">
        {tab === "activity" && <ActivityView />}
        {tab === "pending" && <PendingView />}
        {tab === "report" && <ReportView />}
        {tab === "history" && <HistoryView />}
        {tab === "settings" && <SettingsView />}
      </section>
    </div>
  );
}

const AI_LABEL = {
  off: "AI 꺼짐 (규칙만)",
  ready: "AI 사용",
  unavailable: "AI 사용 불가 — 규칙만",
  starting: "AI 확인 중…",
} as const;

function StatusBar() {
  const status = useStatus((s) => s.status);
  if (!status) return null;
  const state = status.paused
    ? "일시정지됨"
    : status.watching.length
      ? `감시 중: ${status.watching.join(" · ")}`
      : "감시 중인 폴더 없음 — 설정에서 추가하세요";
  return (
    <div className="statusbar">
      <span>{state}</span>
      <span className="muted">
        인덱스 {status.filesIndexed.toLocaleString()}개{status.indexing ? " (인덱싱 중…)" : ""}
      </span>
      <span className={status.aiState === "unavailable" ? "warn" : "muted"} title={status.aiDetail ?? ""}>
        {AI_LABEL[status.aiState]}
        {status.aiState === "ready" && status.aiDetail ? ` (${status.aiDetail})` : ""}
      </span>
      {status.lastError && <span className="error">{status.lastError}</span>}
    </div>
  );
}
