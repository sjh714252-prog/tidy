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
import { useSecurity } from "../store/security";
import { useCurate } from "../store/curate";
import { useNav } from "../store/nav";
import { useReport } from "../store/report";
import { ActivityView } from "./ActivityView";
import { HelpMenu, helpItems } from "../components/HelpMenu";
import { MoreMenu } from "../components/MoreMenu";
import { PathText } from "../components/PathText";
import { baseName } from "../components/paths";
import { useNarrow } from "../components/useNarrow";
import { AboutDialog } from "../components/AboutDialog";
import { useHome } from "../store/home";
import { HistoryView } from "./HistoryView";
import { PendingView } from "./PendingView";
import { ReportView } from "./ReportView";
import { SettingsView } from "./SettingsView";

const TABS: { id: MainTab; label: string }[] = [
  { id: "activity", label: "홈" },
  { id: "pending", label: "확인 대기" },
  { id: "report", label: "정리하기" },
  { id: "history", label: "되돌리기" },
  { id: "settings", label: "설정" },
];

/** Main window: status bar + tabs (홈 / 확인 대기 / 정리하기 / 되돌리기 / 설정, SPEC §14.6). Hidden to tray on close. */
export function MainWindow() {
  const tab = useNav((s) => s.tab);
  const setTab = useNav((s) => s.setTab);
  const curateDirty = useCurate((s) => s.dirty);
  const settingsDirty = useSettings((s) => s.dirty);
  const aiDirty = useAiSettings((s) => s.dirty);
  const securityDirty = useSecurity((s) => s.dirty);
  // Leaving the settings tab with unsaved edits asks first (issue #8).
  const switchTab = (next: MainTab) => {
    if (tab === "settings" && next !== "settings" && (settingsDirty() || aiDirty() || securityDirty() || curateDirty())) {
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
        // Tray "정보…".
        if (q.get("about")) useHome.getState().openAbout(false);
        // "AI 연결에서 켜기" from another window: open that settings page.
        if (t === "settings" && q.get("section")) useNav.getState().goSettings(q.get("section") ?? "");
        // Wizard "기존 파일 미리보기": rules plan of a watch folder (nothing
        // moves until the user applies selected items).
        if (t === "report" && q.get("plan") === "rules" && q.get("path")) {
          const r = useReport.getState();
          void r.setPath(q.get("path") ?? "").then(() => useReport.getState().makePlan("rules"));
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

  const narrow = useNarrow();
  const openAbout = useHome((s) => s.openAbout);
  const [helpErr, setHelpErr] = useState<string | null>(null);
  const searchLabel = `검색 ${status?.hotkey?.replace(/CmdOrCtrl/i, "Ctrl") ?? ""}`.trim();

  return (
    <div className="main">
      <header className="topbar">
        <div className="brand">Tidy</div>
        <nav aria-label="탭">
          {TABS.map((t) => (
            <button
              key={t.id}
              className={tab === t.id ? "tab on" : "tab"}
              aria-current={tab === t.id ? "page" : undefined}
              data-testid={`tab-${t.id}`}
              onClick={() => switchTab(t.id)}
            >
              {t.label}
              {t.id === "pending" && status?.pendingCount ? <span className="badge">{status.pendingCount}</span> : null}
            </button>
          ))}
        </nav>
        <div className="spacer" />
        <div className="tools">
          {narrow ? (
            <>
              <button className="icon" aria-label="검색" title={status?.hotkey} onClick={() => void ipc.showWindow("search")}>
                ⌕
              </button>
              <MoreMenu
                label="메뉴"
                testid="narrow-menu"
                items={[
                  ...(status
                    ? [{ label: status.paused ? "감시 재개" : "일시정지", onClick: () => void setPaused(!status.paused) }]
                    : []),
                  ...helpItems(openAbout, setHelpErr).map((x, i) => ({ ...x, sep: i === 0 })),
                ]}
              >
                ≡
              </MoreMenu>
            </>
          ) : (
            <>
              <button onClick={() => void ipc.showWindow("search")} title={status?.hotkey}>
                {searchLabel}
              </button>
              {status && (
                <button onClick={() => void setPaused(!status.paused)}>{status.paused ? "감시 재개" : "일시정지"}</button>
              )}
              <HelpMenu />
            </>
          )}
          {helpErr && <span className="error small" title={helpErr}>열지 못했습니다</span>}
        </div>
      </header>
      {tab !== "settings" && <StatusBar />}
      <section className="content">
        {tab === "activity" && <ActivityView />}
        {tab === "pending" && <PendingView />}
        {tab === "report" && <ReportView />}
        {tab === "history" && <HistoryView />}
        {tab === "settings" && <SettingsView />}
      </section>
      <AboutDialog />
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
  const togglePanel = useHome((s) => s.togglePanel);
  const openAbout = useHome((s) => s.openAbout);
  if (!status) return null;
  const n = status.watchTotal;
  const roots = status.organizeRoots;
  const places = status.organizeInPlace + roots.length;
  const state = !status.onboarded
    ? { text: "설정 마법사 전", short: "마법사 전", cls: "" }
    : n === 0
      ? { text: "감시 중인 폴더 없음", short: "감시 없음", cls: "" }
      : status.paused
        ? { text: "일시정지", short: "일시정지", cls: "warn" }
        : status.offline.length
          ? { text: `연결 끊김 ${status.offline.length}곳`, short: `끊김 ${status.offline.length}`, cls: "warn" }
          : status.watching.length
            ? { text: "새 파일 자동 정리 중", short: "자동 정리 중", cls: "ok" }
            : { text: "감시 준비 중", short: "준비 중", cls: "" };
  const model = status.aiState === "ready" && status.aiDetail ? aiModel(status.aiDetail) : null;
  return (
    <div className="statusbar" data-testid="statusbar">
      <span
        className={`sb-group ${state.cls === "warn" ? "warn" : ""}`}
        data-testid="watch-state"
        title={status.offline.length ? `연결이 돌아오면 자동으로 다시 감시합니다: ${status.offline.join(", ")}` : ""}
      >
        <span className={`dot ${state.cls}`} aria-hidden />
        <span className="sb-long">{state.text}</span>
        <span className="sb-short">{state.short}</span>
      </span>
      <span className="sb-sep" aria-hidden>
        │
      </span>
      {!status.onboarded ? (
        <span className="sb-group">
          설정 마법사를 마치면 감시를 시작합니다{" "}
          <button className="link" onClick={() => void ipc.openWizard()}>
            마법사 열기
          </button>
        </span>
      ) : n === 0 ? (
        <span className="sb-group">설정 → 감시 폴더에서 추가하세요</span>
      ) : (
        <button
          className="sb-group sb-summary"
          data-testid="watch-summary"
          title={`${status.watchPaths.join("\n")}${roots.length ? `\n정리 위치: ${roots.join(", ")}` : ""}\n(눌러서 감시 현황 보기)`}
          onClick={togglePanel}
        >
          감시 {n}곳 →{" "}
          {!roots.length ? (
            "각 폴더 안에서 정리"
          ) : places === 1 ? (
            <>
              <span className="sb-long">정리 위치 </span>
              <PathText path={roots[0] ?? ""} max={28} inline clickable={false} />
            </>
          ) : (
            `정리 위치 ${places}곳`
          )}
        </button>
      )}
      <div className="spacer" />
      {progress && (
        <span className="sb-group" title={`${progress.src} → ${progress.dst}`}>
          {PHASE[progress.phase]} {progress.total ? Math.floor((progress.copied * 100) / progress.total) : 100}% (
          {formatSize(progress.copied)} / {formatSize(progress.total)}) {baseName(progress.dst)}
        </span>
      )}
      {status.lastError && (
        <span className="sb-group error sb-err" title={status.lastError}>
          {status.lastError}
        </span>
      )}
      <span className="sb-group sb-long">
        인덱스 {status.filesIndexed.toLocaleString()}
        {status.indexing ? " (인덱싱 중…)" : ""}
      </span>
      {status.semanticState !== "off" && (
        <span
          className={`sb-group sb-long ${status.semanticState === "unavailable" ? "warn" : ""}`}
          title={status.semanticDetail ?? ""}
          data-testid="semantic-state"
        >
          {status.semanticState === "ready" ? "의미검색 켜짐" : "의미검색 일시 중단 (단어 검색만)"}
        </span>
      )}
      <span className="sb-sep sb-long" aria-hidden>
        │
      </span>
      <span
        className={`sb-group ${status.aiState === "unavailable" ? "warn" : ""}`}
        title={status.aiDetail ?? ""}
        data-testid="ai-state"
      >
        {AI_LABEL[status.aiState]}
        {model && <span className="sb-long"> · {model}</span>}
      </span>
      {status.update && (
        <button
          className="sb-group update-dot"
          data-testid="update-dot"
          title={`새 버전 ${status.update.latest}`}
          onClick={() => openAbout(false)}
        >
          ● 업데이트 있음
        </button>
      )}
    </div>
  );
}

/** "openai / fake-chat" → "fake-chat" (the full text stays in the title). */
function aiModel(detail: string): string {
  const i = detail.lastIndexOf(" / ");
  return i >= 0 ? detail.slice(i + 3) : detail;
}
