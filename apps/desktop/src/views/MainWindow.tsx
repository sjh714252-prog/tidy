import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import { onActivity, onNavigate, onPending, onProgress, onStatus } from "../ipc/events";
import { useProgress } from "../store/progress";
import { formatSize } from "../components/format";
import type { MainTab } from "../ipc/types";
import { useActivity } from "../store/activity";
import { usePending } from "../store/pending";
import { useStatus } from "../store/status";
import { useSettings } from "../store/settings";
import { useAiSettings } from "../store/aiSettings";
import { useReport } from "../store/report";
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
  const settingsDirty = useSettings((s) => s.dirty);
  const aiDirty = useAiSettings((s) => s.dirty);
  // Leaving the settings tab with unsaved edits asks first (issue #8).
  const switchTab = (next: MainTab) => {
    if (tab === "settings" && next !== "settings" && (settingsDirty() || aiDirty())) {
      if (!window.confirm("설정에 저장하지 않은 변경이 있습니다. 저장하지 않고 이동할까요?")) return;
    }
    setTab(next);
  };
  const { status, refresh, setPaused } = useStatus();
  const refreshPending = usePending((s) => s.refresh);
  const push = useActivity((s) => s.push);
  const updateProgress = useProgress((s) => s.update);

  useEffect(() => {
    void refresh();
    void refreshPending();
    const subs = [
      onNavigate((t, q) => {
        // The event reaches every window; only main-window tabs apply here.
        if (!TABS.some((x) => x.id === t)) return;
        setTab(t);
        // Wizard "기존 파일 미리보기": rules plan of a watch folder (nothing
        // moves until the user applies selected items).
        if (t === "report" && q.get("plan") === "rules" && q.get("path")) {
          const r = useReport.getState();
          r.setPath(q.get("path") ?? "");
          void r.makePlan("rules");
        }
      }),
      onStatus(() => void refresh()),
      onProgress(updateProgress),
      onPending(() => {
        void refresh();
        void refreshPending();
      }),
      onActivity((a) => {
        push(a);
        if (a.kind === "info" || a.kind === "error" || a.kind === "warning") void refresh();
      }),
    ];
    // Indexing progress and counts are cheap to poll while the window is open.
    const t = setInterval(() => void refresh(), 5000);
    return () => {
      clearInterval(t);
      subs.forEach((p) => void p.then((f) => f()));
    };
  }, [refresh, refreshPending, push, updateProgress]);

  return (
    <div className="main">
      <header className="topbar">
        <div className="brand">Tidy</div>
        <nav>
          {TABS.map((t) => (
            <button key={t.id} className={tab === t.id ? "tab on" : "tab"} onClick={() => switchTab(t.id)}>
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

const PHASE = { copy: "NAS 복사 중", verify: "검증 중", done: "복사 완료" } as const;

function StatusBar() {
  const status = useStatus((s) => s.status);
  const progress = useProgress((s) => s.current);
  if (!status) return null;
  const state = !status.onboarded
    ? "설정 마법사를 마치면 감시를 시작합니다"
    : status.paused
    ? "일시정지됨"
    : status.watching.length
      ? `감시 중: ${status.watching.join(" · ")}`
      : "감시 중인 폴더 없음 — 설정에서 추가하세요";
  return (
    <div className="statusbar">
      <span>{state}</span>
      {!status.onboarded && (
        <button className="link" onClick={() => void ipc.openWizard()}>
          마법사 열기
        </button>
      )}
      <span className="muted">
        인덱스 {status.filesIndexed.toLocaleString()}개{status.indexing ? " (인덱싱 중…)" : ""}
      </span>
      <span className={status.aiState === "unavailable" ? "warn" : "muted"} title={status.aiDetail ?? ""}>
        {AI_LABEL[status.aiState]}
        {status.aiState === "ready" && status.aiDetail ? ` (${status.aiDetail})` : ""}
      </span>
      {status.semanticState !== "off" && (
        <span
          className={status.semanticState === "unavailable" ? "warn" : "muted"}
          title={status.semanticDetail ?? ""}
          data-testid="semantic-state"
        >
          {status.semanticState === "ready" ? "의미검색 켜짐" : "의미검색 일시 중단 (단어 검색만)"}
        </span>
      )}
      {status.offline.length > 0 && (
        <span className="warn" title="연결이 돌아오면 자동으로 다시 감시합니다 (60초마다 확인)">
          연결 끊김(일시정지): {status.offline.join(" · ")}
        </span>
      )}
      {progress && (
        <span title={`${progress.src} → ${progress.dst}`}>
          {PHASE[progress.phase]} {progress.total ? Math.floor((progress.copied * 100) / progress.total) : 100}% (
          {formatSize(progress.copied)} / {formatSize(progress.total)}) {baseName(progress.dst)}
        </span>
      )}
      {status.lastError && <span className="error">{status.lastError}</span>}
    </div>
  );
}

function baseName(p: string): string {
  return p.split(/[\\/]/).pop() ?? p;
}
